use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::claude::{extract_session_id, ClaudeClient, ClaudeOptions};
use crate::commands::LoadedCommand;
use crate::context::{ExecutionContext, SerializedStepResult};
use crate::error::{FlowFinalStatus, ZekError};
use crate::exec::{execute_with_retries, CommandExecutor, StepExecutionStatus};
use crate::flows::Flow;
use crate::opencode::{
    extract_session_id as extract_opencode_session_id, OpencodeClient, OpencodeOptions,
};
use crate::step::{OnErrorAction, OnSuccessAction, Step, StepType};

/// Máximo de saltos por defecto dentro de un bloque `finally`.
pub const DEFAULT_FINALLY_MAX_JUMPS: usize = 5;

/// Evento de progreso emitido durante la ejecución de un flujo.
#[derive(Debug, Clone)]
pub enum StepProgress {
    Started {
        name: String,
        index: usize,
        total: usize,
    },
    Finished {
        name: String,
        status: StepExecutionStatus,
    },
}

type ProgressCb = dyn Fn(StepProgress) + Send + Sync;
type ConfirmCb = dyn Fn(&str) -> bool + Send + Sync;

/// Resultado de ejecutar un flujo completo.
#[derive(Debug)]
pub struct FlowReport {
    pub name: String,
    pub status: FlowFinalStatus,
    pub exit_reason: String,
    pub failed_steps: Vec<String>,
    pub skipped_steps: Vec<String>,
    pub results: ExecutionContext,
    pub duration: Duration,
}

impl FlowReport {
    pub fn exit_code(&self) -> i32 {
        match self.status {
            FlowFinalStatus::Success => 0,
            FlowFinalStatus::Failed => 2,
            FlowFinalStatus::Aborted => 3,
        }
    }
}

/// Ejecuta un flujo: recorre los steps aplicando retries, `on_error`/`on_success`,
/// saltos `goto` (con protección contra loops) y el bloque `finally`.
pub struct FlowRunner<'a> {
    flow: &'a Flow,
    commands: &'a HashMap<String, LoadedCommand>,
    workdir: PathBuf,
    stream: bool,
    claude: ClaudeClient,
    opencode: OpencodeClient,
    claude_session: Mutex<Option<String>>,
    opencode_session: Mutex<Option<String>>,
    progress: Option<Arc<ProgressCb>>,
    confirm: Option<Arc<ConfirmCb>>,
}

impl<'a> FlowRunner<'a> {
    pub fn new(
        flow: &'a Flow,
        commands: &'a HashMap<String, LoadedCommand>,
        workdir: PathBuf,
        stream: bool,
    ) -> Self {
        Self::base(flow, commands, workdir, stream)
    }

    pub fn with_claude(
        flow: &'a Flow,
        commands: &'a HashMap<String, LoadedCommand>,
        workdir: PathBuf,
        stream: bool,
        claude_program: impl Into<String>,
    ) -> Self {
        let mut runner = Self::base(flow, commands, workdir, stream);
        runner.claude = ClaudeClient::with_program(claude_program);
        runner
    }

    pub fn with_opencode(
        flow: &'a Flow,
        commands: &'a HashMap<String, LoadedCommand>,
        workdir: PathBuf,
        stream: bool,
        opencode_program: impl Into<String>,
    ) -> Self {
        let mut runner = Self::base(flow, commands, workdir, stream);
        runner.opencode = OpencodeClient::with_program(opencode_program);
        runner
    }

    fn base(
        flow: &'a Flow,
        commands: &'a HashMap<String, LoadedCommand>,
        workdir: PathBuf,
        stream: bool,
    ) -> Self {
        Self {
            flow,
            commands,
            workdir,
            stream,
            claude: ClaudeClient::new(),
            opencode: OpencodeClient::new(),
            claude_session: Mutex::new(None),
            opencode_session: Mutex::new(None),
            progress: None,
            confirm: None,
        }
    }

    /// Registra un callback de progreso (aviso de inicio/fin de cada step).
    pub fn on_progress(mut self, cb: Arc<ProgressCb>) -> Self {
        self.progress = Some(cb);
        self
    }

    /// Registra un callback de confirmación para steps con `confirm: true`.
    pub fn on_confirm(mut self, cb: Arc<ConfirmCb>) -> Self {
        self.confirm = Some(cb);
        self
    }

