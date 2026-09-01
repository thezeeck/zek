use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use colored::Colorize;
use dialoguer::{Confirm, Input};

use zek_core::claude::{ClaudeClient, ClaudeOptions};
use zek_core::commands::{self as core_commands, LoadedCommand};
use zek_core::config::{home_dir, Config, COMMANDS_DIR, FLOWS_DIR};
use zek_core::error::{FlowFinalStatus, ZekError};
use zek_core::exec::CommandExecutor;
use zek_core::execution::{FlowReport, FlowRunner, StepProgress};
use zek_core::flows::{self as core_flows, Flow, LoadedFlow};
use zek_core::lang::{self, Language};
use zek_core::step::StepType;
use zek_core::t;

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

/// `zek config set-dir <path>`: cambia la carpeta de trabajo.
pub fn config_set_dir(path: PathBuf) -> Result<()> {
    ensure_config()?;

    let workdir = expand_tilde(path)?;
    let config = Config::new(workdir);
    config
        .validate()
        .context(lang::messages().err_workdir_invalid)?;
    config.save()?;

    println!("{}", t!(msg_workdir_updated, config.workdir.display()));
    Ok(())
}

/// `zek config set-language <en|es>`: cambia el idioma de los mensajes.
pub fn config_set_language(language: &str) -> Result<()> {
    let mut config = ensure_config()?;

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
pub async fn run_by_name(
    name: &str,
    args: &[String],
    dry_run: bool,
    verbose: bool,
    debug: bool,
    timeout_global: Option<u64>,
) -> Result<i32> {
    let config = ensure_config()?;
    let commands = core_commands::load_all(&config.commands_dir())?;
    let flows = core_flows::load_all(&config.flows_dir())?;
    let opts = RunOptions {
        dry_run,
        verbose,
        debug,
        timeout_global,
    };

    if let Some(loaded) = flows.get(name) {
        return run_flow(&config, &commands, loaded, args, &opts).await;
    }
    if let Some(loaded) = commands.get(name) {
        return run_command(&config, loaded, &opts).await;
    }
    bail!("{}", t!(err_name_not_found, name));
}

struct RunOptions {
    dry_run: bool,
    verbose: bool,
    debug: bool,
    timeout_global: Option<u64>,
}

async fn run_flow(
    config: &Config,
    commands: &HashMap<String, LoadedCommand>,
    loaded: &LoadedFlow,
    args: &[String],
    opts: &RunOptions,
) -> Result<i32> {
    let flow = &loaded.flow;

    for warning in &loaded.warnings {
        eprintln!("{}", t!(warning_at, warning, loaded.source.display()));
    }

    if opts.dry_run {
        print_plan(flow);
        return Ok(0);
    }

    let params = parse_params(args);

    let runner = FlowRunner::new(flow, commands, config.workdir.clone(), true)
        .with_args(params)
        .on_progress(Arc::new(print_progress))
        .on_confirm(Arc::new(confirm_step));

    let run_future = runner.run();
    let report = match opts.timeout_global {
        Some(secs) => match tokio::time::timeout(Duration::from_secs(secs), run_future).await {
            Ok(res) => res?,
            Err(_) => bail!("{}", t!(err_timeout_exceeded, secs)),
        },
        None => run_future.await?,
    };

    print_summary(flow, &report, opts.verbose, opts.debug);
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

    let status = executor.execute().await;
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
    Confirm::new()
        .with_prompt(t!(confirm_step, name))
        .default(true)
        .interact()
        .unwrap_or(true)
}

fn print_plan(flow: &Flow) {
    println!("{}", t!(plan_label, flow.name).bold());
    for (i, step) in flow.steps.iter().enumerate() {
        let detail = match step.step_type {
            StepType::Command => step
                .command
                .as_deref()
                .unwrap_or(lang::messages().plan_no_command),
            StepType::Claude | StepType::Opencode => step
                .prompt
                .as_deref()
                .unwrap_or(lang::messages().plan_no_prompt),
        };
        let flag = if step.entry_only_via_goto {
            " [entry_only_via_goto]"
        } else {
            ""
        };
        println!(
            "  {}. {:<20} {:?}{flag} -> {detail}",
            i + 1,
            step.name,
            step.step_type
        );
    }
    if let Some(fin) = &flow.finally {
        println!("{}", lang::messages().plan_finally);
        for (i, step) in fin.steps.iter().enumerate() {
            let detail = match step.step_type {
                StepType::Command => step
                    .command
                    .as_deref()
                    .unwrap_or(lang::messages().plan_no_command),
                StepType::Claude | StepType::Opencode => step
                    .prompt
                    .as_deref()
                    .unwrap_or(lang::messages().plan_no_prompt),
            };
            println!(
                "    {}. {:<20} {:?} -> {detail}",
                i + 1,
                step.name,
                step.step_type
            );
        }
    }
}

fn print_summary(flow: &Flow, report: &FlowReport, verbose: bool, debug: bool) {
    let status_colored = match report.status {
        FlowFinalStatus::Success => report.status.as_str().green().bold(),
        FlowFinalStatus::Failed => report.status.as_str().red().bold(),
        FlowFinalStatus::Aborted => report.status.as_str().yellow().bold(),
    };

    println!();
    println!("{}", t!(sum_flow, report.name).bold());
    println!("{}", t!(sum_status, status_colored));
    println!("{}", t!(sum_exit_code, report.exit_code()));
    println!("{}", t!(sum_duration, format_duration(report.duration)));

    let mut types = HashMap::new();
    for step in &flow.steps {
        types.insert(step.name.clone(), step.step_type);
    }
    if let Some(fin) = &flow.finally {
        for step in &fin.steps {
            types.insert(step.name.clone(), step.step_type);
        }
    }

    println!();
    println!("{}", lang::messages().sum_steps);
    for (name, result) in report.results.ordered_results() {
        let type_str = match types.get(&name) {
            Some(StepType::Command) => "command",
            Some(StepType::Claude) => "claude",
            Some(StepType::Opencode) => "opencode",
            None => "?",
        };
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

/// Carga la config; si no existe, dispara el wizard de init automáticamente.
fn ensure_config() -> Result<Config> {
    match Config::load() {
        Ok(config) => {
            lang::set(config.language);
            Ok(config)
        }
        Err(ZekError::ConfigNotFound(_)) => {
            println!("{}\n", lang::messages().msg_no_config);
            init(None)?;
            Config::load().context(lang::messages().err_config_after_init)
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
}
