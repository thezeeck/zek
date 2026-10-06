#![cfg(unix)]
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
struct Workspace {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
}
impl Workspace {
    fn new(execution: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        let config = tmp.path().join("config");
        for dir in [
            root.join("flows"),
            root.join("commands"),
            config.join("zek"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(
            config.join("zek/config.yaml"),
            json!({"workdir":root,"language":"en"}).to_string(),
        )
        .unwrap();
        let ws = Self {
            _tmp: tmp,
            root,
            config,
        };
        ws.flow(json!({"name":"sample","execution":execution,"steps":[
            {"name":"a","type":"command","command":"printf A"},
            {"name":"b","type":"command","command":"printf B","needs":if execution=="dag" {vec!["a"]} else {vec![]}},
            {"name":"outside","type":"command","command":"touch forbidden","cwd":"."}
        ],"finally":{"steps":[{"name":"clean","type":"command","command":"touch cleaned","cwd":"."}]}}));
        ws
    }
    fn flow(&self, value: Value) {
        std::fs::write(self.root.join("flows/sample.yaml"), value.to_string()).unwrap();
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_zek"))
            .current_dir(&self.root)
            .env("XDG_CONFIG_HOME", &self.config)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}
#[test]
fn graph_and_dry_run_never_execute_and_share_selected_nodes() {
    for mode in ["sequential", "dag"] {
        let ws = Workspace::new(mode);
        for args in [
            vec!["graph", "sample"],
            vec!["graph", "sample", "--format", "mermaid"],
            vec!["--dry-run", "sample"],
        ] {
            let output = ws.run(&args);
            assert!(output.status.success(), "{output:?}");
            let text = String::from_utf8(output.stdout).unwrap();
            assert!(text.contains("outside"));
            assert!(text.contains("finally"));
        }
        let graph = ws.run(&["--until", "b", "graph", "sample"]);
        let dry = ws.run(&["--until", "b", "--dry-run", "sample"]);
        assert!(graph.status.success(), "{graph:?}");
        assert_eq!(graph.stdout, dry.stdout);
        assert!(!ws.root.join("forbidden").exists());
        assert!(!ws.root.join("cleaned").exists());
    }
}
#[test]
fn until_and_step_execute_only_selected_nodes_and_keep_cleanup() {
    for mode in ["sequential", "dag"] {
        for selection in [vec!["--until", "b"], vec!["--step", "a"]] {
            let ws = Workspace::new(mode);
            let mut args = selection.clone();
            args.extend(["--report", "json", "sample"]);
            let output = ws.run(&args);
            assert!(output.status.success(), "{output:?}");
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            let names: Vec<_> = report["steps"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["name"].as_str().unwrap())
                .collect();
            assert_eq!(
                names,
                if selection[0] == "--until" {
                    vec!["a", "b", "clean"]
                } else {
                    vec!["a", "clean"]
                }
            );
            assert!(report["excluded_steps"]
                .as_array()
                .unwrap()
                .contains(&json!("outside")));
            assert_eq!(report["skipped_steps"], json!([]));
            assert!(!ws.root.join("forbidden").exists());
            assert!(ws.root.join("cleaned").exists());
        }
    }
}
#[test]
fn invalid_selection_and_dependencies_fail_before_any_process() {
    let ws = Workspace::new("dag");
    for args in [
        vec!["--step", "b", "sample"],
        vec!["--until", "missing", "sample"],
        vec!["--step", "a", "--until", "b", "sample"],
        vec!["--step", "a", "list"],
    ] {
        assert!(!ws.run(&args).status.success());
    }
    assert!(!ws.root.join("cleaned").exists());
    assert!(!ws.root.join("forbidden").exists());
    ws.flow(json!({"name":"sample","steps":[{"name":"a","type":"command","command":"touch forbidden","cwd":"."},{"name":"b","type":"command","command":"echo {{steps.a.stdout}}"}]}));
    for args in [
        vec!["--step", "b", "sample"],
        vec!["--step", "b", "--dry-run", "sample"],
    ] {
        let output = ws.run(&args);
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .contains("unavailable"));
    }
    assert!(!ws.root.join("forbidden").exists());
}
#[test]
fn excluded_and_condition_skipped_are_separate_in_both_reports() {
    let ws = Workspace::new("sequential");
    ws.flow(json!({"name":"sample","steps":[{"name":"disabled","type":"command","command":"touch forbidden","when":"false"},{"name":"outside","type":"command","command":"touch forbidden"}]}));
    let output = ws.run(&["--until", "disabled", "--report", "json", "sample"]);
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["excluded_steps"], json!(["outside"]));
    assert_eq!(report["skipped_steps"], json!(["disabled"]));
    let output = ws.run(&["--until", "disabled", "--report", "markdown", "sample"]);
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.contains("| outside | - | excluded"));
    assert!(report.contains("| disabled | - | skipped"));
}
#[test]
fn trailing_selection_flags_remain_flow_arguments() {
    let ws = Workspace::new("sequential");
    ws.flow(json!({"name":"sample","steps":[{"name":"show","type":"command","command":"printf '%s|%s' '{{args.step}}' '{{args.until}}'"}]}));
    let output = ws.run(&[
        "--report",
        "json",
        "sample",
        "--step",
        "legacy-step",
        "--until",
        "legacy-until",
    ]);
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["steps"][0]["stdout"], "legacy-step|legacy-until");
}

