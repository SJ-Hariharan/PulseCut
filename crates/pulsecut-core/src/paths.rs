//! Application directories. Uses the platform conventions from `directories`
//! (macOS: ~/Library/Application Support/app.PulseCut.PulseCut, Windows: %APPDATA%\PulseCut\PulseCut).
//! `PULSECUT_HOME` overrides everything (tests, portable installs).

use crate::error::{Error, Result};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<AppPaths> {
        if let Ok(home) = std::env::var("PULSECUT_HOME") {
            return Ok(AppPaths::rooted(PathBuf::from(home)));
        }
        let pd = directories::ProjectDirs::from("app", "PulseCut", "PulseCut")
            .ok_or_else(|| Error::other("cannot determine home directory"))?;
        Ok(AppPaths {
            config_dir: pd.config_dir().to_path_buf(),
            data_dir: pd.data_dir().to_path_buf(),
            cache_dir: pd.cache_dir().to_path_buf(),
        })
    }

    pub fn rooted(root: PathBuf) -> AppPaths {
        AppPaths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
        }
    }

    pub fn ensure(&self) -> Result<()> {
        for d in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.library_dir_default(),
            &self.raw_dir(),
            &self.assets_dir(),
            &self.logs_dir(),
            &self.work_dir(),
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.toml")
    }
    pub fn database_file(&self) -> PathBuf {
        self.data_dir.join("pulsecut.sqlite")
    }
    pub fn library_dir_default(&self) -> PathBuf {
        self.data_dir.join("library")
    }
    pub fn raw_dir(&self) -> PathBuf {
        self.data_dir.join("raw")
    }
    pub fn assets_dir(&self) -> PathBuf {
        self.data_dir.join("assets")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }
    pub fn work_dir(&self) -> PathBuf {
        self.cache_dir.join("work")
    }
    pub fn segments_dir(&self) -> PathBuf {
        self.cache_dir.join("segments")
    }
    pub fn models_dir(&self) -> PathBuf {
        self.data_dir.join("models")
    }
}

/// Reject paths that escape `root` (".." components, absolute paths) — used for anything
/// derived from user-imported packs or remote metadata.
pub fn safe_join(root: &Path, rel: &str) -> Result<PathBuf> {
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err(Error::UnsafePath(p.to_path_buf()));
    }
    for c in p.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(Error::UnsafePath(p.to_path_buf())),
        }
    }
    Ok(root.join(p))
}

/// Make a string safe to use as a file-name stem.
pub fn sanitize_file_stem(s: &str, max: usize) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches('_').to_string();
    let mut out: String = out.chars().take(max).collect();
    if out.is_empty() {
        out.push_str("clip");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_rejects_escape() {
        let root = Path::new("/tmp/x");
        assert!(safe_join(root, "../etc/passwd").is_err());
        assert!(safe_join(root, "/etc/passwd").is_err());
        assert!(safe_join(root, "sounds/boom.wav").is_ok());
    }

    #[test]
    fn stems() {
        assert_eq!(sanitize_file_stem("Hello, World!! / 42", 40), "Hello_World_42");
        assert_eq!(sanitize_file_stem("///", 10), "clip");
    }
}
