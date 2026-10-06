use std::fs;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};

use crate::error::ZekError;
use crate::lang::Language;

pub const CONFIG_FILENAME: &str = "config.yaml";
pub const PROJECT_CONFIG_FILENAME: &str = "zek.yaml";
pub const COMMANDS_DIR: &str = "commands";
pub const FLOWS_DIR: &str = "flows";

/// Configuración global de `zek`, guardada en `config_dir()/config.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub workdir: PathBuf,

    #[serde(default)]
    pub history: crate::history::HistoryConfig,

    /// Idioma de los mensajes (`en`/`es`). Por defecto `en`.
    #[serde(default)]
    pub language: Language,
}

/// Config de proyecto (`zek.yaml`), opcional. Solapa la config global.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    #[serde(default)]
    pub workdir: Option<PathBuf>,

    #[serde(default)]
    pub history: Option<crate::history::HistoryConfig>,

    #[serde(default)]
    pub language: Option<Language>,
}

impl ProjectConfig {
    /// Lee y deserializa una config de proyecto desde `path`.
    pub fn load_from(path: &Path) -> Result<Self, ZekError> {
        let content = fs::read_to_string(path)?;
        serde_yaml::from_str(&content).map_err(|e| {
            let line = e.location().map(|l| l.line()).unwrap_or(0);
            ZekError::Yaml {
                path: path.to_path_buf(),
                line,
                source: e,
            }
        })
    }
}

impl Config {
    pub fn new(workdir: PathBuf) -> Self {
        Self {
            workdir,
            history: crate::history::HistoryConfig::default(),
            language: Language::default(),
        }
    }

    /// Aplica una config de proyecto sobre esta config. Los `workdir` relativos
    /// se resuelven contra `base_dir` (la carpeta que contiene el `zek.yaml`).
    pub fn apply_project(&mut self, project: ProjectConfig, base_dir: &Path) {
        if let Some(workdir) = project.workdir {
            self.workdir = if workdir.is_absolute() {
                workdir
            } else {
                base_dir.join(workdir)
            };
        }
        if let Some(mut history) = project.history {
            if let Some(directory) = &mut history.directory {
                if directory.is_relative() {
                    *directory = base_dir.join(&*directory);
                }
            }
            self.history = history;
        }
        if let Some(language) = project.language {
            self.language = language;
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

    pub fn history_store(&self) -> Result<crate::history::HistoryStore, ZekError> {
        let directory = match &self.history.directory {
            Some(directory) if directory.is_absolute() => directory.clone(),
            Some(directory) => self.workdir.join(directory),
            None => config_dir()?.join("history"),
        };
        Ok(crate::history::HistoryStore::new(
            directory,
            self.history.retention_days,
            self.history.max_runs,
        ))
    }

    pub fn commands_dir(&self) -> PathBuf {
        self.workdir.join(COMMANDS_DIR)
    }

    pub fn flows_dir(&self) -> PathBuf {
        self.workdir.join(FLOWS_DIR)
    }
}

/// Busca un `zek.yaml` desde el directorio actual hacia arriba.
/// Devuelve la ruta del primero que encuentre (o `None`).
pub fn find_project_config() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    find_project_config_from(&cwd)
}

/// Carga la config global y le aplica la config de proyecto (`zek.yaml`) si existe.
pub fn load_effective() -> Result<Config, ZekError> {
    let mut config = Config::load()?;
    if let Some(path) = find_project_config() {
        let project = ProjectConfig::load_from(&path)?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        config.apply_project(project, base);
    }
    Ok(config)
}

fn find_project_config_from(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join(PROJECT_CONFIG_FILENAME))
        .find(|candidate| candidate.is_file())
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

    #[test]
    fn apply_project_resuelve_workdir_relativo() {
        let mut config = Config::new(PathBuf::from("/global"));
        config.language = Language::Es;
        let project = ProjectConfig {
            history: None,
            workdir: Some(PathBuf::from(".")),
            language: None,
        };
        config.apply_project(project, Path::new("/repo"));

        assert_eq!(config.workdir, PathBuf::from("/repo"));
        assert_eq!(config.language, Language::Es);
    }

    #[test]
    fn apply_project_respeta_workdir_absoluto_y_language() {
        let mut config = Config::new(PathBuf::from("/global"));
        let project = ProjectConfig {
            history: None,
            workdir: Some(PathBuf::from("/absoluto")),
            language: Some(Language::Es),
        };
        config.apply_project(project, Path::new("/repo"));

        assert_eq!(config.workdir, PathBuf::from("/absoluto"));
        assert_eq!(config.language, Language::Es);
    }

    #[test]
    fn project_config_parsea() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("zek.yaml");
        fs::write(&path, "workdir: .\nlanguage: es\n").unwrap();

        let project = ProjectConfig::load_from(&path).unwrap();
        assert_eq!(project.workdir, Some(PathBuf::from(".")));
        assert_eq!(project.language, Some(Language::Es));
    }

    #[test]
    fn find_project_config_desde_subdirectorio() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let sub = repo.join("a").join("b");
        fs::create_dir_all(&sub).unwrap();
        fs::write(repo.join("zek.yaml"), "workdir: .\n").unwrap();

        let found = find_project_config_from(&sub).unwrap();
        assert_eq!(found, repo.join("zek.yaml"));
    }

    #[test]
    fn find_project_config_devuelve_none_sin_archivo() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(find_project_config_from(tmp.path()).is_none());
    }
}
