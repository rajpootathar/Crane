use std::path::PathBuf;
use std::time::Instant;

use crate::git_log::data::Sha;

pub struct GitLogState {
    pub height: f32,
    pub col_refs_width: f32,
    pub col_details_width: f32,
    pub maximized: bool,
    pub selected_commit: Option<Sha>,
    pub selected_file: Option<PathBuf>,
    pub last_poll: Instant,
}

impl GitLogState {
    pub fn new() -> Self {
        Self {
            height: 320.0,
            col_refs_width: 220.0,
            col_details_width: 360.0,
            maximized: false,
            selected_commit: None,
            selected_file: None,
            last_poll: Instant::now(),
        }
    }
}

impl Default for GitLogState {
    fn default() -> Self {
        Self::new()
    }
}
