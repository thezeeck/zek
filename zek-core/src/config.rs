use std::fs;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};

use crate::error::ZekError;
use crate::lang::Language;

pub const CONFIG_FILENAME: &str = "config.yaml";
pub const COMMANDS_DIR: &str = "commands";
pub const FLOWS_DIR: &str = "flows";

/// Configuración global de `zek`, guardada en `config_dir()/config.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub workdir: PathBuf,

    /// Idioma de los mensajes (`en`/`es`). Por defecto `en`.
    #[serde(default)]
    pub language: Language,
}

impl Config {
    pub fn new(workdir: PathBuf) -> Self {
        Self {
            workdir,
            language: Language::default(),
        }
    }

    /// Ruta absoluta del archivo `config.yaml` (independiente del workdir).
    pub fn config_path() -> Result<PathBuf, ZekError> {
        Ok(config_dir()?.join(CONFIG_FILENAME))
    }

    /// Carga la config desde la ubicación por defecto.
    pub fn load() -> Result<Self, ZekError> {
        let path = Self::config_path()?;
        Self::load_from(&path)
    }

    /// Lee, deserializa y valida la config desde `path`.
    pub fn load_from(path: &Path) -> Result<Self, ZekError> {
        let content = fs::read_to_string(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ZekError::ConfigNotFound(path.to_path_buf()),
            _ => ZekError::Io(e),
        })?;
        let config = Self::from_str(&content, path)?;
        config.validate()?;
        Ok(config)
    }

    /// Solo deserializa el YAML (sin validar el sistema de archivos).
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

    /// Serializa y escribe la config en la ubicación por defecto.
    pub fn save(&self) -> Result<(), ZekError> {
        let path = Self::config_path()?;
        self.save_to(&path)
    }

    /// Serializa y escribe la config en `path` (creando directorios padre).
    pub fn save_to(&self, path: &Path) -> Result<(), ZekError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let content =
            serde_yaml::to_string(self).map_err(|e| ZekError::InvalidConfig(e.to_string()))?;
        fs::write(path, content)?;
        Ok(())
    }

    /// Valida que el workdir exista y contenga `commands/` y `flows/`.
    pub fn validate(&self) -> Result<(), ZekError> {
        if !self.workdir.exists() {
            return Err(ZekError::WorkdirNotFound(self.workdir.clone()));
        }
        if !self.workdir.is_dir() {
            return Err(ZekError::WorkdirNotDir(self.workdir.clone()));
        }
        for dir in [COMMANDS_DIR, FLOWS_DIR] {
            if !self.workdir.join(dir).is_dir() {
                return Err(ZekError::MissingDir {
                    dir,
                    workdir: self.workdir.clone(),
                });
            }
        }
        Ok(())
    }

    pub fn commands_dir(&self) -> PathBuf {
        self.workdir.join(COMMANDS_DIR)
    }

    pub fn flows_dir(&self) -> PathBuf {
        self.workdir.join(FLOWS_DIR)
    }
}

/// Carpeta de config de `zek`:
/// - Windows: `%APPDATA%\zek`
/// - Linux/Mac: `$XDG_CONFIG_HOME/zek` o `~/.config/zek`
pub fn config_dir() -> Result<PathBuf, ZekError> {
    if cfg!(target_os = "windows") {
        let base = BaseDirs::new()
            .map(|d| d.config_dir().to_path_buf())
            .ok_or(ZekError::NoHomeDir)?;
        return Ok(base.join("zek"));
    }

    let home = BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .ok_or(ZekError::NoHomeDir)?;
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    Ok(config_dir_from(xdg, home))
}

/// Lógica pura de resolución: `XDG_CONFIG_HOME` si existe, si no `~/.config`.
fn config_dir_from(xdg: Option<PathBuf>, home: PathBuf) -> PathBuf {
    xdg.unwrap_or_else(|| home.join(".config")).join("zek")
}

