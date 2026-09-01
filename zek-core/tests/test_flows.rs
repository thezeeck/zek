mod common;

use std::collections::HashMap;

use common::{fixture, Workspace};
use zek_core::commands;
use zek_core::error::FlowFinalStatus;
use zek_core::execution::FlowRunner;
use zek_core::flows::{self, Flow};

#[tokio::test]
async fn flujo_valido_corre_exitoso() {
    let (flow, warnings) = Flow::load_with_validation(&fixture("valid_flow.yaml")).unwrap();
    assert!(warnings.is_empty());

    let commands = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let runner = FlowRunner::new(&flow, &commands, tmp.path().to_path_buf(), false);
    let report = runner.run().await.unwrap();

    assert_eq!(report.status, FlowFinalStatus::Success);
    assert!(report.failed_steps.is_empty());
}

#[test]
fn flujo_con_goto_invalido_falla_al_cargar() {
    let err = Flow::load_with_validation(&fixture("invalid_goto.yaml")).unwrap_err();
    assert!(err.to_string().contains("nonexistent"));
}

#[test]
fn flujo_con_ciclo_estatico_avisa() {
    let (_, warnings) = Flow::load_with_validation(&fixture("static_cycle.yaml")).unwrap();
    assert!(warnings.iter().any(|w| w.contains("cycle")));
}

#[tokio::test]
async fn flujo_con_loop_infinito_se_aborta() {
    let (flow, _) = Flow::load_with_validation(&fixture("infinite_loop.yaml")).unwrap();

    let commands = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let runner = FlowRunner::new(&flow, &commands, tmp.path().to_path_buf(), false);
    let report = runner.run().await.unwrap();

    assert_eq!(report.status, FlowFinalStatus::Aborted);
    assert!(report.exit_reason.contains("infinite_loop"));
}

#[tokio::test]
async fn flujo_con_comando_nombrado_y_finally() {
    let ws = Workspace::new();
    ws.write_command("build", "name: build\nrun: echo compilando\n");
    ws.write_flow(
        "deploy",
        "name: deploy\nsteps:\n  - name: build\n    type: command\n    command: build\nfinally:\n  steps:\n    - name: cleanup\n      type: command\n      command: echo limpiando\n",
    );

    let cmds = commands::load_all(&ws.commands_dir()).unwrap();
    let flows = flows::load_all(&ws.flows_dir()).unwrap();
    let loaded = &flows["deploy"];

    let runner = FlowRunner::new(&loaded.flow, &cmds, ws.workdir.clone(), false);
    let report = runner.run().await.unwrap();

    assert_eq!(report.status, FlowFinalStatus::Success);
    assert!(report.results.get("cleanup").unwrap().status.is_success());
}
