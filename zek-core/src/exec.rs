use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::timeout;

/// Resultado de ejecutar un comando.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepExecutionStatus {
    Success {
        stdout: String,
        stderr: String,
        exit_code: i32,
    },
    Failed {
        stdout: String,
        stderr: String,
        exit_code: i32,
    },
    TimedOut {
        stdout: String,
        stderr: String,
    },
}

impl StepExecutionStatus {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    pub fn is_timed_out(&self) -> bool {
        matches!(self, Self::TimedOut { .. })
    }

    pub fn stdout(&self) -> &str {
        match self {
            Self::Success { stdout, .. }
            | Self::Failed { stdout, .. }
            | Self::TimedOut { stdout, .. } => stdout,
        }
    }

    pub fn stderr(&self) -> &str {
        match self {
            Self::Success { stderr, .. }
            | Self::Failed { stderr, .. }
            | Self::TimedOut { stderr, .. } => stderr,
        }
    }

    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Self::Success { exit_code, .. } | Self::Failed { exit_code, .. } => Some(*exit_code),
            Self::TimedOut { .. } => None,
        }
    }

    pub fn status_str(&self) -> &'static str {
        match self {
            Self::Success { .. } => "success",
            Self::Failed { .. } => "failed",
            Self::TimedOut { .. } => "timedout",
        }
    }
}

/// Ejecuta un comando crudo vía shell, con streaming en vivo, captura de
/// outputs, timeout y variables de entorno.
#[derive(Debug, Clone)]
pub struct CommandExecutor {
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub timeout: Duration,
    pub env: HashMap<String, String>,
    /// Si es `true`, escribe stdout/stderr en la terminal mientras corre.
    pub stream: bool,
}

impl CommandExecutor {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            cwd: None,
            timeout: Duration::from_secs(300),
            env: HashMap::new(),
            stream: false,
        }
    }

    pub fn cwd(mut self, cwd: PathBuf) -> Self {
        self.cwd = Some(cwd);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn stream(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }

    pub async fn execute(&self) -> StepExecutionStatus {
        run_command(self.build_command(), self.timeout, self.stream).await
    }

    fn build_command(&self) -> Command {
        let mut cmd;
        #[cfg(windows)]
        {
            cmd = Command::new("cmd");
            cmd.arg("/C").arg(&self.command);
        }
        #[cfg(not(windows))]
        {
            cmd = Command::new("sh");
            cmd.arg("-c").arg(&self.command);
        }

        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        cmd
    }
}

/// Ejecuta un programa con argumentos directos (sin shell), con streaming,
/// captura de outputs y timeout.
pub async fn run_program(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    env: &HashMap<String, String>,
    timeout: Duration,
    stream: bool,
) -> StepExecutionStatus {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    for (key, value) in env {
        cmd.env(key, value);
    }
    run_command(cmd, timeout, stream).await
}

async fn run_command(cmd: Command, timeout_dur: Duration, stream: bool) -> StepExecutionStatus {
    let mut cmd = cmd;
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return StepExecutionStatus::Failed {
                stdout: String::new(),
                stderr: crate::t!(exec_spawn_failed, e),
                exit_code: -1,
            };
        }
    };

    let stdout_pipe = child.stdout.take().unwrap();
    let stderr_pipe = child.stderr.take().unwrap();

    let stdout_buf = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf = Arc::new(Mutex::new(Vec::new()));

    let stdout_task = tokio::spawn(read_and_stream(
        stdout_pipe,
        stream,
        false,
        stdout_buf.clone(),
    ));
    let stderr_task = tokio::spawn(read_and_stream(
        stderr_pipe,
        stream,
        true,
        stderr_buf.clone(),
    ));

    match timeout(timeout_dur, child.wait()).await {
        // Timeout: matamos el proceso y devolvemos lo capturado hasta ahora.
        Err(_) => {
            let _ = child.kill().await;
            StepExecutionStatus::TimedOut {
                stdout: drain(&stdout_buf).await,
                stderr: drain(&stderr_buf).await,
            }
        }
        Ok(Err(e)) => {
            let _ = tokio::join!(stdout_task, stderr_task);
            StepExecutionStatus::Failed {
                stdout: drain(&stdout_buf).await,
                stderr: crate::t!(exec_wait_failed, e),
                exit_code: -1,
            }
        }
        Ok(Ok(status)) => {
            let _ = tokio::join!(stdout_task, stderr_task);
            let stdout = drain(&stdout_buf).await;
            let stderr = drain(&stderr_buf).await;
            let exit_code = status.code().unwrap_or(-1);
            if status.success() {
                StepExecutionStatus::Success {
                    stdout,
                    stderr,
                    exit_code,
                }
            } else {
                StepExecutionStatus::Failed {
                    stdout,
                    stderr,
                    exit_code,
                }
            }
        }
    }
}

async fn drain(buf: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8_lossy(&buf.lock().await).into_owned()
}

