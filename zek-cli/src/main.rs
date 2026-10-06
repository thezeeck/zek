mod commands;

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};

#[derive(Parser)]
#[command(
    name = "zek",
    version,
    about = "Terminal workflow orchestrator with Claude and OpenCode"
)]
struct Cli {
    /// Preview the execution plan without running it
    #[arg(long, global = true)]
    dry_run: bool,

    /// Verbose logs
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Extra debug (paths, timestamps)
    #[arg(long, global = true)]
    debug: bool,

    /// Global timeout in seconds for the whole flow
    #[arg(long, global = true)]
    timeout_global: Option<u64>,

    /// Log file to write execution details
    #[arg(long, global = true)]
    log: Option<PathBuf>,

    /// Export a report (json|markdown) instead of the summary
    #[arg(long, global = true, value_enum)]
    report: Option<ReportFormat>,

    /// Override a root flow variable (repeatable; JSON values or plain strings)
    #[arg(long = "var", value_name = "KEY=VALUE")]
    vars: Vec<String>,

    /// Execute only this main step (requires available inputs)
    #[arg(long, conflicts_with = "until")]
    step: Option<String>,

    /// Execute through this step (includes DAG ancestors)
    #[arg(long, conflicts_with = "step")]
    until: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ReportFormat {
    Json,
    Markdown,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum GraphFormat {
    Text,
    Mermaid,
}

#[derive(Subcommand)]
enum Command {
    /// Set up zek for the first time (or reconfigure)
    Init {
        /// Working directory (if omitted, prompts interactively)
        dir: Option<PathBuf>,
    },
    /// Show or modify the configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// List available commands and flows
    List,
    /// Show a specific command
    Commands {
        /// Command name
        name: String,
    },
    /// Display a flow graph without executing commands
    Graph {
        name: String,
        #[arg(long, value_enum, default_value = "text")]
        format: GraphFormat,
    },
    /// List recorded flow executions
    History {
        #[arg(long)]
        flow: Option<String>,
        #[arg(long, value_parser = ["success", "failed", "aborted", "error", "cancelled", "running", "incomplete"])]
        status: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Read structured events for a recorded execution
    Logs { run_id: String },
    /// Rerun a flow when project files change
    Watch {
        name: String,
        #[arg(long = "include")]
        include: Vec<String>,
        #[arg(long = "exclude")]
        exclude: Vec<String>,
        #[arg(long, default_value_t = 300)]
        debounce_ms: u64,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u64).range(1..))]
        interval_ms: u64,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Ask Claude something outside of flows
    Ask {
        /// Message to send to claude -p
        message: String,
    },
    /// Generate the completion script for a shell
    Completion {
        /// Target shell
        #[arg(long, value_enum)]
        shell: Shell,
    },
    /// Run a flow or command by name
    #[command(external_subcommand)]
    Run(Vec<OsString>),
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Show the current configuration
    Show,
    /// Change the working directory
    SetDir {
        /// New working directory (must contain commands/ and flows/)
        path: PathBuf,
    },
    /// Change the language (en/es)
    SetLanguage {
        /// Language code (en or es)
        language: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("error: {err:#}");
            std::process::exit(1);
        }
    }
}

async fn run(cli: Cli) -> Result<i32> {
    if let Ok(config) = zek_core::config::Config::load() {
        zek_core::lang::set(config.language);
    }
    if !cli.vars.is_empty()
        && !matches!(
            &cli.command,
            Some(Command::Run(_)) | Some(Command::Watch { .. })
        )
    {
        bail!(zek_core::lang::messages().err_vars_require_flow);
    }

    let selection = match (cli.step, cli.until) {
        (Some(name), None) => zek_core::plan::Selection::Step(name),
        (None, Some(name)) => zek_core::plan::Selection::Until(name),
        _ => zek_core::plan::Selection::All,
    };
    if selection != zek_core::plan::Selection::All
        && !matches!(
            &cli.command,
            Some(Command::Run(_)) | Some(Command::Graph { .. }) | Some(Command::Watch { .. })
        )
    {
        bail!(zek_core::lang::messages().err_selection_flow);
    }
    match cli.command {
        Some(Command::Init { dir }) => {
            commands::init(dir)?;
            Ok(0)
        }
        Some(Command::Config {
            command: ConfigCommand::Show,
        }) => {
            commands::config_show()?;
            Ok(0)
        }
        Some(Command::Config {
            command: ConfigCommand::SetDir { path },
        }) => {
            commands::config_set_dir(path)?;
            Ok(0)
        }
        Some(Command::Config {
            command: ConfigCommand::SetLanguage { language },
        }) => {
            commands::config_set_language(&language)?;
            Ok(0)
        }
        Some(Command::List) => {
            commands::list()?;
            Ok(0)
        }
        Some(Command::Commands { name }) => {
            commands::show_command(&name)?;
            Ok(0)
        }
        Some(Command::Graph { name, format }) => {
            commands::graph(&name, matches!(format, GraphFormat::Mermaid), &selection)?;
            Ok(0)
        }
        Some(Command::History {
            flow,
            status,
            limit,
            json,
        }) => {
            commands::history(flow.as_deref(), status.as_deref(), limit, json)?;
            Ok(0)
        }
        Some(Command::Logs { run_id }) => {
            commands::logs(&run_id)?;
            Ok(0)
        }
        Some(Command::Watch {
            name,
            include,
            exclude,
            debounce_ms,
            interval_ms,
            args,
        }) => {
            if cli.dry_run {
                bail!("{}", zek_core::lang::messages().err_watch_dry_run);
            }
            let options = zek_core::watch::WatchOptions {
                include: if include.is_empty() {
                    vec!["**".into()]
                } else {
                    include
                },
                exclude,
                debounce: std::time::Duration::from_millis(debounce_ms),
                ..Default::default()
            };
            commands::watch(
                &name,
                &args,
                &cli.vars,
                selection,
                options,
                interval_ms,
                cli.verbose,
                cli.debug,
                cli.timeout_global,
                cli.log,
                cli.report,
            )
            .await
        }
        Some(Command::Ask { message }) => {
            commands::ask(&message).await?;
            Ok(0)
        }
        Some(Command::Completion { shell }) => {
            let mut cmd = Cli::command();
            let bin_name = cmd.get_name().to_string();
            generate(shell, &mut cmd, bin_name, &mut std::io::stdout());
            Ok(0)
        }
        Some(Command::Run(args)) => {
            let mut iter = args.iter();
            let name = iter
                .next()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.is_empty() {
                bail!(zek_core::lang::messages().err_missing_name);
            }
            let rest: Vec<String> = iter.map(|s| s.to_string_lossy().into_owned()).collect();
            commands::run_by_name(
                &name,
                &rest,
                &cli.vars,
                selection,
                false,
                cli.dry_run,
                cli.verbose,
                cli.debug,
                cli.timeout_global,
                cli.log,
                cli.report,
            )
            .await
        }
        None => {
            commands::run_default()?;
            Ok(0)
        }
    }
}
