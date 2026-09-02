use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::exec::{run_program, StepExecutionStatus};
use crate::parser::parse_claude_json;

/// Opciones para ejecutar un paso de OpenCode.
#[derive(Debug, Clone)]
pub struct OpencodeOptions {
    pub output_format: Option<String>,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub agent: Option<String>,
    pub timeout: Duration,
    pub stream: bool,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
}

/// Cliente simple para invocar `opencode run`.
#[derive(Debug, Clone)]
pub struct OpencodeClient {
    program: String,
}

impl Default for OpencodeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl OpencodeClient {
    pub fn new() -> Self {
        Self::with_program("opencode")
    }

    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }

    pub async fn run(&self, prompt: &str, opts: &OpencodeOptions) -> StepExecutionStatus {
        let mut args = vec!["run".to_string()];

        if let Some(format) = &opts.output_format {
            args.push("--format".to_string());
            args.push(format.clone());
        }
        if let Some(session_id) = &opts.session_id {
            args.push("--session".to_string());
            args.push(session_id.clone());
        }
        if let Some(model) = &opts.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }
        if let Some(agent) = &opts.agent {
            args.push("--agent".to_string());
            args.push(agent.clone());
        }

        args.push(prompt.to_string());

        run_program(
            &self.program,
            &args,
            opts.cwd.as_deref(),
            &opts.env,
            opts.timeout,
            opts.stream,
        )
        .await
    }
}

/// Extrae el `session_id` del output de opencode. Con `--format json` el output
/// es NDJSON, así que se busca en cada línea (y en un bloque JSON suelto como
/// fallback) un campo `sessionID`/`session_id`/`sessionId`.
pub fn extract_session_id(output: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            for key in ["sessionID", "session_id", "sessionId"] {
                if let Some(id) = v.get(key).and_then(|v| v.as_str()) {
                    return Some(id.to_string());
                }
            }
        }
    }

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

    fn write_fake_opencode(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-opencode");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[tokio::test]
    async fn corre_opencode_y_extrae_session_id() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = write_fake_opencode(
            tmp.path(),
            "echo '{\"sessionID\": \"sess-123\", \"type\": \"message\"}'",
        );
        let client = OpencodeClient::with_program(fake.to_str().unwrap());
        let opts = OpencodeOptions {
            output_format: Some("json".into()),
            session_id: None,
            model: None,
            agent: None,
            timeout: Duration::from_secs(30),
            stream: false,
            cwd: None,
            env: HashMap::new(),
        };

        let status = client.run("hola", &opts).await;
        assert!(status.is_success());
        assert_eq!(extract_session_id(status.stdout()), Some("sess-123".into()));
    }

    #[tokio::test]
    async fn pasa_modelo_agente_y_sesion() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = write_fake_opencode(tmp.path(), "echo \"ARGS: $@\"");
        let client = OpencodeClient::with_program(fake.to_str().unwrap());
        let opts = OpencodeOptions {
            output_format: None,
            session_id: Some("sess-abc".into()),
            model: Some("anthropic/claude-sonnet-4".into()),
            agent: Some("build".into()),
            timeout: Duration::from_secs(30),
            stream: false,
            cwd: None,
            env: HashMap::new(),
        };

        let status = client.run("pregunta", &opts).await;
        assert!(status.is_success());
        let out = status.stdout();
        assert!(out.contains("--model anthropic/claude-sonnet-4"));
        assert!(out.contains("--agent build"));
        assert!(out.contains("--session sess-abc"));
    }
}