    pub async fn run(&self) -> Result<FlowReport, ZekError> {
        let start = Instant::now();
        let mut ctx = ExecutionContext::new();
        let mut skipped = Vec::new();

        let (mut status, mut exit_reason) = self.run_main(&mut ctx, &mut skipped).await?;
        let failed_steps = ctx.failed_step_names();
        ctx.set_flow_result(status, exit_reason.clone());

        if self.run_finally(&mut ctx, &mut skipped).await? && status == FlowFinalStatus::Success {
            status = FlowFinalStatus::Failed;
            exit_reason = "finally".to_string();
        }

        Ok(FlowReport {
            name: self.flow.name.clone(),
            status,
            exit_reason,
            failed_steps,
            skipped_steps: skipped,
            results: ctx,
            duration: start.elapsed(),
        })
    }

    async fn run_main(
        &self,
        ctx: &mut ExecutionContext,
        skipped: &mut Vec<String>,
    ) -> Result<(FlowFinalStatus, String), ZekError> {
        let steps = &self.flow.steps;
        let total = steps.len();
        let max_jumps = self.flow.max_jumps;
        let mut idx = 0usize;
        let mut jumps = 0usize;

        let outcome = loop {
            if idx >= steps.len() {
                break MainOutcome::NaturalEnd;
            }
            let step = &steps[idx];

            if step.entry_only_via_goto && !ctx.was_targeted(&step.name) {
                skipped.push(step.name.clone());
                idx += 1;
                continue;
            }

            self.emit_started(&step.name, idx, total);

            if step.confirm && !self.ask_confirm(&step.name) {
                skipped.push(step.name.clone());
                idx += 1;
                continue;
            }

            let result = self.run_step(step, ctx).await?;
            self.emit_finished(&step.name, result.status.clone());
            let success = result.status.is_success();

            match next_action(step, success) {
                NextAction::Continue => idx += 1,
                NextAction::End => break MainOutcome::End(step.name.clone()),
                NextAction::Stop => break MainOutcome::Stop(step.name.clone()),
                NextAction::Goto(target) => {
                    jumps += 1;
                    if jumps > max_jumps {
                        break MainOutcome::Aborted;
                    }
                    match steps.iter().position(|s| s.name == target) {
                        Some(i) => {
                            ctx.mark_targeted(&target);
                            idx = i;
                        }
                        None => {
                            return Err(ZekError::InvalidConfig(format!(
                                "goto a step inexistente: '{target}'"
                            )));
                        }
                    }
                }
            }
        };

        let has_failed = !ctx.failed_step_names().is_empty();
        let (status, reason) = match outcome {
            MainOutcome::NaturalEnd => (
                if has_failed {
                    FlowFinalStatus::Failed
                } else {
                    FlowFinalStatus::Success
                },
                "natural_end".to_string(),
            ),
            MainOutcome::End(step) => (
                if has_failed {
                    FlowFinalStatus::Failed
                } else {
                    FlowFinalStatus::Success
                },
                format!("end:{step}"),
            ),
            MainOutcome::Stop(step) => (FlowFinalStatus::Failed, format!("stop:{step}")),
            MainOutcome::Aborted => (
                FlowFinalStatus::Aborted,
                format!("infinite_loop: se superaron {max_jumps} saltos"),
            ),
        };
        Ok((status, reason))
    }

