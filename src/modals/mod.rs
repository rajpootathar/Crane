pub mod empty_state;
pub mod help;
pub mod lsp_install;
pub mod new_workspace;
pub mod settings;
pub mod update_toast;

pub use empty_state::render as render_empty_state;
pub use help::render as render_help_modal;
pub use lsp_install::render as render_lsp_install_prompt;
pub use lsp_install::render_download_toast as render_lsp_download_toast;
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
