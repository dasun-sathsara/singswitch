use std::fs;
use std::path::{Path, PathBuf};

use crate::profile::validate_name;

/// Sidecar extension recording the URL a profile was pulled from.
///
/// Using `.url` keeps sidecars separate from `*.json` profile listings.
pub const SOURCE_EXTENSION: &str = ".url";

/// Path to the `.url` sidecar for a validated profile name.
pub fn source_path_for(profiles_dir: &Path, name: &str) -> PathBuf {
    profiles_dir.join(format!("{name}{SOURCE_EXTENSION}"))
}

/// Read the recorded remote URL for `name`, returning `Ok(None)` if no sidecar exists.
pub fn read_source(profiles_dir: &Path, name: &str) -> std::io::Result<Option<String>> {
    let path = source_path_for(profiles_dir, name);
    match fs::read_to_string(path) {
        Ok(content) => {
            let trimmed = content.trim().to_string();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed))
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Record the remote URL a profile was downloaded from.
pub fn write_source(profiles_dir: &Path, name: &str, url: &str) -> std::io::Result<()> {
    let path = source_path_for(profiles_dir, name);
    fs::write(path, format!("{}\n", url.trim()))
}

/// Remove the `.url` sidecar when a profile is deleted. Missing files succeed.
pub fn remove_source(profiles_dir: &Path, name: &str) -> std::io::Result<()> {
    let path = source_path_for(profiles_dir, name);
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// Return sorted names of all profiles in `profiles_dir` that have a non-empty `.url` sidecar.
pub fn sourced_profiles(profiles_dir: &Path) -> std::io::Result<Vec<String>> {
    let entries = match fs::read_dir(profiles_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };

    let mut names = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        let is_url = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext == "url");
        if !is_url {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if validate_name(stem).is_err() {
            continue;
        }
        if let Ok(Some(_)) = read_source(profiles_dir, stem) {
            names.push(stem.to_string());
        }
    }
    names.sort();
    Ok(names)
}
