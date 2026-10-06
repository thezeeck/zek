use std::collections::HashMap;
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use colored::Colorize;
use dialoguer::{Confirm, Input};

use zek_core::claude::{ClaudeClient, ClaudeOptions};
use zek_core::commands::{self as core_commands, LoadedCommand};
use zek_core::config::{
    find_project_config, home_dir, load_effective, Config, COMMANDS_DIR, FLOWS_DIR,
};
use zek_core::error::{FlowFinalStatus, ZekError};
use zek_core::exec::CommandExecutor;
use zek_core::execution::{FlowReport, FlowRunner, StepProgress};
use zek_core::flows::{self as core_flows, LoadedFlow};
use zek_core::lang::{self, Language};
use zek_core::step::StepType;
use zek_core::t;
use zek_core::util::expand_env_vars;

use crate::ReportFormat;

/// `zek init [dir]`: configura zek por primera vez (o re-configura).
pub fn init(dir: Option<PathBuf>) -> Result<()> {
    let workdir = match dir {
        Some(d) => expand_tilde(d)?,
        None => prompt_workdir()?,
    };

    ensure_workdir(&workdir)?;

    let config = Config::new(workdir.clone());
    config.save()?;

    println!(
        "\n{}",
        t!(msg_config_saved, Config::config_path()?.display())
    );
    println!("{}", t!(msg_workdir, workdir.display()));
    Ok(())
}

/// `zek config show`: muestra la config actual (hace init si no existe).
pub fn config_show() -> Result<()> {
    let config = ensure_config()?;

    println!("{}", t!(label_config, Config::config_path()?.display()));
    if let Some(project) = find_project_config() {
        println!("{}", t!(label_project, project.display()));
    }
    println!("{}", t!(label_workdir, config.workdir.display()));
    println!(
        "  {:<8}: {} ({})",
        COMMANDS_DIR,
        config.commands_dir().display(),
        status(&config.commands_dir())
    );
    println!(
        "  {:<8}: {} ({})",
        FLOWS_DIR,
        config.flows_dir().display(),
        status(&config.flows_dir())
    );
    Ok(())
}

/// `zek config set-dir <path>`: cambia la carpeta de trabajo global.
pub fn config_set_dir(path: PathBuf) -> Result<()> {
    let mut config = ensure_global_config()?;

    config.workdir = expand_tilde(path)?;
    config
        .validate()
        .context(lang::messages().err_workdir_invalid)?;
    config.save()?;

    println!("{}", t!(msg_workdir_updated, config.workdir.display()));
    Ok(())
}

/// `zek config set-language <en|es>`: cambia el idioma global de los mensajes.
pub fn config_set_language(language: &str) -> Result<()> {
    let mut config = ensure_global_config()?;

    let lang = Language::parse(language)
        .ok_or_else(|| anyhow::anyhow!(t!(err_language_invalid, language)))?;
    config.language = lang;
    config.save()?;
    lang::set(lang);

    println!("{}", t!(msg_language_updated, lang.as_str()));
    Ok(())
}

/// `zek` sin subcomando: asegura config y da una pista.
pub fn run_default() -> Result<()> {
    ensure_config()?;
    println!("{}", lang::messages().msg_configured);
    Ok(())
}

