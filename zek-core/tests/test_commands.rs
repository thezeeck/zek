mod common;

use std::time::Duration;

use common::Workspace;
use zek_core::commands;
use zek_core::exec::CommandExecutor;

#[tokio::test]
async fn ejecuta_comando_nombrado() {
    let ws = Workspace::new();
    ws.write_command(
        "build",
        "name: build\ndescription: Compila\nrun: echo compilando\n",
    );

    let cmds = commands::load_all(&ws.commands_dir()).unwrap();
    let status = CommandExecutor::new(cmds["build"].command.run.clone())
        .execute()
        .await;

    assert!(status.is_success());
    assert_eq!(status.stdout().trim(), "compilando");
}

#[tokio::test]
async fn ejecuta_comando_que_falla() {
    let ws = Workspace::new();
    ws.write_command("fail", "name: fail\nrun: exit 5\n");

    let cmds = commands::load_all(&ws.commands_dir()).unwrap();
    let status = CommandExecutor::new(cmds["fail"].command.run.clone())
        .execute()
        .await;

    assert!(status.is_failed());
    assert_eq!(status.exit_code(), Some(5));
}

#[tokio::test]
async fn respeta_timeout_del_comando() {
    let ws = Workspace::new();
    ws.write_command("slow", "name: slow\nrun: sleep 5\ntimeout: 1\n");

    let cmds = commands::load_all(&ws.commands_dir()).unwrap();
    let cmd = &cmds["slow"].command;
    let status = CommandExecutor::new(cmd.run.clone())
        .timeout(Duration::from_secs(cmd.timeout as u64))
        .execute()
        .await;

    assert!(status.is_timed_out());
}
