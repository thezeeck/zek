#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

/// Workspace temporal con `commands/` y `flows/`.
pub struct Workspace {
    _tmp: tempfile::TempDir,
    pub workdir: PathBuf,
}

impl Workspace {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().expect("crear tempdir");
        let workdir = tmp.path().join("proyecto");
        fs::create_dir_all(workdir.join("commands")).expect("crear commands");
        fs::create_dir_all(workdir.join("flows")).expect("crear flows");
        Self { _tmp: tmp, workdir }
    }

    pub fn commands_dir(&self) -> PathBuf {
        self.workdir.join("commands")
    }

    pub fn flows_dir(&self) -> PathBuf {
        self.workdir.join("flows")
    }

    pub fn write_command(&self, name: &str, content: &str) {
        fs::write(self.commands_dir().join(format!("{name}.yaml")), content).unwrap();
    }

    pub fn write_flow(&self, name: &str, content: &str) {
        fs::write(self.flows_dir().join(format!("{name}.yaml")), content).unwrap();
    }
}

/// Crea un binario `claude` falso que ejecuta `body` como script de shell.
#[cfg(unix)]
pub fn write_fake_claude(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-claude");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

/// Crea un binario `opencode` falso que ejecuta `body` como script de shell.
#[cfg(unix)]
pub fn write_fake_opencode(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-opencode");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

/// Ruta a un fixture YAML de `tests/fixtures/`.
pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}