/// `zek list`: lista los comandos y flujos disponibles.
pub fn list() -> Result<()> {
    let config = ensure_config()?;
    let commands = core_commands::load_all(&config.commands_dir())?;
    let flows = core_flows::load_all(&config.flows_dir())?;

    for flow in flows.values() {
        for warning in &flow.warnings {
            eprintln!("{}", t!(warning_at, warning, flow.source.display()));
        }
        for warning in flow.flow.validate_command_refs(&commands) {
            eprintln!("{}", t!(warning_at, warning, flow.source.display()));
        }
        for warning in flow.flow.validate_flow_refs(&flows) {
            eprintln!("{}", t!(warning_at, warning, flow.source.display()));
        }
    }

    println!("{}", t!(label_commands, config.commands_dir().display()));
    let mut cmd_entries: Vec<_> = commands.values().collect();
    cmd_entries.sort_by_key(|c| c.command.name.as_str());
    if cmd_entries.is_empty() {
        println!("  {}", lang::messages().label_none);
    } else {
        for c in cmd_entries {
            println!(
                "  {:<20} {:<36} {}",
                c.command.name,
                c.command.description,
                c.source.display()
            );
        }
    }

    println!("\n{}", t!(label_flows, config.flows_dir().display()));
    let mut flow_entries: Vec<_> = flows.values().collect();
    flow_entries.sort_by_key(|f| f.flow.name.as_str());
    if flow_entries.is_empty() {
        println!("  {}", lang::messages().label_none);
    } else {
        for f in flow_entries {
            println!(
                "  {:<20} {:<36} {}",
                f.flow.name,
                f.flow.description,
                f.source.display()
            );
        }
    }

    Ok(())
}

/// `zek commands <nombre>`: muestra los detalles de un comando.
pub fn show_command(name: &str) -> Result<()> {
    let config = ensure_config()?;
    let commands = core_commands::load_all(&config.commands_dir())?;

    let loaded = commands
        .get(name)
        .ok_or_else(|| anyhow::anyhow!(t!(err_command_not_found, name)))?;
    let c = &loaded.command;

    println!("{}", t!(label_command, c.name));
    println!(
        "{}",
        t!(
            label_description,
            if c.description.is_empty() {
                "-"
            } else {
                &c.description
            }
        )
    );
    println!("{}", t!(label_run, c.run));
    println!("{}", t!(label_cwd, c.cwd.as_deref().unwrap_or(".")));
    println!("{}", t!(label_timeout, c.timeout));
    println!("{}", t!(label_author, c.author.as_deref().unwrap_or("-")));
    println!("{}", t!(label_source, loaded.source.display()));
    Ok(())
}

/// `zek ask <mensaje>`: pregunta algo a claude fuera de flujos.
pub async fn ask(message: &str) -> Result<()> {
    let client = ClaudeClient::new();
    let opts = ClaudeOptions {
        output_format: None,
        session_id: None,
        timeout: Duration::from_secs(60),
        stream: true,
        cwd: None,
        env: HashMap::new(),
    };

    let status = client.run(message, &opts).await;
    if status.is_success() {
        Ok(())
    } else {
        let code = status.exit_code().unwrap_or(-1);
        bail!("{}", t!(err_claude_failed, code));
    }
}

/// `zek <nombre> [--clave valor ...]`: ejecuta un flujo o un comando por nombre.
#[allow(clippy::too_many_arguments)]
pub async fn run_by_name(
    name: &str,
    args: &[String],
    vars: &[String],
    selection: zek_core::plan::Selection,
    flow_only: bool,
    dry_run: bool,
    verbose: bool,
    debug: bool,
    timeout_global: Option<u64>,
    log: Option<PathBuf>,
    report: Option<ReportFormat>,
) -> Result<i32> {
    let config = ensure_config()?;
    let vars = parse_vars(vars)?;
    let catalogs = (|| -> Result<_> {
        Ok((
            core_commands::load_all(&config.commands_dir())?,
            core_flows::load_all(&config.flows_dir())?,
        ))
    })();
    let (commands, flows) = match catalogs {
        Ok(catalogs) => catalogs,
        Err(error) => {
            if !dry_run {
                record_preflight_error(&config, name)?;
            }
            return Err(error);
        }
    };
    let opts = RunOptions {
        selection,
        dry_run,
        verbose,
        debug,
        timeout_global,
        log,
        report,
        vars,
    };

    if let Some(loaded) = flows.get(name) {
        return run_flow(&config, &commands, &flows, loaded, args, &opts).await;
    }
    if flow_only {
        record_preflight_error(&config, name)?;
        bail!("{}", t!(err_watch_flow, name));
    }
    if let Some(loaded) = commands.get(name) {
        if opts.selection != zek_core::plan::Selection::All {
            bail!(zek_core::lang::messages().err_selection_flow);
        }
        if !opts.vars.is_empty() {
            bail!("{}", lang::messages().err_vars_require_flow);
        }
        return run_command(&config, loaded, &opts).await;
    }
    bail!("{}", t!(err_name_not_found, name));
}