async fn read_and_stream<R>(mut reader: R, stream: bool, red: bool, output: Arc<Mutex<Vec<u8>>>)
where
    R: AsyncRead + Unpin,
{
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let chunk = &buf[..n];
                if stream {
                    if red {
                        write_stderr(chunk);
                    } else {
                        write_stdout(chunk);
                    }
                }
                output.lock().await.extend_from_slice(chunk);
            }
        }
    }
}

fn write_stdout(bytes: &[u8]) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

fn write_stderr(bytes: &[u8]) {
    use std::io::Write;
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(b"\x1b[31m");
    let _ = err.write_all(bytes);
    let _ = err.write_all(b"\x1b[0m");
    let _ = err.flush();
}

/// Ejecuta el comando con reintentos. Devuelve (resultado final, intentos usados).
pub async fn execute_with_retries(
    executor: &CommandExecutor,
    retries: u32,
    retry_delay: Duration,
) -> (StepExecutionStatus, u32) {
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let status = executor.execute().await;
        if status.is_success() || attempts > retries {
            return (status, attempts);
        }
        if !retry_delay.is_zero() {
            tokio::time::sleep(retry_delay).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_helpers() {
        let ok = StepExecutionStatus::Success {
            stdout: "out".into(),
            stderr: "err".into(),
            exit_code: 0,
        };
        assert!(ok.is_success());
        assert!(!ok.is_failed());
        assert_eq!(ok.stdout(), "out");
        assert_eq!(ok.exit_code(), Some(0));

        let fail = StepExecutionStatus::Failed {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 7,
        };
        assert!(fail.is_failed());
        assert_eq!(fail.exit_code(), Some(7));

        let timeout = StepExecutionStatus::TimedOut {
            stdout: String::new(),
            stderr: String::new(),
        };
        assert!(timeout.is_timed_out());
        assert_eq!(timeout.exit_code(), None);
    }

    #[tokio::test]
    async fn ejecuta_comando_exitoso() {
        let status = CommandExecutor::new("echo hola").execute().await;
        assert!(status.is_success());
        assert_eq!(status.stdout().trim(), "hola");
        assert_eq!(status.exit_code(), Some(0));
    }

    #[tokio::test]
    async fn captura_exit_code_de_fallo() {
        let status = CommandExecutor::new("exit 3").execute().await;
        assert!(status.is_failed());
        assert_eq!(status.exit_code(), Some(3));
    }

    #[tokio::test]
    async fn captura_stderr() {
        let status = CommandExecutor::new("echo error >&2").execute().await;
        assert!(status.is_success());
        assert_eq!(status.stderr().trim(), "error");
        assert!(status.stdout().is_empty());
    }

    #[tokio::test]
    async fn aplica_timeout() {
        let executor = CommandExecutor::new("sleep 5").timeout(Duration::from_millis(200));
        let status = executor.execute().await;
        assert!(status.is_timed_out());
    }

    #[tokio::test]
    async fn sobrescribe_entorno() {
        let executor = CommandExecutor::new("echo $FOO").env("FOO", "bar");
        let status = executor.execute().await;
        assert!(status.is_success());
        assert_eq!(status.stdout().trim(), "bar");
    }

    #[tokio::test]
    async fn respeta_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let executor = CommandExecutor::new("pwd").cwd(tmp.path().to_path_buf());
        let status = executor.execute().await;
        assert!(status.is_success());
        // canonicalize por symlinks de macOS (/var -> /private/var)
        let expected = tmp.path().canonicalize().unwrap();
        assert_eq!(status.stdout().trim(), expected.to_str().unwrap());
    }

    #[tokio::test]
    async fn retries_hasta_exito() {
        let tmp = tempfile::tempdir().unwrap();
        let counter = tmp.path().join("count");
        let counter = counter.to_str().unwrap();
        // Falla la primera vez y tiene éxito a partir de la segunda.
        let script = format!(
            "count=$(cat {counter} 2>/dev/null || echo 0); count=$((count + 1)); echo $count > {counter}; [ $count -ge 2 ]"
        );

        let executor = CommandExecutor::new(script);
        let (status, attempts) = execute_with_retries(&executor, 3, Duration::from_millis(1)).await;
        assert!(status.is_success());
        assert_eq!(attempts, 2);
    }
    #[tokio::test]
    async fn retries_respeta_limite() {
        let executor = CommandExecutor::new("exit 1");
        let (status, attempts) = execute_with_retries(&executor, 2, Duration::from_millis(1)).await;
        assert!(status.is_failed());
        assert_eq!(attempts, 3);
    }

    #[tokio::test]
    async fn streaming_captura_igual() {
        let executor = CommandExecutor::new("echo hola && echo error >&2").stream(true);
        let status = executor.execute().await;
        assert!(status.is_success());
        assert_eq!(status.stdout().trim(), "hola");
        assert_eq!(status.stderr().trim(), "error");
    }
}
