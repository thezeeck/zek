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
    about = "Terminal workflow orchestrator con Claude y OpenCode"
)]
struct Cli {
    /// Previsualizar el plan de ejecución sin correrlo
    #[arg(long, global = true)]
    dry_run: bool,

    /// Logs detallados
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Debug extra (rutas, timestamps)
    #[arg(long, global = true)]
    debug: bool,

    /// Timeout global en segundos para todo el flujo
    #[arg(long, global = true)]
    timeout_global: Option<u64>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Configura zek por primera vez (o re-configura)
    Init {
        /// Carpeta de trabajo (si se omite, se pregunta interactivamente)
        dir: Option<PathBuf>,
    },
    /// Muestra o modifica la configuración
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Lista los comandos y flujos disponibles
    List,
    /// Muestra un comando específico
    Commands {
        /// Nombre del comando
        name: String,
    },
    /// Pregunta algo a Claude fuera de flujos
    Ask {
        /// Mensaje a enviar a claude -p
        message: String,
    },
    /// Genera el script de completions para un shell
    Completion {
        /// Shell objetivo
        #[arg(long, value_enum)]
        shell: Shell,
    },
    /// Ejecuta un flujo o comando por nombre
    #[command(external_subcommand)]
    Run(Vec<OsString>),
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Muestra la configuración actual
    Show,
    /// Cambia la carpeta de trabajo
    SetDir {
        /// Nueva carpeta de trabajo (debe contener commands/ y flows/)
        path: PathBuf,
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
            let name = args
                .first()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.is_empty() {
                bail!("falta el nombre del flujo o comando");
            }
            commands::run_by_name(
                &name,
                cli.dry_run,
                cli.verbose,
                cli.debug,
                cli.timeout_global,
            )
            .await
        }
        None => {
            commands::run_default()?;
            Ok(0)
        }
    }
}