struct RunOptions {
    selection: zek_core::plan::Selection,
    dry_run: bool,
    verbose: bool,
    debug: bool,
    timeout_global: Option<u64>,
    log: Option<PathBuf>,
    report: Option<ReportFormat>,
    vars: HashMap<String, serde_json::Value>,
}

async fn run_flow(
    config: &Config,
    commands: &HashMap<String, LoadedCommand>,
    flows: &HashMap<String, LoadedFlow>,
    loaded: &LoadedFlow,
    args: &[String],
    opts: &RunOptions,
) -> Result<i32> {
    let flow = &loaded.flow;

    for warning in &loaded.warnings {
        eprintln!("{}", t!(warning_at, warning, loaded.source.display()));
    }
    for warning in flow.validate_flow_refs(flows) {
        eprintln!("{}", t!(warning_at, warning, loaded.source.display()));
    }

    let plan = match (|| -> Result<_> {
        let plan = zek_core::plan::ExecutionPlan::build(flow, &opts.selection)?;
        plan.validate_catalog_inputs(flow, commands, flows)?;
        Ok(plan)
    })() {
        Ok(plan) => plan,
        Err(error) => {
            if !opts.dry_run {
                record_preflight_error(config, &flow.name)?;
            }
            return Err(error);
        }
    };
    if opts.dry_run {
        print!("{}", plan.graph(flow, false));
        return Ok(0);
    }

    let logger = Logger::open(opts.log.as_deref())?;
    logger.log(&format!("flow started: {}", flow.name));

    let params = parse_params(args);

    let progress_logger = logger.clone();
    let stream = opts.report.is_none();
    let history = config.history_store()?.start(&flow.name)?;
    let runner = FlowRunner::new(flow, commands, config.workdir.clone(), stream)
        .with_flows(flows)
        .with_history_run(history.clone())
        .with_selection(opts.selection.clone())
        .with_args(params)
        .with_vars(opts.vars.clone())
        .on_progress(Arc::new(move |p| {
            log_progress(&progress_logger, &p);
            print_progress(p);
        }))
        .on_confirm(Arc::new(confirm_step));

    let work = async {
        match opts.timeout_global {
            Some(secs) => match tokio::time::timeout(Duration::from_secs(secs), runner.run()).await
            {
                Ok(result) => Ok(result?),
                Err(_) => {
                    history.event(
                        &flow.name,
                        None,
                        None,
                        "global_timeout",
                        Some("aborted"),
                        None,
                        None,
                    )?;
                    history.finish("aborted", None)?;
                    bail!("{}", t!(err_timeout_exceeded, secs));
                }
            },
            None => Ok(runner.run().await?),
        }
    };
    let report = tokio::select! {
        signal = tokio::signal::ctrl_c() => {signal?; history.finish("cancelled",None)?; logger.log("flow cancelled"); return Ok(130);},
        result = work => result?,
    };

    logger.log(&format!(
        "flow finished: {} exit code {}",
        report.status.as_str(),
        report.exit_code()
    ));

    match opts.report {
        Some(format) => print_report(flows, &report, format)?,
        None => print_summary(flows, &report, opts.verbose, opts.debug),
    }
    Ok(report.exit_code())
}

