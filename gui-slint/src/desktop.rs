use crate::{auth, config::AppConfig, env_check};

pub trait DesktopServices: Send + Sync {
    fn clipboard(&self, value: &str) -> Result<(), String>;
    fn open(&self, value: &str) -> Result<(), String>;
    fn autostart(&self, enabled: bool) -> Result<(), String>;
    fn dependencies(&self) -> env_check::DependencyReport;
    fn authenticate(
        &self,
        config: &AppConfig,
        process: &auth::ProcessHandle,
        cancelled: &std::sync::atomic::AtomicBool,
        on_device: Box<dyn FnMut(String, String) + Send>,
    ) -> Result<(), String>;
}

pub struct SystemDesktop;

impl DesktopServices for SystemDesktop {
    fn clipboard(&self, value: &str) -> Result<(), String> {
        let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
        clipboard.set_text(value).map_err(|error| error.to_string())
    }

    fn open(&self, value: &str) -> Result<(), String> {
        open::that_detached(value).map_err(|error| error.to_string())
    }

    fn autostart(&self, enabled: bool) -> Result<(), String> {
        crate::autostart::set_autostart(enabled).map_err(|error| error.to_string())
    }

    fn dependencies(&self) -> env_check::DependencyReport {
        env_check::check_all()
    }

    fn authenticate(
        &self,
        config: &AppConfig,
        process: &auth::ProcessHandle,
        cancelled: &std::sync::atomic::AtomicBool,
        on_device: Box<dyn FnMut(String, String) + Send>,
    ) -> Result<(), String> {
        auth::run_auth_command(config, process, cancelled, on_device)
    }
}