    async fn run_finally(
        &self,
        ctx: &mut ExecutionContext,
        skipped: &mut Vec<String>,
    ) -> Result<bool, ZekError> {
        let Some(fin) = &self.flow.finally else {
            return Ok(false);
        };
        let steps = &fin.steps;
        if steps.is_empty() {
            return Ok(false);
        }

        let total = steps.len();
        let max_jumps = fin.max_jumps.unwrap_or(DEFAULT_FINALLY_MAX_JUMPS);
        let mut idx = 0usize;
        let mut jumps = 0usize;
        let mut failed = false;

        loop {
            if idx >= steps.len() {
                break;
            }
            let step = &steps[idx];

            if step.entry_only_via_goto && !ctx.was_targeted(&step.name) {
                skipped.push(step.name.clone());
                idx += 1;
                continue;
            }

            self.emit_started(&step.name, idx, total);

            if step.confirm && !self.ask_confirm(&step.name) {
                skipped.push(step.name.clone());
                idx += 1;
                continue;
            }

            let result = self.run_step(step, ctx).await?;
            self.emit_finished(&step.name, result.status.clone());
            if !result.status.is_success() {
                failed = true;
            }

            let action = if result.status.is_success() {
                match step.effective_on_success() {
                    OnSuccessAction::Continue => NextAction::Continue,
                    OnSuccessAction::End => NextAction::End,
                    OnSuccessAction::Goto(t) => NextAction::Goto(t),
                }
            } else {
                match step.effective_on_error(true) {
                    OnErrorAction::Continue => NextAction::Continue,
                    OnErrorAction::Stop => NextAction::End,
                    OnErrorAction::Goto(t) => NextAction::Goto(t),
                }
            };

            match action {
                NextAction::Continue => idx += 1,
                NextAction::End | NextAction::Stop => break,
                NextAction::Goto(target) => {
                    jumps += 1;
                    if jumps > max_jumps {
                        break;
                    }
                    match steps.iter().position(|s| s.name == target) {
                        Some(i) => {
                            ctx.mark_targeted(&target);
                            idx = i;
                        }
                        None => break,
                    }
                }
            }
        }

        Ok(failed && fin.fail_flow_on_error)
    }

    fn emit_started(&self, name: &str, index: usize, total: usize) {
        if let Some(cb) = &self.progress {
            cb(StepProgress::Started {
                name: name.to_string(),
                index,
                total,
            });
        }
    }

    fn emit_finished(&self, name: &str, status: StepExecutionStatus) {
        if let Some(cb) = &self.progress {
            cb(StepProgress::Finished {
                name: name.to_string(),
                status,
            });
        }
    }

    fn ask_confirm(&self, name: &str) -> bool {
        match &self.confirm {
            Some(cb) => cb(name),
            None => true,
        }
    }

    async fn run_step(
        &self,
        step: &Step,
        ctx: &mut ExecutionContext,
    ) -> Result<SerializedStepResult, ZekError> {
        let start = Instant::now();
        let (status, attempts) = match step.step_type {
            StepType::Command => self.run_command_step(step).await?,
            StepType::Claude => self.run_claude_step(step, ctx).await?,
            StepType::Opencode => self.run_opencode_step(step, ctx).await?,
        };
        let result = SerializedStepResult {
            status,
            duration: start.elapsed(),
            attempts,
        };
        ctx.record(step.name.clone(), result.clone());
        Ok(result)
    }

    async fn run_command_step(&self, step: &Step) -> Result<(StepExecutionStatus, u32), ZekError> {
        let resolved = resolve_command(step, self.commands).ok_or_else(|| {
            ZekError::InvalidConfig(format!(
                "el step '{}' de tipo command no tiene comando definido",
                step.name
            ))
        })?;
        let cwd = resolve_cwd(resolved.cwd.as_deref(), &self.workdir);

        let mut executor = CommandExecutor::new(resolved.run)
            .timeout(resolved.timeout)
            .stream(self.stream);
        if let Some(cwd) = cwd {
            executor = executor.cwd(cwd);
        }

        let (status, attempts) = execute_with_retries(
            &executor,
            step.retries,
            Duration::from_secs(step.retry_delay as u64),
        )
        .await;
        Ok((status, attempts))
    }

    async fn run_claude_step(
        &self,
        step: &Step,
        ctx: &ExecutionContext,
    ) -> Result<(StepExecutionStatus, u32), ZekError> {
        let prompt = step.prompt.as_deref().unwrap_or_default();
        let rendered = ctx.render(prompt)?;

        let session_id = if step.continue_session {
            self.claude_session.lock().unwrap().clone()
        } else {
            step.session_id.clone()
        };

        let opts = ClaudeOptions {
            output_format: step.output_format.clone(),
            session_id,
            timeout: Duration::from_secs(step.timeout as u64),
            stream: self.stream,
            cwd: Some(self.workdir.clone()),
        };
        let status = self.claude.run(&rendered, &opts).await;

        if let Some(sid) = extract_session_id(status.stdout()) {
            *self.claude_session.lock().unwrap() = Some(sid);
        }

        Ok((status, 1))
    }