pub fn home_dir() -> Result<PathBuf, ZekError> {
    BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .ok_or(ZekError::NoHomeDir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workdir_with_subdirs() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("crear tempdir");
        let workdir = tmp.path().join("proyecto");
        fs::create_dir_all(workdir.join(COMMANDS_DIR)).unwrap();
        fs::create_dir_all(workdir.join(FLOWS_DIR)).unwrap();
        (tmp, workdir)
    }

    #[test]
    fn from_str_parsea_yaml_valido() {
        let path = PathBuf::from("/irrelevante/config.yaml");
        let config = Config::from_str("workdir: /alguna/ruta\n", &path).unwrap();
        assert_eq!(config.workdir, PathBuf::from("/alguna/ruta"));
        assert_eq!(config.language, Language::En);
    }

    #[test]
    fn from_str_parsea_idioma() {
        let path = PathBuf::from("/irrelevante/config.yaml");
        let config = Config::from_str("workdir: /x\nlanguage: es\n", &path).unwrap();
        assert_eq!(config.language, Language::Es);
    }

    #[test]
    fn from_str_error_yaml_con_linea() {
        let path = PathBuf::from("config.yaml");
        let err = Config::from_str("workdir: [sin cerrar\n", &path).unwrap_err();
        match err {
            ZekError::Yaml { line, .. } => assert!(line >= 1),
            other => panic!("se esperaba ZekError::Yaml, se obtuvo {other:?}"),
        }
    }

    #[test]
    fn from_str_error_tipo_incorrecto() {
        let path = PathBuf::from("config.yaml");
        let err = Config::from_str("workdir:\n  - a\n  - b\n", &path).unwrap_err();
        assert!(matches!(err, ZekError::Yaml { .. }));
    }

    #[test]
    fn load_from_no_encontrado() {
        let missing = PathBuf::from("/no/existe/config.yaml");
        let err = Config::load_from(&missing).unwrap_err();
        assert!(matches!(err, ZekError::ConfigNotFound(_)));
    }

    #[test]
    fn validate_rechaza_workdir_inexistente() {
        let config = Config::new(PathBuf::from("/no/existe/workdir"));
        assert!(matches!(
            config.validate(),
            Err(ZekError::WorkdirNotFound(_))
        ));
    }

    #[test]
    fn validate_rechaza_workdir_que_no_es_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let archivo = tmp.path().join("archivo.txt");
        fs::write(&archivo, "x").unwrap();

        let config = Config::new(archivo);
        assert!(matches!(config.validate(), Err(ZekError::WorkdirNotDir(_))));
    }

    #[test]
    fn validate_detecta_carpeta_faltante() {
        let tmp = tempfile::tempdir().unwrap();
        let workdir = tmp.path().join("proyecto");
        fs::create_dir_all(workdir.join(COMMANDS_DIR)).unwrap();
        // sin flows/

        let config = Config::new(workdir);
        match config.validate() {
            Err(ZekError::MissingDir { dir, .. }) => assert_eq!(dir, FLOWS_DIR),
            other => panic!("se esperaba MissingDir(flows), se obtuvo {other:?}"),
        }
    }

    #[test]
    fn save_to_y_load_from_roundtrip() {
        let (_tmp, workdir) = workdir_with_subdirs();
        let config = Config::new(workdir.clone());

        let path = workdir.join("config.yaml");
        config.save_to(&path).unwrap();

        let cargado = Config::load_from(&path).unwrap();
        assert_eq!(cargado, config);
    }

    #[test]
    fn save_to_crea_directorios_padre() {
        let tmp = tempfile::tempdir().unwrap();
        let (_tmp2, workdir) = workdir_with_subdirs();
        let config = Config::new(workdir);

        let path = tmp.path().join("a").join("b").join("config.yaml");
        config.save_to(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn config_dir_respeta_xdg_config_home() {
        let xdg = PathBuf::from("/mi/xdg");
        let home = PathBuf::from("/mi/home");
        assert_eq!(
            config_dir_from(Some(xdg), home),
            PathBuf::from("/mi/xdg/zek")
        );
    }

    #[test]
    fn config_dir_cae_en_home_config_sin_xdg() {
        let home = PathBuf::from("/mi/home");
        assert_eq!(
            config_dir_from(None, home),
            PathBuf::from("/mi/home/.config/zek")
        );
    }

    #[test]
    fn config_path_termina_en_config_yaml() {
        let path = Config::config_path().unwrap();
        assert_eq!(path.file_name().unwrap(), "config.yaml");
    }
}
