use std::io;

use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const RUN_KEY_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APP_VALUE: &str = "BlindSpot";

pub fn is_run_on_startup_enabled() -> io::Result<bool> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let run = hkcu.open_subkey(RUN_KEY_PATH)?;
    let value: Result<String, _> = run.get_value(APP_VALUE);
    Ok(value.is_ok())
}

pub fn set_run_on_startup(enabled: bool) -> io::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = hkcu.create_subkey(RUN_KEY_PATH)?;
    if enabled {
        let exe = std::env::current_exe()?;
        let command = format!("\"{}\"", exe.display());
        run.set_value(APP_VALUE, &command)?;
    } else {
        let _ = run.delete_value(APP_VALUE);
    }
    Ok(())
}

pub fn refresh_startup_path_if_enabled() -> io::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let run = match hkcu.open_subkey(RUN_KEY_PATH) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    let existing: String = match run.get_value(APP_VALUE) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    let exe = std::env::current_exe()?;
    let expected = format!("\"{}\"", exe.display());
    if existing != expected {
        drop(run);
        set_run_on_startup(true)?;
    }
    Ok(())
}