    async fn run_opencode_step(
        &self,
        step: &Step,
        ctx: &ExecutionContext,
    ) -> Result<(StepExecutionStatus, u32), ZekError> {
        let prompt = step.prompt.as_deref().unwrap_or_default();
        let rendered = ctx.render(prompt)?;

        let session_id = if step.continue_session {
            self.opencode_session.lock().unwrap().clone()
        } else {
            step.session_id.clone()
        };

        let opts = OpencodeOptions {
            output_format: step.output_format.clone(),
            session_id,
            model: step.model.clone(),
            agent: step.agent.clone(),
            timeout: Duration::from_secs(step.timeout as u64),
            stream: self.stream,
            cwd: Some(self.workdir.clone()),
        };
        let status = self.opencode.run(&rendered, &opts).await;

        if let Some(sid) = extract_opencode_session_id(status.stdout()) {
            *self.opencode_session.lock().unwrap() = Some(sid);
        }

        Ok((status, 1))
    }
}

enum MainOutcome {
    NaturalEnd,
    End(String),
    Stop(String),
    Aborted,
}

enum NextAction {
    Continue,
    End,
    Stop,
    Goto(String),
}

fn next_action(step: &Step, success: bool) -> NextAction {
    if success {
        match step.effective_on_success() {
            OnSuccessAction::Continue => NextAction::Continue,
            OnSuccessAction::End => NextAction::End,
            OnSuccessAction::Goto(t) => NextAction::Goto(t),
        }
    } else {
        match step.effective_on_error(false) {
            OnErrorAction::Stop => NextAction::Stop,
            OnErrorAction::Continue => NextAction::Continue,
            OnErrorAction::Goto(t) => NextAction::Goto(t),
        }
    }
}

struct ResolvedCommand {
    run: String,
    cwd: Option<String>,
    timeout: Duration,
}

fn resolve_command(
    step: &Step,
    commands: &HashMap<String, LoadedCommand>,
) -> Option<ResolvedCommand> {
    let name = step.command.as_deref()?;
    if let Some(loaded) = commands.get(name) {
        let cmd = &loaded.command;
        Some(ResolvedCommand {
            run: cmd.run.clone(),
            cwd: cmd.cwd.clone(),
            timeout: Duration::from_secs(cmd.timeout as u64),
        })
    } else {
        Some(ResolvedCommand {
            run: name.to_string(),
            cwd: step.cwd.clone(),
            timeout: Duration::from_secs(step.timeout as u64),
        })
    }
}

