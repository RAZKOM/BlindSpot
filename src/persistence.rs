use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::state::AppConfig;

const CONFIG_FILE: &str = "config.json";

pub fn app_data_dir() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let parent = exe
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Executable directory not found"))?;
    Ok(parent.to_path_buf())
}

pub fn load_config() -> io::Result<(AppConfig, bool)> {
    let path = app_data_dir()?.join(CONFIG_FILE);
    if !path.exists() {
        return Ok((AppConfig::default(), true));
    }

    let text = fs::read_to_string(&path)?;
    let cfg = serde_json::from_str::<AppConfig>(&text).unwrap_or_default();
    Ok((cfg, false))
}

pub fn save_config(config: &AppConfig) -> io::Result<()> {
    let path = app_data_dir()?.join(CONFIG_FILE);
    let json = serde_json::to_string_pretty(config)?;
    atomic_write(&path, &json)
}

fn atomic_write(path: &Path, content: &str) -> io::Result<()> {
    let temp = path.with_extension("tmp");
    fs::write(&temp, content)?;
    fs::rename(temp, path)?;
    Ok(())
}