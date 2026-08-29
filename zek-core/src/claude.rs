use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::exec::{run_program, StepExecutionStatus};
use crate::parser::parse_claude_json;

/// Opciones para ejecutar un paso de Claude.
#[derive(Debug, Clone)]
pub struct ClaudeOptions {
    pub output_format: Option<String>,
    pub session_id: Option<String>,
    pub timeout: Duration,
    pub stream: bool,
    pub cwd: Option<PathBuf>,
}

/// Cliente simple para invocar `claude -p`.
#[derive(Debug, Clone)]
pub struct ClaudeClient {
    program: String,
}

impl Default for ClaudeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeClient {
    pub fn new() -> Self {
        Self::with_program("claude")
    }

    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }

    pub async fn run(&self, prompt: &str, opts: &ClaudeOptions) -> StepExecutionStatus {
        let mut args = vec!["-p".to_string(), prompt.to_string()];

        if let Some(format) = &opts.output_format {
            args.push("--output-format".to_string());
            args.push(format.clone());
        }
        if let Some(session_id) = &opts.session_id {
            args.push("--resume".to_string());
            args.push(session_id.clone());
        }

        run_program(
            &self.program,
            &args,
            opts.cwd.as_deref(),
            &HashMap::new(),
            opts.timeout,
            opts.stream,
        )
        .await
    }
}

/// Extrae el `session_id` del output JSON de claude (si lo hay).
pub fn extract_session_id(output: &str) -> Option<String> {
    parse_claude_json(output)
        .ok()
        .and_then(|v| v.get("session_id").cloned())
        .and_then(|v| v.as_str().map(String::from))
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn write_fake_claude(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-claude");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[tokio::test]
    async fn corre_claude_y_extrae_session_id() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = write_fake_claude(
            tmp.path(),
            "echo '{\"result\": \"hola\", \"session_id\": \"sess-123\"}'",
        );
        let client = ClaudeClient::with_program(fake.to_str().unwrap());
        let opts = ClaudeOptions {
            output_format: Some("json".into()),
            session_id: None,
            timeout: Duration::from_secs(30),
            stream: false,
            cwd: None,
        };

        let status = client.run("hola", &opts).await;
        assert!(status.is_success());
        assert_eq!(extract_session_id(status.stdout()), Some("sess-123".into()));
    }

    #[tokio::test]
    async fn pasa_resume_cuando_hay_session_id() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = write_fake_claude(tmp.path(), "echo \"ARGS: $@\"");
        let client = ClaudeClient::with_program(fake.to_str().unwrap());
        let opts = ClaudeOptions {
            output_format: None,
            session_id: Some("sess-abc".into()),
            timeout: Duration::from_secs(30),
            stream: false,
            cwd: None,
        };

        let status = client.run("pregunta", &opts).await;
        assert!(status.is_success());
        assert!(status.stdout().contains("--resume sess-abc"));
    }
}
