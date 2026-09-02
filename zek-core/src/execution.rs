use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::claude::{extract_session_id, ClaudeClient, ClaudeOptions};
use crate::commands::LoadedCommand;
use crate::condition;
use crate::context::{ExecutionContext, SerializedStepResult};
use crate::error::{FlowFinalStatus, ZekError};
use crate::exec::{execute_with_retries, CommandExecutor, StepExecutionStatus};
use crate::flows::{Flow, LoadedFlow};
use crate::opencode::{
    extract_session_id as extract_opencode_session_id, OpencodeClient, OpencodeOptions,
};
use crate::step::{OnErrorAction, OnSuccessAction, Step, StepType};
use crate::util::expand_env_vars;

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
    flows: Option<&'a HashMap<String, LoadedFlow>>,
    workdir: PathBuf,
    stream: bool,
    claude: ClaudeClient,
    opencode: OpencodeClient,
    claude_session: Mutex<Option<String>>,
    opencode_session: Mutex<Option<String>>,
    progress: Option<Arc<ProgressCb>>,
    confirm: Option<Arc<ConfirmCb>>,
    args: HashMap<String, String>,
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
            flows: None,
            workdir,
            stream,
            claude: ClaudeClient::new(),
            opencode: OpencodeClient::new(),
            claude_session: Mutex::new(None),
            opencode_session: Mutex::new(None),
            progress: None,
            confirm: None,
            args: HashMap::new(),
        }
    }

    /// Registra el catálogo de flujos (necesario para steps `type: flow`).
    pub fn with_flows(mut self, flows: &'a HashMap<String, LoadedFlow>) -> Self {
        self.flows = Some(flows);
        self
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

    /// Argumentos pasados por CLI, disponibles como `{{args.<clave>}}`.
    pub fn with_args(mut self, args: HashMap<String, String>) -> Self {
        self.args = args;
        self
    }

    pub async fn run(&self) -> Result<FlowReport, ZekError> {
        let start = Instant::now();
        let mut ctx = ExecutionContext::new();
        ctx.set_args(self.args.clone());
        let mut skipped = Vec::new();

        let (status, exit_reason) = self
            .run_flow_internal(self.flow, &mut ctx, &mut skipped)
            .await?;
        let failed_steps = ctx.failed_step_names();

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

    /// Ejecuta un flujo (main + finally) registrando sus steps en `ctx`.
    async fn run_flow_internal(
        &self,
        flow: &Flow,
        ctx: &mut ExecutionContext,
        skipped: &mut Vec<String>,
    ) -> Result<(FlowFinalStatus, String), ZekError> {
        let (outcome, any_failed) = self
            .run_steps(&flow.steps, ctx, skipped, flow.max_jumps, false)
            .await?;
        let (mut status, mut reason) = outcome_to_status(outcome, any_failed, flow.max_jumps);
        ctx.set_flow_result(status, reason.clone());

        if let Some(fin) = &flow.finally {
            if !fin.steps.is_empty() {
                let max_jumps = fin.max_jumps.unwrap_or(DEFAULT_FINALLY_MAX_JUMPS);
                let (_, fin_failed) = self
                    .run_steps(&fin.steps, ctx, skipped, max_jumps, true)
                    .await?;
                if fin_failed && fin.fail_flow_on_error && status == FlowFinalStatus::Success {
                    status = FlowFinalStatus::Failed;
                    reason = "finally".to_string();
                }
            }
        }

        Ok((status, reason))
    }

    /// Recorre una secuencia de steps (main o `finally`) aplicando condiciones,
    /// confirmación, `on_error`/`on_success`, `goto` y grupos paralelos.
    async fn run_steps(
        &self,
        steps: &[Step],
        ctx: &mut ExecutionContext,
        skipped: &mut Vec<String>,
        max_jumps: usize,
        in_finally: bool,
    ) -> Result<(MainOutcome, bool), ZekError> {
        let total = steps.len();
        let mut idx = 0usize;
        let mut jumps = 0usize;
        let mut any_failed = false;

        let outcome = loop {
            if idx >= steps.len() {
                break MainOutcome::NaturalEnd;
            }

            if steps[idx].parallel {
                let start = idx;
                while idx < steps.len() && steps[idx].parallel {
                    idx += 1;
                }
                let group = &steps[start..idx];
                if let Some(failed) = self
                    .run_parallel_group(group, ctx, skipped, start, total)
                    .await?
                {
                    any_failed = true;
                    break MainOutcome::Stop(failed);
                }
                continue;
            }

            let step = &steps[idx];

            if step.entry_only_via_goto && !ctx.was_targeted(&step.name) {
                skipped.push(step.name.clone());
                idx += 1;
                continue;
            }

            if !self.condition_met(step, ctx)? {
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
                any_failed = true;
            }

            match next_action(step, result.status.is_success(), in_finally) {
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
                            return Err(ZekError::InvalidConfig(crate::t!(
                                val_goto_missing_short,
                                target
                            )));
                        }
                    }
                }
            }
        };

        Ok((outcome, any_failed))
    }

    /// Ejecuta un grupo de steps en paralelo y espera a todos. Devuelve el nombre
    /// del primer step que falló (para detener el flujo), o `None` si todo ok.
    async fn run_parallel_group(
        &self,
        group: &[Step],
        ctx: &mut ExecutionContext,
        skipped: &mut Vec<String>,
        start: usize,
        total: usize,
    ) -> Result<Option<String>, ZekError> {
        let mut to_run: Vec<&Step> = Vec::new();
        for step in group {
            if step.entry_only_via_goto && !ctx.was_targeted(&step.name) {
                skipped.push(step.name.clone());
                continue;
            }
            if !self.condition_met(step, ctx)? {
                skipped.push(step.name.clone());
                continue;
            }
            if step.confirm && !self.ask_confirm(&step.name) {
                skipped.push(step.name.clone());
                continue;
            }
            to_run.push(step);
        }

        for (i, step) in to_run.iter().enumerate() {
            self.emit_started(&step.name, start + i, total);
        }

        let futures = to_run.iter().map(|step| self.execute_step(step, ctx));
        let results = futures::future::join_all(futures).await;

        let mut first_failed = None;
        for (step, result) in to_run.iter().zip(results) {
            let result = result?;
            ctx.record(step.name.clone(), result.clone());
            self.emit_finished(&step.name, result.status.clone());
            if !result.status.is_success() && first_failed.is_none() {
                first_failed = Some(step.name.clone());
            }
        }

        Ok(first_failed)
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

    /// Evalúa la condición `when` del step (si la hay). Devuelve `true` si el
    /// step debe ejecutarse.
    fn condition_met(&self, step: &Step, ctx: &ExecutionContext) -> Result<bool, ZekError> {
        let Some(condition) = &step.when else {
            return Ok(true);
        };
        let rendered = ctx.render(condition)?.trim().to_string();
        if rendered.is_empty() {
            return Ok(false);
        }
        condition::evaluate(&rendered)
            .map_err(|e| ZekError::InvalidConfig(crate::t!(err_when_invalid, step.name, e)))
    }

    async fn run_step(
        &self,
        step: &Step,
        ctx: &mut ExecutionContext,
    ) -> Result<SerializedStepResult, ZekError> {
        let result = if step.step_type == StepType::Flow {
            self.run_flow_step(step, ctx).await?
        } else {
            self.execute_step(step, ctx).await?
        };
        ctx.record(step.name.clone(), result.clone());
        Ok(result)
    }

    /// Ejecuta un step sin mutar el contexto (para poder correr en paralelo).
    async fn execute_step(
        &self,
        step: &Step,
        ctx: &ExecutionContext,
    ) -> Result<SerializedStepResult, ZekError> {
        let start = Instant::now();
        let (status, attempts) = match step.step_type {
            StepType::Command => self.run_command_step(step, ctx).await?,
            StepType::Claude => self.run_claude_step(step, ctx).await?,
            StepType::Opencode => self.run_opencode_step(step, ctx).await?,
            StepType::Flow => {
                return Err(ZekError::InvalidConfig(crate::t!(
                    val_parallel_flow,
                    step.name
                )));
            }
        };
        Ok(SerializedStepResult {
            status,
            duration: start.elapsed(),
            attempts,
        })
    }

    /// Ejecuta un step `type: flow` invocando el flujo referenciado como subrutina.
    async fn run_flow_step(
        &self,
        step: &Step,
        ctx: &mut ExecutionContext,
    ) -> Result<SerializedStepResult, ZekError> {
        let start = Instant::now();
        let name = step
            .flow
            .as_deref()
            .ok_or_else(|| ZekError::InvalidConfig(crate::t!(val_flow_missing_name, step.name)))?;
        let flows = self
            .flows
            .ok_or_else(|| ZekError::InvalidConfig(crate::t!(val_flow_no_catalog, step.name)))?;
        let loaded = flows.get(name).ok_or_else(|| {
            ZekError::InvalidConfig(crate::t!(val_flow_not_found, step.name, name))
        })?;

        let saved_status = ctx.flow_status;
        let saved_reason = ctx.exit_reason.clone();
        let mut sub_skipped = Vec::new();
        let (status, _reason) =
            Box::pin(self.run_flow_internal(&loaded.flow, ctx, &mut sub_skipped)).await?;
        ctx.flow_status = saved_status;
        ctx.exit_reason = saved_reason;

        let status = match status {
            FlowFinalStatus::Success => StepExecutionStatus::Success {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            },
            _ => StepExecutionStatus::Failed {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 1,
            },
        };

        Ok(SerializedStepResult {
            status,
            duration: start.elapsed(),
            attempts: 1,
        })
    }

    async fn run_command_step(
        &self,
        step: &Step,
        ctx: &ExecutionContext,
    ) -> Result<(StepExecutionStatus, u32), ZekError> {
        let resolved = resolve_command(step, self.commands)
            .ok_or_else(|| ZekError::InvalidConfig(crate::t!(val_step_no_command, step.name)))?;
        let run = ctx.render(&resolved.run)?;
        let cwd = resolve_cwd(resolved.cwd.as_deref(), &self.workdir);
        let env = prepare_env(&resolved.env, ctx)?;

        let mut executor = CommandExecutor::new(run)
            .timeout(resolved.timeout)
            .stream(self.stream);
        if let Some(cwd) = cwd {
            executor = executor.cwd(cwd);
        }
        for (key, value) in env {
            executor = executor.env(key, value);
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
            env: prepare_env(&step.env, ctx)?,
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
            env: prepare_env(&step.env, ctx)?,
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

fn next_action(step: &Step, success: bool, in_finally: bool) -> NextAction {
    if success {
        match step.effective_on_success() {
            OnSuccessAction::Continue => NextAction::Continue,
            OnSuccessAction::End => NextAction::End,
            OnSuccessAction::Goto(t) => NextAction::Goto(t),
        }
    } else {
        match step.effective_on_error(in_finally) {
            OnErrorAction::Stop => NextAction::Stop,
            OnErrorAction::Continue => NextAction::Continue,
            OnErrorAction::Goto(t) => NextAction::Goto(t),
        }
    }
}

fn outcome_to_status(
    outcome: MainOutcome,
    any_failed: bool,
    max_jumps: usize,
) -> (FlowFinalStatus, String) {
    match outcome {
        MainOutcome::NaturalEnd => (
            if any_failed {
                FlowFinalStatus::Failed
            } else {
                FlowFinalStatus::Success
            },
            "natural_end".to_string(),
        ),
        MainOutcome::End(step) => (
            if any_failed {
                FlowFinalStatus::Failed
            } else {
                FlowFinalStatus::Success
            },
            format!("end:{step}"),
        ),
        MainOutcome::Stop(step) => (FlowFinalStatus::Failed, format!("stop:{step}")),
        MainOutcome::Aborted => (
            FlowFinalStatus::Aborted,
            crate::t!(reason_infinite_loop, max_jumps),
        ),
    }
}

struct ResolvedCommand {
    run: String,
    cwd: Option<String>,
    timeout: Duration,
    env: HashMap<String, String>,
}

fn resolve_command(
    step: &Step,
    commands: &HashMap<String, LoadedCommand>,
) -> Option<ResolvedCommand> {
    let name = step.command.as_deref()?;
    let mut env = HashMap::new();
    let (run, cwd, timeout) = if let Some(loaded) = commands.get(name) {
        let cmd = &loaded.command;
        env.extend(cmd.env.clone());
        (
            cmd.run.clone(),
            cmd.cwd.clone(),
            Duration::from_secs(cmd.timeout as u64),
        )
    } else {
        (
            name.to_string(),
            step.cwd.clone(),
            Duration::from_secs(step.timeout as u64),
        )
    };
    env.extend(step.env.clone());
    Some(ResolvedCommand {
        run,
        cwd,
        timeout,
        env,
    })
}

/// Renderiza y expande las variables de entorno de un step (`{{...}}` y `$VAR`).
fn prepare_env(
    env: &HashMap<String, String>,
    ctx: &ExecutionContext,
) -> Result<HashMap<String, String>, ZekError> {
    let mut resolved = HashMap::new();
    for (key, value) in env {
        let rendered = ctx.render(value)?;
        resolved.insert(key.clone(), expand_env_vars(&rendered));
    }
    Ok(resolved)
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
    async fn args_se_renderizan_en_comandos() {
        let flow = Flow::from_str(
            "name: f\nsteps:\n  - name: create\n    type: command\n    command: echo {{args.branch}}\n",
            Path::new("test.yaml"),
        )
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let commands = HashMap::new();
        let mut args = HashMap::new();
        args.insert("branch".to_string(), "feature-x".to_string());
        let runner =
            FlowRunner::new(&flow, &commands, tmp.path().to_path_buf(), false).with_args(args);
        let report = runner.run().await.unwrap();

        assert_eq!(report.status, FlowFinalStatus::Success);
        assert_eq!(
            report.results.get("create").unwrap().status.stdout().trim(),
            "feature-x"
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
                    env: HashMap::new(),
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

    #[tokio::test]
    async fn env_se_injecta_en_comando() {
        let report = run_flow(
            "name: f\nsteps:\n  - name: a\n    type: command\n    command: echo $MI_VAR\n    env:\n      MI_VAR: hola-env\n",
        )
        .await;
        assert_eq!(report.status, FlowFinalStatus::Success);
        assert_eq!(
            report.results.get("a").unwrap().status.stdout().trim(),
            "hola-env"
        );
    }

    #[tokio::test]
    async fn env_referencia_var_de_proceso() {
        std::env::set_var("ZEK_ENV_TEST", "secreto");
        let report = run_flow(
            "name: f\nsteps:\n  - name: a\n    type: command\n    command: echo $TOKEN\n    env:\n      TOKEN: ${ZEK_ENV_TEST}\n",
        )
        .await;
        assert_eq!(report.status, FlowFinalStatus::Success);
        assert_eq!(
            report.results.get("a").unwrap().status.stdout().trim(),
            "secreto"
        );
    }

    #[tokio::test]
    async fn when_falso_salta_el_step() {
        let report = run_flow(
            "name: f\nsteps:\n  - name: a\n    type: command\n    command: echo hola\n  - name: b\n    type: command\n    command: echo chau\n    when: \"false\"\n",
        )
        .await;
        assert_eq!(report.status, FlowFinalStatus::Success);
        assert!(report.results.get("b").is_none());
        assert_eq!(report.skipped_steps, vec!["b".to_string()]);
    }

    #[tokio::test]
    async fn when_compara_exit_code() {
        let report = run_flow(
            "name: f\nsteps:\n  - name: build\n    type: command\n    command: echo ok\n  - name: solo_si_ok\n    type: command\n    command: echo paso\n    when: \"{{steps.build.exit_code}} == 0\"\n",
        )
        .await;
        assert_eq!(report.status, FlowFinalStatus::Success);
        assert!(report
            .results
            .get("solo_si_ok")
            .unwrap()
            .status
            .is_success());
    }

    #[tokio::test]
    async fn when_invalido_da_error() {
        let flow = Flow::from_str(
            "name: f\nsteps:\n  - name: a\n    type: command\n    command: echo hola\n    when: \"==\"\n",
            Path::new("test.yaml"),
        )
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let commands = HashMap::new();
        let runner = FlowRunner::new(&flow, &commands, tmp.path().to_path_buf(), false);
        let err = runner.run().await.unwrap_err();
        assert!(err.to_string().contains("when"));
    }

    #[tokio::test]
    async fn parallel_corre_steps_concurrentes() {
        let report = run_flow(
            "name: f\nsteps:\n  - name: a\n    type: command\n    command: echo a\n    parallel: true\n  - name: b\n    type: command\n    command: echo b\n    parallel: true\n  - name: c\n    type: command\n    command: echo c\n",
        )
        .await;
        assert_eq!(report.status, FlowFinalStatus::Success);
        assert!(report.results.get("a").unwrap().status.is_success());
        assert!(report.results.get("b").unwrap().status.is_success());
        assert!(report.results.get("c").unwrap().status.is_success());
    }

    #[tokio::test]
    async fn parallel_falla_si_alguno_falla() {
        let report = run_flow(
            "name: f\nsteps:\n  - name: a\n    type: command\n    command: echo a\n    parallel: true\n  - name: b\n    type: command\n    command: exit 1\n    parallel: true\n  - name: c\n    type: command\n    command: echo no-corre\n",
        )
        .await;
        assert_eq!(report.status, FlowFinalStatus::Failed);
        assert!(report.results.get("b").unwrap().status.is_failed());
        assert!(report.results.get("c").is_none());
    }

    #[tokio::test]
    async fn flow_step_invoca_otro_flujo() {
        let flow = Flow::from_str(
            "name: main\nsteps:\n  - name: sub\n    type: flow\n    flow: child\n  - name: after\n    type: command\n    command: echo hecho\n",
            Path::new("test.yaml"),
        )
        .unwrap();
        let child = Flow::from_str(
            "name: child\nsteps:\n  - name: build\n    type: command\n    command: echo compilando\n",
            Path::new("child.yaml"),
        )
        .unwrap();
        let mut flows = HashMap::new();
        flows.insert(
            "child".to_string(),
            LoadedFlow {
                flow: child,
                source: PathBuf::from("child.yaml"),
                warnings: Vec::new(),
            },
        );

        let tmp = tempfile::tempdir().unwrap();
        let commands = HashMap::new();
        let runner =
            FlowRunner::new(&flow, &commands, tmp.path().to_path_buf(), false).with_flows(&flows);
        let report = runner.run().await.unwrap();

        assert_eq!(report.status, FlowFinalStatus::Success);
        assert!(report.results.get("sub").unwrap().status.is_success());
        assert!(report.results.get("after").unwrap().status.is_success());
        assert_eq!(
            report.results.get("build").unwrap().status.stdout().trim(),
            "compilando"
        );
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
