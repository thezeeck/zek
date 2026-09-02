use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ZekError;
use crate::step::DEFAULT_TIMEOUT;
use crate::util::yaml_files;

fn default_timeout() -> u32 {
    DEFAULT_TIMEOUT
}

/// Comando reutilizable definido en `commands/*.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub name: String,

    #[serde(default)]
    pub description: String,

    /// Comando a ejecutar (obligatorio).
    pub run: String,

    #[serde(default)]
    pub cwd: Option<String>,

    #[serde(default = "default_timeout")]
    pub timeout: u32,

    #[serde(default)]
    pub author: Option<String>,

    /// Variables de entorno extra para el comando (soporta `$VAR`/`${VAR}` y placeholders).
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Un comando cargado junto con el archivo del que proviene.
#[derive(Debug, Clone)]
pub struct LoadedCommand {
    pub command: Command,
    pub source: PathBuf,
}

impl Command {
    pub fn from_str(content: &str, path: &Path) -> Result<Self, ZekError> {
        serde_yaml::from_str(content).map_err(|e| {
            let line = e.location().map(|l| l.line()).unwrap_or(0);
            ZekError::Yaml {
                path: path.to_path_buf(),
                line,
                source: e,
            }
        })
    }

    pub fn load_from(path: &Path) -> Result<Self, ZekError> {
        let content = fs::read_to_string(path)?;
        Self::from_str(&content, path)
    }
}

/// Carga todos los comandos de `dir`, indexados por nombre.
pub fn load_all(dir: &Path) -> Result<HashMap<String, LoadedCommand>, ZekError> {
    let mut commands = HashMap::new();
    for path in yaml_files(dir)? {
        let command = Command::load_from(&path)?;
        if commands.contains_key(&command.name) {
            return Err(ZekError::validation(
                &path,
                0,
                crate::t!(val_command_duplicate, command.name),
            ));
        }
        commands.insert(
            command.name.clone(),
            LoadedCommand {
                command,
                source: path,
            },
        );
    }
    Ok(commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_commands(dir: &Path, files: &[(&str, &str)]) {
        fs::create_dir_all(dir).unwrap();
        for (name, content) in files {
            fs::write(dir.join(name), content).unwrap();
        }
    }

    #[test]
    fn from_str_parsea_y_defaults() {
        let path = PathBuf::from("x.yaml");
        let cmd = Command::from_str("name: build\nrun: cargo build\n", &path).unwrap();
        assert_eq!(cmd.name, "build");
        assert_eq!(cmd.run, "cargo build");
        assert_eq!(cmd.description, "");
        assert_eq!(cmd.cwd, None);
        assert_eq!(cmd.timeout, DEFAULT_TIMEOUT);
        assert_eq!(cmd.author, None);
    }

    #[test]
    fn load_all_indexa_por_nombre() {
        let tmp = tempfile::tempdir().unwrap();
        write_commands(
            tmp.path(),
            &[
                ("build.yaml", "name: build\nrun: cargo build\n"),
                ("test.yaml", "name: test\nrun: cargo test\n"),
                ("ignorado.txt", "esto no es yaml"),
            ],
        );
        let map = load_all(tmp.path()).unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("build"));
        assert!(map["test"].source.ends_with("test.yaml"));
    }

    #[test]
    fn load_all_rechaza_nombres_duplicados() {
        let tmp = tempfile::tempdir().unwrap();
        write_commands(
            tmp.path(),
            &[
                ("a.yaml", "name: build\nrun: cargo build\n"),
                ("b.yaml", "name: build\nrun: cargo build --release\n"),
            ],
        );
        assert!(load_all(tmp.path()).is_err());
    }
}
