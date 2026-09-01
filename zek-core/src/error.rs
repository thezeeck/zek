use std::fmt;
use std::path::{Path, PathBuf};

use crate::lang;

/// Error unificado de `zek`.
#[derive(Debug)]
pub enum ZekError {
    NoHomeDir,
    ConfigNotFound(PathBuf),
    Io(std::io::Error),
    Yaml {
        path: PathBuf,
        line: usize,
        source: serde_yaml::Error,
    },
    InvalidConfig(String),
    Template(String),
    Validation {
        location: String,
        message: String,
    },
    WorkdirNotFound(PathBuf),
    WorkdirNotDir(PathBuf),
    MissingDir {
        dir: &'static str,
        workdir: PathBuf,
    },
}

impl fmt::Display for ZekError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHomeDir => write!(f, "{}", lang::messages().err_no_home_dir),
            Self::ConfigNotFound(path) => {
                write!(f, "{}", crate::t!(err_config_not_found, path.display()))
            }
            Self::Io(e) => write!(f, "{}", crate::t!(err_io, e)),
            Self::Yaml { path, line, source } => {
                write!(f, "{}", crate::t!(err_yaml, path.display(), line, source))
            }
            Self::InvalidConfig(msg) => write!(f, "{}", crate::t!(err_invalid_config, msg)),
            Self::Template(msg) => write!(f, "{}", crate::t!(err_template, msg)),
            Self::Validation { location, message } => write!(f, "{location}: {message}"),
            Self::WorkdirNotFound(path) => {
                write!(f, "{}", crate::t!(err_workdir_not_found, path.display()))
            }
            Self::WorkdirNotDir(path) => {
                write!(f, "{}", crate::t!(err_workdir_not_dir, path.display()))
            }
            Self::MissingDir { dir, workdir } => {
                write!(f, "{}", crate::t!(err_missing_dir, dir, workdir.display()))
            }
        }
    }
}

impl std::error::Error for ZekError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Yaml { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ZekError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
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