async fn run_command(config: &Config, loaded: &LoadedCommand, opts: &RunOptions) -> Result<i32> {
    let cmd = &loaded.command;
    let cwd = cmd.cwd.as_deref().map(|c| {
        let p = PathBuf::from(c);
        if p.is_absolute() {
            p
        } else {
            config.workdir.join(p)
        }
    });

    let logger = Logger::open(opts.log.as_deref())?;
    logger.log(&format!("command started: {}", cmd.name));

    let timeout = opts
        .timeout_global
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(cmd.timeout as u64));
    let mut executor = CommandExecutor::new(cmd.run.clone())
        .timeout(timeout)
        .stream(true);
    if let Some(cwd) = cwd {
        executor = executor.cwd(cwd);
    }
    for (key, value) in &cmd.env {
        executor = executor.env(key.clone(), expand_env_vars(value));
    }

    let status = executor.execute().await;
    logger.log(&format!(
        "command finished: {} ({})",
        cmd.name,
        status.status_str()
    ));
    if status.is_success() {
        Ok(0)
    } else {
        eprintln!(
            "{}",
            t!(
                err_command_failed,
                cmd.name,
                status.exit_code().unwrap_or(-1)
            )
            .red()
            .bold()
        );
        Ok(status.exit_code().unwrap_or(1))
    }
}

fn print_progress(progress: StepProgress) {
    match progress {
        StepProgress::Started { name, index, total } => {
            eprintln!(
                "{}",
                format!("▶ {name} ({}/{total})", index + 1).cyan().bold()
            );
        }
        StepProgress::Finished { name, status } => {
            if status.is_success() {
                eprintln!("  {}", format!("✓ {name}").green());
            } else {
                eprintln!("  {}", format!("✗ {name}").red());
            }
        }
    }
}

fn confirm_step(name: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    Confirm::new()
        .with_prompt(t!(confirm_step, name))
        .default(true)
        .interact()
        .unwrap_or(false)
}

fn print_summary(
    flows: &HashMap<String, LoadedFlow>,
    report: &FlowReport,
    verbose: bool,
    debug: bool,
) {
    let status_colored = match report.status {
        FlowFinalStatus::Success => report.status.as_str().green().bold(),
        FlowFinalStatus::Failed => report.status.as_str().red().bold(),
        FlowFinalStatus::Aborted => report.status.as_str().yellow().bold(),
    };

    println!();
    println!("{}", t!(sum_flow, report.name).bold());
    if let Some(run_id) = &report.run_id {
        println!("{}", t!(history_run_id, run_id));
    }
    println!("{}", t!(sum_status, status_colored));
    println!("{}", t!(sum_exit_code, report.exit_code()));
    println!("{}", t!(sum_duration, format_duration(report.duration)));

    let types = step_types(flows);

    println!();
    println!("{}", lang::messages().sum_steps);
    for (name, result) in report.results.ordered_results() {
        let type_str = type_str(types.get(&name));
        let marker = if result.status.is_success() {
            "✓".green()
        } else {
            "✗".red()
        };
        println!(
            "  {marker} {name:<20} ({type_str}, {}, {}, {})",
            result.status.status_str(),
            format_duration(result.duration),
            t!(sum_attempts, result.attempts)
        );
    }
    for name in &report.excluded_steps {
        println!("  → {name:<20} (excluded)");
    }
    for name in &report.skipped_steps {
        println!("  {} {name:<20} (skipped)", "→".yellow());
    }

    if !report.failed_steps.is_empty() {
        println!();
        println!(
            "{}",
            t!(sum_failed_steps, report.failed_steps.join(", ")).red()
        );
        println!("{}", t!(sum_exit_reason, report.exit_reason));
    }

    if verbose || debug {
        for (name, result) in report.results.ordered_results() {
            if !result.status.is_success() {
                println!();
                println!("{}", t!(sum_output_of, name).yellow());
                if !result.status.stdout().trim().is_empty() {
                    println!("{}", result.status.stdout());
                }
                if !result.status.stderr().trim().is_empty() {
                    println!("{}", result.status.stderr());
                }
            }
        }
    }
}

fn step_types(flows: &HashMap<String, LoadedFlow>) -> HashMap<String, StepType> {
    let mut types = HashMap::new();
    for loaded in flows.values() {
        for step in &loaded.flow.steps {
            types.insert(step.name.clone(), step.step_type);
        }
        if let Some(fin) = &loaded.flow.finally {
            for step in &fin.steps {
                types.insert(step.name.clone(), step.step_type);
            }
        }
    }
    types
}

