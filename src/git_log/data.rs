use std::path::Path;
use std::process::Command;

pub type Sha = String;

#[derive(Clone, Debug, PartialEq)]
pub struct CommitRecord {
    pub sha: Sha,
    pub parents: Vec<Sha>,
    pub author: String,
    pub date: String,        // ISO 8601 string (parse on demand to avoid chrono in hot path)
    pub subject: String,
    pub refs_decoration: String,
}

const FIELD_SEP: char = '\x1f';
const RECORD_SEP: char = '\n';

/// Format: `%H<US>%P<US>%an<US>%aI<US>%s<US>%D<LF>`
pub fn parse_log_output(stdout: &str) -> Vec<CommitRecord> {
    let mut out = Vec::new();
    for line in stdout.split(RECORD_SEP) {
        if line.is_empty() { continue; }
        let mut fields = line.split(FIELD_SEP);
        let (Some(sha), Some(parents), Some(author), Some(date), Some(subject), Some(refs)) =
            (fields.next(), fields.next(), fields.next(), fields.next(), fields.next(), fields.next())
            else { continue; };
        let parents: Vec<Sha> = if parents.is_empty() {
            Vec::new()
        } else {
            parents.split(' ').map(String::from).collect()
        };
        out.push(CommitRecord {
            sha: sha.to_string(),
            parents,
            author: author.to_string(),
            date: date.to_string(),
            subject: subject.to_string(),
            refs_decoration: refs.to_string(),
        });
    }
    out
}

/// Run `git log --all --date-order --pretty=...` against `repo` and
/// return parsed commit records. `max_count` caps the result —
/// pass a large value (e.g. 10_000) for the initial load. Returns
/// empty Vec on any error.
pub fn load_commits(repo: &Path, max_count: usize) -> Vec<CommitRecord> {
    let format = format!(
        "--pretty=format:%H{us}%P{us}%an{us}%aI{us}%s{us}%D",
        us = '\x1f'
    );
    let max_count_arg = format!("--max-count={max_count}");
    let out = match Command::new("git")
        .args([
            "log",
            "--all",
            "--date-order",
            &format,
            &max_count_arg,
        ])
        .current_dir(repo)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_log_output(&stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(sha: &str, parents: &str, author: &str, date: &str, subject: &str, refs: &str) -> String {
        format!("{sha}\x1f{parents}\x1f{author}\x1f{date}\x1f{subject}\x1f{refs}")
    }

    #[test]
    fn parses_single_commit_no_parents() {
        let stdout = line("abc123", "", "Alice", "2026-05-01T10:00:00+00:00", "Initial", "");
        let parsed = parse_log_output(&stdout);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].sha, "abc123");
        assert!(parsed[0].parents.is_empty());
        assert_eq!(parsed[0].author, "Alice");
        assert_eq!(parsed[0].subject, "Initial");
    }

    #[test]
    fn parses_two_parent_merge() {
        let stdout = line("m1", "p1 p2", "Bob", "2026-05-02T10:00:00+00:00", "Merge", "");
        let parsed = parse_log_output(&stdout);
        assert_eq!(parsed[0].parents, vec!["p1".to_string(), "p2".to_string()]);
    }

    #[test]
    fn parses_octopus_three_parents() {
        let stdout = line("m1", "p1 p2 p3", "Carol", "2026-05-03T10:00:00+00:00", "Octopus", "");
        let parsed = parse_log_output(&stdout);
        assert_eq!(parsed[0].parents.len(), 3);
    }

    #[test]
    fn malformed_lines_skip_cleanly() {
        let stdout = format!(
            "good\x1f\x1fAuthor\x1f2026-05-01T10:00:00+00:00\x1fSubject\x1f\nshort_line_only_two_fields\nanother\x1f\x1fAuthor\x1f2026-05-01T10:00:00+00:00\x1fSubject\x1f"
        );
        let parsed = parse_log_output(&stdout);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].sha, "good");
        assert_eq!(parsed[1].sha, "another");
    }

    #[test]
    fn subjects_with_pipe_chars_dont_corrupt() {
        let stdout = line("abc", "", "Author", "2026-05-01T10:00:00+00:00", "fix: a | b | c", "");
        let parsed = parse_log_output(&stdout);
        assert_eq!(parsed[0].subject, "fix: a | b | c");
    }

    #[test]
    fn refs_decoration_carries_through() {
        let stdout = line("abc", "", "Author", "2026-05-01T10:00:00+00:00", "Subject",
            " (HEAD -> main, origin/main, tag: v1.0)");
        let parsed = parse_log_output(&stdout);
        assert!(parsed[0].refs_decoration.contains("HEAD"));
        assert!(parsed[0].refs_decoration.contains("v1.0"));
    }
}
