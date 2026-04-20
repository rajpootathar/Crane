pub mod confirm_close_tab;
pub mod confirm_remove_worktree;
pub mod empty_state;
pub mod help;
pub mod lsp_install;
pub mod missing_project;
pub mod new_workspace;
pub mod settings;
pub mod settings_lsp;
pub mod tab_switcher;
pub mod update_toast;

pub use confirm_close_tab::render as render_confirm_close_tab;
pub use confirm_remove_worktree::render as render_confirm_remove_worktree;
pub use empty_state::render as render_empty_state;
pub use help::render as render_help_modal;
pub use lsp_install::render as render_lsp_install_prompt;
pub use lsp_install::render_download_toast as render_lsp_download_toast;
pub use missing_project::render as render_missing_project_modal;
pub use new_workspace::render as render_new_workspace_modal;
pub use settings::render as render_settings_modal;
pub use update_toast::render as render_update_toast;

pub fn open_in_file_manager(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer").arg(path).spawn();
    }
}
