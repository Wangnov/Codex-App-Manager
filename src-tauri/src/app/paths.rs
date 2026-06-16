use std::path::PathBuf;

/// Manager data directory shared by settings, provenance, and operation locks.
pub fn data_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("io.github", "wangnov", "codexappmanager")
        .map(|dirs| dirs.data_dir().to_path_buf())
}

pub fn settings_path() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("settings.json"))
}

pub fn provenance_path() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("provenance.json"))
}
