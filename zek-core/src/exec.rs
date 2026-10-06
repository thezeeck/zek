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
        .stdin(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    // En Windows se asigna el job antes de permitir que el hijo cree procesos.
    #[cfg(windows)]
    cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);

    #[cfg(windows)]
    let process_tree = match process_tree::ProcessTree::new() {
        Ok(tree) => tree,
        Err(e) => {
            return StepExecutionStatus::Failed {
                stdout: String::new(),
                stderr: crate::t!(exec_spawn_failed, e),
                exit_code: -1,
            };
        }
    };

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

    #[cfg(unix)]
    let process_tree = process_tree::ProcessTree::new(child.id().unwrap());
    #[cfg(windows)]
    if let Err(e) = process_tree.attach(&child) {
        let _ = child.kill().await;
        return StepExecutionStatus::Failed {
            stdout: String::new(),
            stderr: crate::t!(exec_spawn_failed, e),
            exit_code: -1,
        };
    }

    let stdout_pipe = child.stdout.take().unwrap();
    let stderr_pipe = child.stderr.take().unwrap();

    let stdout_buf = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf = Arc::new(Mutex::new(Vec::new()));

    // La espera y ambos lectores comparten el mismo plazo. Al cancelar esta
    // future se cierran los pipes; no quedan tareas de lectura desprendidas.
    let execution = async {
        let (status, (), ()) = tokio::join!(
            child.wait(),
            read_and_stream(stdout_pipe, stream, false, stdout_buf.clone()),
            read_and_stream(stderr_pipe, stream, true, stderr_buf.clone()),
        );
        status
    };

    match timeout(timeout_dur, execution).await {
        Err(_) => {
            #[cfg(any(unix, windows))]
            drop(process_tree);
            let _ = child.kill().await;
            StepExecutionStatus::TimedOut {
                stdout: drain(&stdout_buf).await,
                stderr: drain(&stderr_buf).await,
            }
        }
        Ok(Err(e)) => StepExecutionStatus::Failed {
            stdout: drain(&stdout_buf).await,
            stderr: crate::t!(exec_wait_failed, e),
            exit_code: -1,
        },
        Ok(Ok(status)) => {
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

// El guard también termina los descendientes si se cancela la future desde
// fuera (por ejemplo, con --timeout-global), incluso si el shell ya salió.
#[cfg(unix)]
mod process_tree {
    pub(super) struct ProcessTree(libc::pid_t);

    impl ProcessTree {
        pub(super) fn new(pid: u32) -> Self {
            Self(pid as libc::pid_t)
        }
    }

    impl Drop for ProcessTree {
        fn drop(&mut self) {
            // SAFETY: el hijo es líder de un grupo propio (process_group(0));
            // el PID negativo señala ese grupo, nunca el grupo de zek.
            unsafe { libc::kill(-self.0, libc::SIGKILL) };
        }
    }
}

#[cfg(windows)]
mod process_tree {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    pub(super) struct ProcessTree(OwnedHandle);

    impl ProcessTree {
        pub(super) fn new() -> io::Result<Self> {
            // SAFETY: punteros nulos crean un job sin nombre ni atributos extra.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: CreateJobObjectW devuelve un handle válido y propio.
            let job = Self(unsafe { OwnedHandle::from_raw_handle(handle) });
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: estructura, tamaño y clase de información coinciden.
            let ok = unsafe {
                SetInformationJobObject(
                    job.0.as_raw_handle() as HANDLE,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const _,
                    std::mem::size_of_val(&limits) as u32,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }

        pub(super) fn attach(&self, child: &tokio::process::Child) -> io::Result<()> {
            let handle = child.raw_handle().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "child process handle unavailable")
            })?;
            // SAFETY: ambos handles siguen vivos durante la llamada.
            if unsafe {
                AssignProcessToJobObject(self.0.as_raw_handle() as HANDLE, handle as HANDLE)
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            resume_child(child.id().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "child process ID unavailable")
            })?)
        }
    }

    fn resume_child(pid: u32) -> io::Result<()> {
        // SAFETY: la API crea un snapshot propio de los threads del sistema.
        let handle = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: el snapshot válido pertenece exclusivamente a este guard.
        let snapshot = unsafe { OwnedHandle::from_raw_handle(handle) };
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        // SAFETY: snapshot válido y estructura con tamaño inicializado.
        let mut found = unsafe { Thread32First(snapshot.as_raw_handle() as HANDLE, &mut entry) };
        while found != 0 {
            if entry.th32OwnerProcessID == pid {
                // El proceso sigue suspendido: todavía solo tiene su thread inicial.
                // SAFETY: abrimos el thread identificado con permiso mínimo.
                let handle = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if handle.is_null() {
                    return Err(io::Error::last_os_error());
                }
                // SAFETY: OpenThread devolvió un handle válido y propio.
                let thread = unsafe { OwnedHandle::from_raw_handle(handle) };
                // SAFETY: el thread pertenece al hijo suspendido ya asignado al job.
                if unsafe { ResumeThread(thread.as_raw_handle() as HANDLE) } == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
            // SAFETY: el snapshot y la estructura siguen vivos.
            found = unsafe { Thread32Next(snapshot.as_raw_handle() as HANDLE, &mut entry) };
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "suspended child thread unavailable",
        ))
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