fn type_str(step_type: Option<&StepType>) -> &'static str {
    match step_type {
        Some(StepType::Command) => "command",
        Some(StepType::Claude) => "claude",
        Some(StepType::Opencode) => "opencode",
        Some(StepType::Flow) => "flow",
        None => "?",
    }
}

fn print_report(
    flows: &HashMap<String, LoadedFlow>,
    report: &FlowReport,
    format: ReportFormat,
) -> Result<()> {
    match format {
        ReportFormat::Json => {
            let value = report_json(flows, report);
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        ReportFormat::Markdown => {
            print!("{}", report_markdown(flows, report));
        }
    }
    Ok(())
}

fn report_json(flows: &HashMap<String, LoadedFlow>, report: &FlowReport) -> serde_json::Value {
    let types = step_types(flows);
    let steps: Vec<serde_json::Value> = report
        .results
        .ordered_results()
        .into_iter()
        .map(|(name, result)| {
            serde_json::json!({
                "name": name,
                "type": type_str(types.get(&name)),
                "status": result.status.status_str(),
                "exit_code": result.status.exit_code(),
                "attempts": result.attempts,
                "duration_ms": result.duration.as_millis(),
                "stdout": result.status.stdout(),
                "stderr": result.status.stderr(),
            })
        })
        .collect();

    serde_json::json!({
        "name": report.name,
        "run_id": report.run_id,
        "status": report.status.as_str(),
        "exit_code": report.exit_code(),
        "duration_ms": report.duration.as_millis(),
        "exit_reason": report.exit_reason,
        "failed_steps": report.failed_steps,
        "skipped_steps": report.skipped_steps,
        "excluded_steps": report.excluded_steps,
        "skip_reasons": report.skip_reasons,
        "steps": steps,
    })
}

fn report_markdown(flows: &HashMap<String, LoadedFlow>, report: &FlowReport) -> String {
    let types = step_types(flows);
    let mut out = String::new();

    out.push_str(&format!("# Flow: {}\n\n", report.name));
    if let Some(run_id) = &report.run_id {
        out.push_str(&format!("- **Run ID**: {run_id}\n"));
    }
    out.push_str(&format!("- **Status**: {}\n", report.status.as_str()));
    out.push_str(&format!("- **Exit code**: {}\n", report.exit_code()));
    out.push_str(&format!(
        "- **Duration**: {}\n",
        format_duration(report.duration)
    ));
    out.push_str(&format!("- **Exit reason**: {}\n", report.exit_reason));

    if !report.failed_steps.is_empty() {
        out.push_str(&format!(
            "- **Failed steps**: {}\n",
            report.failed_steps.join(", ")
        ));
    }
    if !report.skipped_steps.is_empty() {
        out.push_str(&format!(
            "- **Skipped steps**: {}\n",
            report.skipped_steps.join(", ")
        ));
    }

    if !report.excluded_steps.is_empty() {
        out.push_str(&format!(
            "- **Excluded steps**: {}\n",
            report.excluded_steps.join(", ")
        ));
    }
    out.push_str("\n## Steps\n\n");
    out.push_str("| Step | Type | Status | Exit code | Attempts | Duration |\n");
    out.push_str("|------|------|--------|-----------|----------|----------|\n");
    for (name, result) in report.results.ordered_results() {
        let exit_code = result
            .status
            .exit_code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!(
            "| {name} | {} | {} | {exit_code} | {} | {} |\n",
            type_str(types.get(&name)),
            result.status.status_str(),
            result.attempts,
            format_duration(result.duration)
        ));
    }
    for name in &report.excluded_steps {
        out.push_str(&format!("| {name} | - | excluded | - | - | - |\n"));
    }
    for name in &report.skipped_steps {
        out.push_str(&format!("| {name} | - | skipped | - | - | - |\n"));
    }

    for (name, result) in report.results.ordered_results() {
        if !result.status.is_success() {
            out.push_str(&format!("\n## Output: {name}\n\n"));
            if !result.status.stdout().trim().is_empty() {
                out.push_str(&format!("```\n{}\n```\n", result.status.stdout()));
            }
            if !result.status.stderr().trim().is_empty() {
                out.push_str(&format!("```\n{}\n```\n", result.status.stderr()));
            }
        }
    }

    out
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else if secs > 0 {
        format!("{secs}s")
    } else {
        format!("{}ms", d.as_millis())
    }
}

