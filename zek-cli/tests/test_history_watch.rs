#![cfg(unix)]
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
struct Workspace {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
}
impl Workspace {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        let config = tmp.path().join("config");
        for path in [
            root.join("commands"),
            root.join("flows"),
            config.join("zek"),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(config.join("zek/config.yaml"),json!({"workdir":root,"history":{"directory":root.join("records"),"retention_days":30,"max_runs":100}}).to_string()).unwrap();
        Self {
            _tmp: tmp,
            root,
            config,
        }
    }
    fn flow(&self, command: &str) {
        fs::write(self.root.join("flows/sample.yaml"),json!({"name":"sample","steps":[{"name":"work","type":"command","command":command,"cwd":"."}]}).to_string()).unwrap();
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_zek"));
        cmd.current_dir(&self.root)
            .env("XDG_CONFIG_HOME", &self.config)
            .args(args)
            .stdin(Stdio::null());
        cmd
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    fn history(&self) -> Vec<Value> {
        let out = self.run(&["history", "--json"]);
        assert!(out.status.success(), "{out:?}");
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn wait(&self, mut condition: impl FnMut() -> bool) {
        let start = Instant::now();
        while !condition() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "Timed out; history: {:?}",
                self.history()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn watch(&self) -> Child {
        self.command(&[
            "watch",
            "sample",
            "--debounce-ms",
            "60",
            "--interval-ms",
            "20",
            "--exclude",
            "runs.txt",
            "--exclude",
            "active",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn interrupt(child: &mut Child) {
    assert!(Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap()
        .success());
}
#[test]
fn cli_history_filters_and_logs_match_report_run_id() {
    let ws = Workspace::new();
    ws.flow("echo okay");
    let out = ws.run(&["--report", "json", "sample"]);
    assert!(out.status.success(), "{out:?}");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = report["run_id"].as_str().unwrap();
    let out = ws.run(&[
        "history", "--flow", "sample", "--status", "success", "--limit", "1", "--json",
    ]);
    assert!(out.status.success());
    let runs: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["run_id"], id);
    let logs = ws.run(&["logs", id]);
    assert!(logs.status.success());
    let text = String::from_utf8(logs.stdout).unwrap();
    let events: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.first().unwrap()["kind"], "run_started");
    assert_eq!(events.last().unwrap()["status"], "success");
    assert!(!ws.run(&["logs", "../outside"]).status.success());
    assert!(ws.run(&["--dry-run", "sample"]).status.success());
    assert_eq!(ws.history().len(), 1);
}
#[test]
fn watch_coalesces_active_changes_ignores_history_and_recovers_invalid_yaml() {
    let ws = Workspace::new();
    let action = "printf 'run\\n' >> runs.txt; touch active; sleep 0.4; rm active";
    ws.flow(action);
    let mut process = Process(ws.watch());
    ws.wait(|| ws.root.join("active").exists());
    fs::write(ws.root.join("input.txt"), "first").unwrap();
    thread::sleep(Duration::from_millis(30));
    fs::write(ws.root.join("input.txt"), "second").unwrap();
    ws.wait(|| {
        ws.history()
            .iter()
            .filter(|r| r["status"] == "success")
            .count()
            == 2
    });
    thread::sleep(Duration::from_millis(150));
    assert_eq!(
        fs::read_to_string(ws.root.join("runs.txt"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    fs::write(ws.root.join("flows/sample.yaml"), "name: [broken").unwrap();
    ws.wait(|| ws.history().iter().any(|r| r["status"] == "error"));
    assert!(process.0.try_wait().unwrap().is_none());
    ws.flow(action);
    ws.wait(|| {
        ws.history()
            .iter()
            .filter(|r| r["status"] == "success")
            .count()
            == 3
    });
    interrupt(&mut process.0);
    let start = Instant::now();
    let status = loop {
        if let Some(status) = process.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "watch failed to exit after SIGINT"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(130));
    assert_eq!(
        fs::read_to_string(ws.root.join("runs.txt"))
            .unwrap()
            .lines()
            .count(),
        3
    );
    assert_eq!(ws.history().len(), 4);
}
#[test]
fn watch_interrupt_kills_active_work_and_records_cancellation() {
    let ws = Workspace::new();
    ws.flow("touch active; sleep 1; touch forbidden");
    let mut process = Process(ws.watch());
    ws.wait(|| ws.root.join("active").exists());
    interrupt(&mut process.0);
    let start = Instant::now();
    let status = loop {
        if let Some(status) = process.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "watch failed to exit after SIGINT"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(130));
    assert!(ws.history().iter().any(|r| r["status"] == "cancelled"));
    thread::sleep(Duration::from_millis(1100));
    assert!(!ws.root.join("forbidden").exists());
}
#[test]
fn global_timeout_is_distinct_from_interrupt_in_history() {
    let ws = Workspace::new();
    ws.flow("sleep 5; touch forbidden");
    let out = ws.run(&["--timeout-global", "1", "sample"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(ws.history()[0]["status"], "aborted");
    assert!(!ws.root.join("forbidden").exists());
}
