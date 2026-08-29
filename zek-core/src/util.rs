use std::path::{Path, PathBuf};

use crate::error::ZekError;

/// Lista los archivos `.yaml`/`.yml` de un directorio, ordenados.
pub fn yaml_files(dir: &Path) -> Result<Vec<PathBuf>, ZekError> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("yaml") | Some("yml")
            )
        })
        .collect();
    files.sort();
    Ok(files)
}