/// Escritor de log en modo append. Los mensajes se guardan con timestamp.
#[derive(Clone)]
struct Logger {
    file: Option<Arc<Mutex<fs::File>>>,
}

impl Logger {
    fn open(path: Option<&Path>) -> Result<Self> {
        let file = match path {
            Some(p) => {
                let f = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)
                    .with_context(|| t!(err_log_open, p.display()))?;
                Some(Arc::new(Mutex::new(f)))
            }
            None => None,
        };
        Ok(Self { file })
    }

    fn log(&self, line: &str) {
        if let Some(file) = &self.file {
            if let Ok(mut file) = file.lock() {
                let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f");
                let _ = writeln!(file, "{ts}  {line}");
                let _ = file.flush();
            }
        }
    }
}

fn log_progress(logger: &Logger, progress: &StepProgress) {
    match progress {
        StepProgress::Started { name, index, total } => {
            logger.log(&format!("step started: {name} ({}/{total})", index + 1));
        }
        StepProgress::Finished { name, status } => {
            logger.log(&format!("step finished: {name} ({})", status.status_str()));
            for line in status.stdout().lines() {
                logger.log(&format!("  [stdout] {line}"));
            }
            for line in status.stderr().lines() {
                logger.log(&format!("  [stderr] {line}"));
            }
        }
    }
}

/// Parsea overrides KEY=VALUE: JSON válido conserva su tipo; el resto es texto.
/// Una clave repetida usa su última asignación, sin modificar los argumentos.
fn parse_vars(vars: &[String]) -> Result<HashMap<String, serde_json::Value>> {
    let mut parsed = HashMap::new();
    for definition in vars {
        let (key, value) = definition
            .split_once('=')
            .filter(|(key, _)| !key.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!(t!(err_var_invalid, definition)))?;
        let value = serde_json::from_str(value)
            .unwrap_or_else(|_| serde_json::Value::String(value.to_string()));
        parsed.insert(key.trim().to_string(), value);
    }
    Ok(parsed)
}

/// Parsea argumentos de CLI como pares clave/valor para `{{args.<clave>}}`.
/// Acepta `--clave valor`, `--clave=valor` y `--flag` (que queda como "").
fn parse_params(args: &[String]) -> HashMap<String, String> {
    let mut params = HashMap::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some(rest) = arg.strip_prefix("--") {
            if rest.is_empty() {
                i += 1;
                continue;
            }
            if let Some((key, value)) = rest.split_once('=') {
                params.insert(key.to_string(), value.to_string());
            } else if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                params.insert(rest.to_string(), args[i + 1].clone());
                i += 1;
            } else {
                params.insert(rest.to_string(), String::new());
            }
        }
        i += 1;
    }
    params
}

fn status(path: &Path) -> &'static str {
    if path.is_dir() {
        lang::messages().status_ok
    } else {
        lang::messages().status_missing
    }
}

/// Carga la config global; si no existe, dispara el wizard de init.
fn ensure_global_config() -> Result<Config> {
    match Config::load() {
        Ok(config) => Ok(config),
        Err(ZekError::ConfigNotFound(_)) => {
            println!("{}\n", lang::messages().msg_no_config);
            init(None)?;
            Config::load().context(lang::messages().err_config_after_init)
        }
        Err(e) => Err(e).context(lang::messages().err_config_load),
    }
}

/// Carga la config efectiva (global + `zek.yaml` del proyecto) y fija el idioma.
fn ensure_config() -> Result<Config> {
    match load_effective() {
        Ok(config) => {
            lang::set(config.language);
            Ok(config)
        }
        Err(ZekError::ConfigNotFound(_)) => {
            println!("{}\n", lang::messages().msg_no_config);
            init(None)?;
            load_effective().context(lang::messages().err_config_after_init)
        }
        Err(e) => Err(e).context(lang::messages().err_config_load),
    }
}