#[test]
fn partial_selection_checks_reusable_commands_and_nested_inputs_before_execution() {
    let ws = Workspace::new("sequential");
    std::fs::write(
        ws.root.join("commands/dependent.yaml"),
        json!({"name":"dependent","run":"echo {{steps.a.stdout}}"}).to_string(),
    )
    .unwrap();
    ws.flow(json!({"name":"sample","steps":[{"name":"a","type":"command","command":"touch forbidden","cwd":"."},{"name":"b","type":"command","command":"dependent"}]}));
    assert!(!ws.run(&["--step", "b", "sample"]).status.success());
    std::fs::write(ws.root.join("flows/child.yaml"), json!({"name":"child","steps":[{"name":"read","type":"command","command":"echo {{steps.a.stdout}}"}]}).to_string()).unwrap();
    ws.flow(json!({"name":"sample","steps":[{"name":"a","type":"command","command":"touch forbidden","cwd":"."},{"name":"b","type":"flow","flow":"child"}]}));
    assert!(!ws.run(&["--step", "b", "sample"]).status.success());
    assert!(!ws.root.join("forbidden").exists());
}

#[test]
fn sequential_selection_can_read_outputs_of_selected_subflows() {
    let ws = Workspace::new("sequential");
    std::fs::write(ws.root.join("flows/child.yaml"), json!({"name":"child","steps":[{"name":"child_output","type":"command","command":"printf child"}]}).to_string()).unwrap();
    ws.flow(json!({"name":"sample","steps":[{"name":"a","type":"flow","flow":"child"},{"name":"b","type":"command","command":"printf '{{steps.child_output.stdout}}'"},{"name":"outside","type":"command","command":"touch forbidden","cwd":"."}]}));
    let output = ws.run(&["--until", "b", "--report", "json", "sample"]);
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["steps"][2]["stdout"], "child");
    assert!(!ws.root.join("forbidden").exists());
}

#[test]
fn selection_diagnostics_follow_configured_language() {
    let ws = Workspace::new("dag");
    for (language, expected) in [
        ("en", "Unknown selected step"),
        ("es", "Paso seleccionado inexistente"),
    ] {
        std::fs::write(
            ws.config.join("zek/config.yaml"),
            json!({"workdir":ws.root,"language":language}).to_string(),
        )
        .unwrap();
        let output = ws.run(&["--step", "missing", "sample"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8(output.stderr).unwrap().contains(expected));
    }
    assert!(!ws.root.join("cleaned").exists());
}

#[test]
fn dag_partial_selection_can_read_scoped_child_outputs() {
    let ws = Workspace::new("dag");
    std::fs::write(ws.root.join("flows/child.yaml"),json!({"name":"child","steps":[{"name":"child_output","type":"command","command":"printf child"}]}).to_string()).unwrap();
    ws.flow(json!({"name":"sample","execution":"dag","steps":[{"name":"a","type":"flow","flow":"child"},{"name":"b","type":"command","needs":["a"],"command":"printf '{{steps.[a::child_output].stdout}}'"},{"name":"outside","type":"command","command":"touch forbidden","cwd":"."}]}));
    let output = ws.run(&["--until", "b", "--report", "json", "sample"]);
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let steps = report["steps"].as_array().unwrap();
    assert_eq!(
        steps.iter().find(|s| s["name"] == "b").unwrap()["stdout"],
        "child"
    );
    assert!(!ws.root.join("forbidden").exists());
}