fn resolve_cwd(cwd: Option<&str>, workdir: &Path) -> Option<PathBuf> {
    cwd.map(|c| {
        let path = PathBuf::from(c);
        if path.is_absolute() {
            path
        } else {
            workdir.join(path)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Command;
    use crate::flows::Flow;
    use std::path::Path;

    async fn run_flow(yaml: &str) -> FlowReport {
        run_flow_with_commands(yaml, HashMap::new()).await
    }

    async fn run_flow_with_commands(
        yaml: &str,
        commands: HashMap<String, LoadedCommand>,
    ) -> FlowReport {
        let flow = Flow::from_str(yaml, Path::new("test.yaml")).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let runner = FlowRunner::new(&flow, &commands, tmp.path().to_path_buf(), false);
        runner.run().await.unwrap()
    }

    #[tokio::test]
    async fn flujo_simple_exitoso() {
        let report = run_flow(
            "name: f\nsteps:\n  - name: a\n    type: command\n    command: echo hola\n  - name: b\n    type: command\n    command: echo chau\n",
        )
        .await;
        assert_eq!(report.status, FlowFinalStatus::Success);
        assert_eq!(report.exit_reason, "natural_end");
        assert!(report.failed_steps.is_empty());
        assert_eq!(
            report.results.get("a").unwrap().status.stdout().trim(),
            "hola"
        );
    }

    #[tokio::test]
    async fn stop_corta_el_flujo() {
        let report = run_flow(
            "name: f\nsteps:\n  - name: build\n    type: command\n    command: exit 1\n  - name: test\n    type: command\n    command: echo no-deberia-correr\n",
        )
        .await;
        assert_eq!(report.status, FlowFinalStatus::Failed);
        assert_eq!(report.exit_reason, "stop:build");
        assert_eq!(report.failed_steps, vec!["build"]);
        assert!(report.results.get("test").is_none());
    }

    #[tokio::test]
    async fn continue_avanza_con_fallo() {
        let report = run_flow(
            "name: f\nsteps:\n  - name: build\n    type: command\n    command: exit 1\n    on_error: continue\n  - name: test\n    type: command\n    command: echo ok\n",
        )
        .await;
        assert_eq!(report.status, FlowFinalStatus::Failed);
        assert_eq!(report.exit_reason, "natural_end");
        assert_eq!(report.failed_steps, vec!["build"]);
        assert!(report.results.get("test").unwrap().status.is_success());
    }

    #[tokio::test]
    async fn end_termina_el_flujo() {
        let report = run_flow(
            "name: f\nsteps:\n  - name: a\n    type: command\n    command: echo hola\n    on_success: end\n  - name: b\n    type: command\n    command: echo no\n",
        )
        .await;
        assert_eq!(report.status, FlowFinalStatus::Success);
        assert_eq!(report.exit_reason, "end:a");
        assert!(report.results.get("b").is_none());
    }

    #[tokio::test]
    async fn reintenta_hasta_exito() {
        let tmp = tempfile::tempdir().unwrap();
        let counter = tmp.path().join("count").to_str().unwrap().to_string();
        let script = format!(
            "count=$(cat {counter} 2>/dev/null || echo 0); count=$((count+1)); echo $count > {counter}; [ $count -ge 2 ]"
        );
        let yaml = format!(
            "name: f\nsteps:\n  - name: build\n    type: command\n    command: '{script}'\n    retries: 3\n"
        );
        let report = run_flow(&yaml).await;
        assert_eq!(report.status, FlowFinalStatus::Success);
        assert_eq!(report.results.get("build").unwrap().attempts, 2);
    }

    #[tokio::test]
    async fn entry_only_no_referenciado_se_salta() {
        let report = run_flow(
            "name: f\nsteps:\n  - name: a\n    type: command\n    command: echo hola\n  - name: handler\n    type: command\n    command: echo no\n    entry_only_via_goto: true\n",
        )
        .await;
        assert_eq!(report.status, FlowFinalStatus::Success);
        assert!(report.results.get("handler").is_none());
    }

    #[tokio::test]
    async fn goto_salta_a_otro_step() {
        let report = run_flow(
            "name: f\nsteps:\n  - name: a\n    type: command\n    command: echo hola\n    on_success: goto:c\n  - name: b\n    type: command\n    command: echo no\n  - name: c\n    type: command\n    command: echo adios\n",
        )
        .await;
        assert_eq!(report.status, FlowFinalStatus::Success);
        assert!(report.results.get("b").is_none());
        assert!(report.results.get("c").unwrap().status.is_success());
    }

    #[tokio::test]
    async fn loop_infinito_se_aborta() {
        let report = run_flow(
            "name: f\nmax_jumps: 3\nsteps:\n  - name: a\n    type: command\n    command: echo a\n    on_success: goto:b\n  - name: b\n    type: command\n    command: echo b\n    on_success: goto:a\n",
        )
        .await;
        assert_eq!(report.status, FlowFinalStatus::Aborted);
        assert!(report.exit_reason.contains("infinite_loop"));
    }

    #[tokio::test]
    async fn finally_corre_tras_stop() {
        let report = run_flow(
            "name: f\nsteps:\n  - name: build\n    type: command\n    command: exit 1\nfinally:\n  steps:\n    - name: cleanup\n      type: command\n      command: echo limpiando\n",
        )
        .await;
        assert_eq!(report.status, FlowFinalStatus::Failed);
        assert!(report.results.get("cleanup").unwrap().status.is_success());
    }

    #[tokio::test]
    async fn finally_falla_y_tumba_el_flujo() {
        let report = run_flow(
            "name: f\nsteps:\n  - name: a\n    type: command\n    command: echo hola\nfinally:\n  fail_flow_on_error: true\n  steps:\n    - name: cleanup\n      type: command\n      command: exit 1\n",
        )
        .await;
        assert_eq!(report.status, FlowFinalStatus::Failed);
        assert_eq!(report.exit_reason, "finally");
    }

    #[tokio::test]
    async fn resuelve_comando_nombrado() {
        let mut commands = HashMap::new();
        commands.insert(
            "build".to_string(),
            LoadedCommand {
                command: Command {
                    name: "build".into(),
                    description: String::new(),
                    run: "echo compilando".into(),
                    cwd: None,
                    timeout: 300,
                    author: None,
                },
                source: PathBuf::from("build.yaml"),
            },
        );
        let report = run_flow_with_commands(
            "name: f\nsteps:\n  - name: build\n    type: command\n    command: build\n",
            commands,
        )
        .await;
        let status = &report.results.get("build").unwrap().status;
        assert!(status.is_success());
        assert!(status.stdout().contains("compilando"));
    }

    #[cfg(unix)]
    fn write_fake_claude(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-claude");
        std::fs::write(
            &path,
            "#!/bin/sh\necho \"ARGS: $@\" >&2\necho '{\"result\": \"ok\", \"session_id\": \"sess-1\"}'\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[cfg(unix)]
    fn write_fake_opencode(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-opencode");
        std::fs::write(
            &path,
            "#!/bin/sh\necho \"ARGS: $@\" >&2\necho '{\"sessionID\": \"oc-sess-1\"}'\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn flujo_con_step_claude() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = write_fake_claude(tmp.path());

        let flow = Flow::from_str(
            "name: f\nsteps:\n  - name: build\n    type: command\n    command: echo hola\n  - name: resumen\n    type: claude\n    prompt: 'Resultado: {{steps.build.stdout}}'\n",
            Path::new("test.yaml"),
        )
        .unwrap();
        let commands = HashMap::new();
        let runner = FlowRunner::with_claude(
            &flow,
            &commands,
            tmp.path().to_path_buf(),
            false,
            fake.to_str().unwrap(),
        );
        let report = runner.run().await.unwrap();

        assert_eq!(report.status, FlowFinalStatus::Success);
        assert!(report.results.get("resumen").unwrap().status.is_success());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn continue_session_usa_sesion_anterior() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = write_fake_claude(tmp.path());

        let flow = Flow::from_str(
            "name: f\nsteps:\n  - name: primero\n    type: claude\n    prompt: hola\n  - name: segundo\n    type: claude\n    prompt: continuo\n    continue_session: true\n",
            Path::new("test.yaml"),
        )
        .unwrap();
        let commands = HashMap::new();
        let runner = FlowRunner::with_claude(
            &flow,
            &commands,
            tmp.path().to_path_buf(),
            false,
            fake.to_str().unwrap(),
        );
        let report = runner.run().await.unwrap();

        let segundo = &report.results.get("segundo").unwrap().status;
        assert!(segundo.stderr().contains("--resume sess-1"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn flujo_con_step_opencode() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = write_fake_opencode(tmp.path());

        let flow = Flow::from_str(
            "name: f\nsteps:\n  - name: build\n    type: command\n    command: echo hola\n  - name: review\n    type: opencode\n    prompt: 'Resultado: {{steps.build.stdout}}'\n    model: anthropic/claude-sonnet-4\n",
            Path::new("test.yaml"),
        )
        .unwrap();
        let commands = HashMap::new();
        let runner = FlowRunner::with_opencode(
            &flow,
            &commands,
            tmp.path().to_path_buf(),
            false,
            fake.to_str().unwrap(),
        );
        let report = runner.run().await.unwrap();

        assert_eq!(report.status, FlowFinalStatus::Success);
        let review = &report.results.get("review").unwrap().status;
        assert!(review.is_success());
        assert!(review
            .stderr()
            .contains("--model anthropic/claude-sonnet-4"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn continue_session_opencode_usa_sesion_anterior() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = write_fake_opencode(tmp.path());

        let flow = Flow::from_str(
            "name: f\nsteps:\n  - name: primero\n    type: opencode\n    prompt: hola\n  - name: segundo\n    type: opencode\n    prompt: continuo\n    continue_session: true\n",
            Path::new("test.yaml"),
        )
        .unwrap();
        let commands = HashMap::new();
        let runner = FlowRunner::with_opencode(
            &flow,
            &commands,
            tmp.path().to_path_buf(),
            false,
            fake.to_str().unwrap(),
        );
        let report = runner.run().await.unwrap();

        let segundo = &report.results.get("segundo").unwrap().status;
        assert!(segundo.stderr().contains("--session oc-sess-1"));
    }
}