fn prompt_workdir() -> Result<PathBuf> {
    let default = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .display()
        .to_string();

    let input: String = Input::new()
        .with_prompt(lang::messages().prompt_workdir)
        .default(default)
        .interact_text()?;

    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("{}", lang::messages().err_workdir_empty);
    }
    expand_tilde(PathBuf::from(trimmed))
}

/// Se asegura de que el workdir exista y tenga `commands/` y `flows/`.
fn ensure_workdir(workdir: &Path) -> Result<()> {
    if !workdir.exists() {
        let create = Confirm::new()
            .with_prompt(t!(prompt_create_dir, workdir.display()))
            .default(true)
            .interact()?;
        if !create {
            bail!("{}", lang::messages().err_init_cancelled);
        }
        fs::create_dir_all(workdir)?;
    }

    if !workdir.is_dir() {
        bail!("{}", t!(err_not_a_dir, workdir.display()));
    }

    for sub in [COMMANDS_DIR, FLOWS_DIR] {
        let p = workdir.join(sub);
        if !p.exists() {
            let create = Confirm::new()
                .with_prompt(t!(prompt_create_subdir, p.display()))
                .default(true)
                .interact()?;
            if create {
                fs::create_dir_all(&p)?;
            }
        }
    }
    Ok(())
}

/// Expande `~` y `~/...` usando el home del usuario.
fn expand_tilde(path: PathBuf) -> Result<PathBuf> {
    if path.as_os_str() == "~" {
        return Ok(home_dir()?);
    }
    if let Some(rest) = path.to_str().and_then(|s| s.strip_prefix("~/")) {
        return Ok(home_dir()?.join(rest));
    }
    Ok(path)
}

/// Render only the shared execution plan; never invoke process or AI clients.
pub fn graph(name: &str, mermaid: bool, selection: &zek_core::plan::Selection) -> Result<()> {
    let config = ensure_config()?;
    let flows = core_flows::load_all(&config.flows_dir())?;
    let commands = core_commands::load_all(&config.commands_dir())?;
    let loaded = flows
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("{}", t!(err_name_not_found, name)))?;
    let plan = zek_core::plan::ExecutionPlan::build(&loaded.flow, selection)?;
    plan.validate_catalog_inputs(&loaded.flow, &commands, &flows)?;
    print!("{}", plan.graph(&loaded.flow, mermaid));
    Ok(())
}

fn record_preflight_error(config: &Config, flow: &str) -> Result<()> {
    let run = config.history_store()?.start(flow)?;
    run.event(
        flow,
        None,
        None,
        "internal_error",
        Some("error"),
        None,
        None,
    )?;
    run.finish("error", None)?;
    Ok(())
}

