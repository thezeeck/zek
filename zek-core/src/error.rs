use std::path::{Path, PathBuf};

/// Error unificado de `zek`.
#[derive(Debug, thiserror::Error)]
pub enum ZekError {
    #[error("no se pudo determinar el directorio home del usuario")]
    NoHomeDir,

    #[error("no se encontró la configuración: {0}")]
    ConfigNotFound(PathBuf),

    #[error("error de I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("error de YAML en {path}:{line}: {source}")]
    Yaml {
        path: PathBuf,
        line: usize,
        #[source]
        source: serde_yaml::Error,
    },

    #[error("configuración inválida: {0}")]
    InvalidConfig(String),

    #[error("error de template: {0}")]
    Template(String),

    #[error("{location}: {message}")]
    Validation { location: String, message: String },

    #[error("el directorio de trabajo no existe: {0}")]
    WorkdirNotFound(PathBuf),

    #[error("el directorio de trabajo no es un directorio: {0}")]
    WorkdirNotDir(PathBuf),

    #[error("falta la carpeta '{dir}' en el workdir: {workdir}")]
    MissingDir { dir: &'static str, workdir: PathBuf },
}

/// Estado final de un flujo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowFinalStatus {
    Success,
    Failed,
    Aborted,
}

impl FlowFinalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Aborted => "aborted",
        }
    }
}

impl ZekError {
    /// Error de validación con formato `archivo:línea: mensaje`.
    pub fn validation(path: &Path, line: usize, message: impl Into<String>) -> Self {
        let location = if line > 0 {
            format!("{}:{line}", path.display())
        } else {
            path.display().to_string()
        };
        ZekError::Validation {
            location,
            message: message.into(),
        }
    }
}
