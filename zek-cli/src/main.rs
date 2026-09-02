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

    #[command(subcommand)]
    command: Option<Command>,
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
                cli.dry_run,
                cli.verbose,
                cli.debug,
                cli.timeout_global,
                cli.log,
            )
            .await
        }
        None => {
            commands::run_default()?;
            Ok(0)
        }
    }
}