pub fn history(flow: Option<&str>, status: Option<&str>, limit: usize, json: bool) -> Result<()> {
    let config = ensure_config()?;
    let store = config.history_store()?;
    store.prune()?;
    let runs = store.list(flow, status, limit)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&runs)?);
    } else {
        println!("{}", lang::messages().history_heading);
        for run in runs {
            println!(
                "{}  {}  {}  {}",
                run.run_id, run.flow, run.status, run.started_at_ms
            );
        }
    }
    Ok(())
}
pub fn logs(id: &str) -> Result<()> {
    let config = ensure_config()?;
    for event in config.history_store()?.events(id)? {
        println!("{}", serde_json::to_string(&event)?);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn watch(
    name: &str,
    args: &[String],
    vars: &[String],
    selection: zek_core::plan::Selection,
    mut options: zek_core::watch::WatchOptions,
    interval_ms: u64,
    verbose: bool,
    debug: bool,
    timeout_global: Option<u64>,
    log: Option<PathBuf>,
    report: Option<ReportFormat>,
) -> Result<i32> {
    let config = ensure_config()?;
    options
        .ignored_paths
        .push(config.history_store()?.directory);
    if let Some(log) = &log {
        options.ignored_paths.push(log.clone());
    }
    let mut watcher = zek_core::watch::PollWatcher::new(config.workdir.clone(), options)?;
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    eprintln!("{}", t!(watch_started, config.workdir.display()));
    loop {
        let setup = match load_effective() {
            Ok(current) => {
                watcher.ignore_path(current.history_store()?.directory)?;
                Ok(())
            }
            Err(error) => {
                record_preflight_error(&config, name)?;
                Err(anyhow::Error::from(error))
            }
        };
        let work = async {
            setup?;
            run_by_name(
                name,
                args,
                vars,
                selection.clone(),
                true,
                false,
                verbose,
                debug,
                timeout_global,
                log.clone(),
                report,
            )
            .await
        };
        tokio::pin!(work);
        let result = loop {
            tokio::select! {
                biased;
                signal=&mut shutdown => {signal?; return Ok(130);},
                result=&mut work => break result,
                _=interval.tick() => { if let Err(error)=watcher.poll(std::time::Instant::now()) {eprintln!("{}",t!(watch_scan_error,error));} }
            }
        };
        match result {
            Ok(130) => return Ok(130),
            Err(error) => eprintln!("error: {error:#}"),
            _ => {}
        }
        // Catch changes that arrived after the final poll during execution.
        if let Err(error) = watcher.poll(std::time::Instant::now()) {
            eprintln!("{}", t!(watch_scan_error, error));
        }
        loop {
            if watcher.take_ready(std::time::Instant::now()) {
                break;
            }
            tokio::select! {
                biased;
                signal=&mut shutdown => {signal?;return Ok(130);},
                _=interval.tick() => {if let Err(error)=watcher.poll(std::time::Instant::now()) {eprintln!("{}",t!(watch_scan_error,error));}}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(args: &[&str]) -> HashMap<String, String> {
        let v: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        parse_params(&v)
    }

    #[test]
    fn parsea_clave_valor_y_flag() {
        let p = params(&["--branch", "feature-x", "--force"]);
        assert_eq!(p.get("branch").map(String::as_str), Some("feature-x"));
        assert_eq!(p.get("force").map(String::as_str), Some(""));
    }

    #[test]
    fn parsea_clave_igual_valor() {
        let p = params(&["--branch=feature-x"]);
        assert_eq!(p.get("branch").map(String::as_str), Some("feature-x"));
    }

    #[test]
    fn ignora_argumentos_posicionales() {
        let p = params(&["extra", "--branch", "x"]);
        assert_eq!(p.get("branch").map(String::as_str), Some("x"));
        assert!(!p.contains_key("extra"));
    }

    #[test]
    fn vars_preserve_json_types_and_literal_text() {
        let definitions = [
            "number=42",
            "float=1.5",
            "enabled=false",
            "empty=null",
            r#"object={"nested":1}"#,
            "items=[1,true]",
            r#"text="true""#,
            "raw=a=b",
            "blank=",
            "invalid={broken}",
        ]
        .map(String::from);
        let vars = parse_vars(&definitions).unwrap();
        assert_eq!(vars["number"], serde_json::json!(42));
        assert_eq!(vars["float"], serde_json::json!(1.5));
        assert_eq!(vars["enabled"], serde_json::json!(false));
        assert_eq!(vars["empty"], serde_json::Value::Null);
        assert_eq!(vars["object"], serde_json::json!({"nested": 1}));
        assert_eq!(vars["items"], serde_json::json!([1, true]));
        assert_eq!(vars["text"], serde_json::json!("true"));
        assert_eq!(vars["raw"], serde_json::json!("a=b"));
        assert_eq!(vars["blank"], serde_json::json!(""));
        assert_eq!(vars["invalid"], serde_json::json!("{broken}"));
    }

    #[test]
    fn vars_replace_whole_values_and_reject_empty_keys() {
        let vars = parse_vars(&[
            r#"object={"first":1}"#.into(),
            r#"object={"second":2}"#.into(),
        ])
        .unwrap();
        assert_eq!(vars["object"], serde_json::json!({"second": 2}));
        for input in ["missing", "=value", " =value"] {
            assert!(parse_vars(&[input.into()]).is_err());
        }
    }
}
