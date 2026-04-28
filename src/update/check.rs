use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};
use std::time::{SystemTime, UNIX_EPOCH};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_LATEST: &str =
    "https://api.github.com/repos/rajpootathar/Crane/releases/latest";
const USER_AGENT: &str = "Crane-Update-Checker";
pub const REMIND_AFTER_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Clone, Debug)]
pub struct AvailableUpdate {
    pub version: String,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum PromptState {
    Dismissed,
    RemindAt(u64),
}

pub struct UpdateCheck {
    pub available: Option<AvailableUpdate>,
    pub prompts: HashMap<String, PromptState>,
    pub dismissed_this_session: Option<String>,
    pub manual_check: bool,
    pub manual_result_seen: bool,
    rx: Option<Receiver<Option<AvailableUpdate>>>,
}

impl UpdateCheck {
    pub fn new(prompts: HashMap<String, PromptState>) -> Self {
        Self {
            available: None,
            prompts,
            dismissed_this_session: None,
            manual_check: false,
            manual_result_seen: true,
            rx: None,
        }
    }

    pub fn spawn_check(&mut self, ctx: egui::Context) {
        if self.rx.is_some() || self.available.is_some() {
            return;
        }
        let (tx, rx) = channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let result = fetch_latest();
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    pub fn drain(&mut self) {
        if let Some(rx) = self.rx.as_ref()
            && let Ok(result) = rx.try_recv() {
                self.available = result;
                self.rx = None;
                if self.manual_check {
                    self.manual_result_seen = false;
                }
            }
    }

    pub fn should_show(&self) -> bool {
        let Some(update) = &self.available else {
            return false;
        };
        if self.manual_check {
            return true;
        }
        if self
            .dismissed_this_session
            .as_deref()
            .is_some_and(|v| v == update.version)
        {
            return false;
        }
        match self.prompts.get(&update.version) {
            None => true,
            Some(PromptState::Dismissed) => false,
            Some(PromptState::RemindAt(ts)) => now_secs() >= *ts,
        }
    }

    pub fn dismiss_session(&mut self) {
        if let Some(u) = &self.available {
            self.dismissed_this_session = Some(u.version.clone());
        }
        // Clear manual_check too — otherwise should_show() short-
        // circuits on it and the toast keeps appearing despite
        // dismissed_this_session being set. The manual flag's job
        // is done once the user has acted on the prompt.
        self.manual_check = false;
        self.manual_result_seen = true;
    }

    pub fn dismiss_forever(&mut self) {
        if let Some(u) = &self.available {
            self.prompts
                .insert(u.version.clone(), PromptState::Dismissed);
            self.dismissed_this_session = Some(u.version.clone());
        }
        self.manual_check = false;
        self.manual_result_seen = true;
    }

    pub fn remind_later(&mut self) {
        if let Some(u) = &self.available {
            self.prompts.insert(
                u.version.clone(),
                PromptState::RemindAt(now_secs() + REMIND_AFTER_SECS),
            );
            self.dismissed_this_session = Some(u.version.clone());
        }
        self.manual_check = false;
        self.manual_result_seen = true;
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

fn fetch_latest() -> Option<AvailableUpdate> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .build();
    let response = agent.get(GITHUB_LATEST).call().ok()?;
    let release: Release = response.into_json().ok()?;
    if release.draft || release.prerelease {
        return None;
    }
    let tag = release.tag_name.trim_start_matches('v').to_string();
    if is_newer(&tag, CURRENT_VERSION) {
        Some(AvailableUpdate {
            version: tag,
            url: release.html_url,
        })
    } else {
        None
    }
}

fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let parts: Vec<&str> = s.split('.').take(3).collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(c), Some(cur)) => c > cur,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_major_minor_patch() {
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.3", "0.1.2"));
        assert!(is_newer("0.2.0", "0.1.9"));
    }

    #[test]
    fn not_newer_when_equal_or_older() {
        assert!(!is_newer("0.1.2", "0.1.2"));
        assert!(!is_newer("0.1.1", "0.1.2"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn bad_versions_are_not_newer() {
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(!is_newer("0.1", "0.1.0"));
    }
}
