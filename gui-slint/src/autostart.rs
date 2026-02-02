use std::error::Error;
use std::path::PathBuf;

pub fn set_autostart(enable: bool) -> Result<(), Box<dyn Error>> {
    let exe = std::env::current_exe()?;
    set_autostart_with_path(enable, exe)
}

#[cfg(target_os = "windows")]
fn set_autostart_with_path(enable: bool, exe_path: PathBuf) -> Result<(), Box<dyn Error>> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_WRITE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let run = hkcu.open_subkey_with_flags(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
        KEY_WRITE,
    )?;

    let app_name = "CopilotApiGui";
    if enable {
        let value = exe_path.to_string_lossy().to_string();
        run.set_value(app_name, &value)?;
    } else {
        let _ = run.delete_value(app_name);
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn set_autostart_with_path(_enable: bool, _exe_path: PathBuf) -> Result<(), Box<dyn Error>> {
    Ok(())
}
