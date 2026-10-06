use serde_json::json;
use std::{collections::HashMap, fs, path::Path, sync::Arc};
use zek_core::{execution::FlowRunner, flows::Flow, history::HistoryStore};
fn store(tmp: &tempfile::TempDir) -> HistoryStore {
    HistoryStore::new(tmp.path().join("history"), 30, 1000)
}
#[test]
fn concurrent_records_have_unique_ids_isolated_events_and_contiguous_sequences() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(store(&tmp));
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let store = store.clone();
            std::thread::spawn(move || {
                let flow = format!("flow-{i}");
                let run = store.start(&flow).unwrap();
                for attempt in 1..=5 {
                    run.event(
                        &flow,
                        Some("step"),
                        Some(attempt),
                        "attempt_started",
                        Some("running"),
                        None,
                        None,
                    )
                    .unwrap();
                }
                run.finish("success", None).unwrap();
                run.id()
            })
        })
        .collect();
    let ids: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        8
    );
    for (i, id) in ids.iter().enumerate() {
        let events = store.events(id).unwrap();
        assert_eq!(events.len(), 7);
        for (sequence, event) in events.iter().enumerate() {
            assert_eq!(event.sequence, sequence as u64);
            assert_eq!(&event.run_id, id);
            assert_eq!(event.flow, format!("flow-{i}"));
            assert_eq!(event.schema_version, 1);
        }
    }
    assert_eq!(
        store
            .list(Some("flow-2"), Some("success"), 2)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(store.list(None, None, 3).unwrap().len(), 3);
}
#[test]
fn incomplete_summary_and_truncated_event_tail_preserve_readable_history() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp);
    let run = store.start("broken").unwrap();
    let id = run.id();
    run.finish("success", None).unwrap();
    drop(run);
    let dir = store.directory.join(&id);
    fs::write(dir.join("summary.json"), b"{unfinished").unwrap();
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(dir.join("events.jsonl"))
        .unwrap()
        .write_all(b"{unfinished")
        .unwrap();
    assert_eq!(store.events(&id).unwrap().len(), 2);
    let summary = store.summary(&id).unwrap();
    assert_eq!(summary.flow, "broken");
    assert_eq!(summary.status, "success");
    assert!(summary.incomplete);
    assert_eq!(store.list(None, None, 20).unwrap().len(), 1);
    fs::write(
        dir.join("events.jsonl"),
        format!(
            "{}\n",
            serde_json::to_string(&store.events(&id).unwrap()[0]).unwrap()
        ),
    )
    .unwrap();
    assert_eq!(store.summary(&id).unwrap().status, "incomplete");
}
#[test]
fn guard_drop_records_cancellation_and_finishing_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp);
    let id = {
        let run = store.start("cancelled").unwrap();
        run.id()
    };
    assert_eq!(store.summary(&id).unwrap().status, "cancelled");
    let run = store.start("done").unwrap();
    let id = run.id();
    run.finish("success", None).unwrap();
    run.finish("failed", None).unwrap();
    assert!(run
        .event("done", None, None, "late", None, None, None)
        .is_err());
    drop(run);
    assert_eq!(
        store
            .events(&id)
            .unwrap()
            .iter()
            .filter(|e| e.kind == "run_finished")
            .count(),
        1
    );
    assert_eq!(store.summary(&id).unwrap().status, "success");
}
#[test]
fn retention_prunes_only_completed_history_and_rejects_path_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let store = HistoryStore::new(tmp.path().join("history"), 0, 1);
    let old = store.start("old").unwrap();
    let old_id = old.id();
    old.finish("success", None).unwrap();
    drop(old);
    let active = store.start("active").unwrap();
    let active_id = active.id();
    let recent = store.start("recent").unwrap();
    let recent_id = recent.id();
    recent.finish("failed", None).unwrap();
    drop(recent);
    store.prune().unwrap();
    assert!(!store.directory.join(old_id).exists());
    assert!(store.directory.join(&active_id).exists());
    assert!(store.directory.join(recent_id).exists());
    for id in ["../outside", "/tmp", "", "..", "abc/def"] {
        assert!(store.events(id).is_err());
    }
    active.finish("success", None).unwrap();
}
#[tokio::test]
async fn runner_records_attempts_and_errors_without_process_output_or_vars() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp);
    let commands = HashMap::new();
    let flow=Flow::from_str(&json!({"name":"recorded","vars":{"secret":"never-persist-me"},"steps":[{"name":"show","type":"command","command":"echo {{vars.secret}}"},{"name":"skip","type":"command","command":"echo ignored","when":"false"}]}).to_string(),Path::new("flow.yaml")).unwrap();
    let report = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
        .with_history(store.clone())
        .run()
        .await
        .unwrap();
    let id = report.run_id.as_ref().unwrap();
    let events = store.events(id).unwrap();
    assert!(events
        .iter()
        .any(|e| e.kind == "attempt_started" && e.attempt == Some(1)));
    assert!(events.iter().any(|e| e.kind == "step_skipped"));
    assert_eq!(events.last().unwrap().status.as_deref(), Some("success"));
    let contents = fs::read_to_string(store.directory.join(id).join("events.jsonl")).unwrap();
    assert!(!contents.contains("never-persist-me"));
    let mut invalid = flow.clone();
    invalid.execution = zek_core::plan::ExecutionMode::Dag;
    invalid.steps[0].needs = vec!["missing".into()];
    assert!(
        FlowRunner::new(&invalid, &commands, tmp.path().into(), false)
            .with_history(store.clone())
            .run()
            .await
            .is_err()
    );
    let errors = store.list(Some("recorded"), Some("error"), 10).unwrap();
    assert_eq!(errors.len(), 1);
    assert!(store
        .events(&errors[0].run_id)
        .unwrap()
        .iter()
        .any(|e| e.kind == "internal_error"));
}
#[cfg(unix)]
#[tokio::test]
async fn cancellation_of_active_runner_is_recorded_and_processes_are_killed() {
    use std::time::Duration;
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp);
    let commands = HashMap::new();
    let flow=Flow::from_str("name: cancel\nsteps:\n - name: long\n   type: command\n   command: sleep 1; touch forbidden\n   cwd: .\n",Path::new("flow.yaml")).unwrap();
    let runner =
        FlowRunner::new(&flow, &commands, tmp.path().into(), false).with_history(store.clone());
    assert!(
        tokio::time::timeout(Duration::from_millis(50), runner.run())
            .await
            .is_err()
    );
    let runs = store.list(None, Some("cancelled"), 10).unwrap();
    assert_eq!(runs.len(), 1);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(!tmp.path().join("forbidden").exists());
}

#[test]
fn history_configuration_defaults_and_project_relative_directory_are_explicit() {
    use zek_core::config::{Config, ProjectConfig};
    let tmp = tempfile::tempdir().unwrap();
    let mut config = Config::new(tmp.path().into());
    assert_eq!(config.history.retention_days, 30);
    assert_eq!(config.history.max_runs, 1000);
    config.history.directory = Some("records".into());
    assert_eq!(
        config.history_store().unwrap().directory,
        tmp.path().join("records")
    );
    let project: ProjectConfig = serde_yaml::from_str(
        "history:\n  directory: .zek/history\n  retention_days: 7\n  max_runs: 20\n",
    )
    .unwrap();
    let base = tmp.path().join("project");
    config.apply_project(project, &base);
    assert_eq!(
        config.history_store().unwrap().directory,
        base.join(".zek/history")
    );
    assert_eq!(config.history.retention_days, 7);
    assert_eq!(config.history.max_runs, 20);
}

#[test]
fn retention_age_removes_old_finished_runs_but_preserves_incomplete_records() {
    let tmp = tempfile::tempdir().unwrap();
    let store = HistoryStore::new(tmp.path().join("history"), 1, 0);
    let run = store.start("old").unwrap();
    let id = run.id();
    run.finish("success", None).unwrap();
    drop(run);
    let mut summary = store.summary(&id).unwrap();
    summary.finished_at_ms = Some(0);
    fs::write(
        store.directory.join(&id).join("summary.json"),
        serde_json::to_vec(&summary).unwrap(),
    )
    .unwrap();
    store.prune().unwrap();
    assert!(!store.directory.join(id).exists());
    let run = store.start("incomplete").unwrap();
    let id = run.id();
    run.finish("success", None).unwrap();
    drop(run);
    let mut summary = store.summary(&id).unwrap();
    summary.finished_at_ms = Some(0);
    summary.incomplete = true;
    fs::write(
        store.directory.join(&id).join("summary.json"),
        serde_json::to_vec(&summary).unwrap(),
    )
    .unwrap();
    store.prune().unwrap();
    assert!(store.directory.join(id).exists());
}

#[cfg(unix)]
#[tokio::test]
async fn retries_have_distinct_attempt_numbers_and_record_their_outcomes() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp);
    let commands = HashMap::new();
    let flow=Flow::from_str("name: retry\nsteps:\n - name: flaky\n   type: command\n   command: if [ -f retried ]; then echo okay; else touch retried; exit 1; fi\n   cwd: .\n   retries: 1\n",Path::new("flow.yaml")).unwrap();
    let report = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
        .with_history(store.clone())
        .run()
        .await
        .unwrap();
    let events = store.events(report.run_id.as_ref().unwrap()).unwrap();
    let attempts: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "attempt_finished")
        .map(|e| (e.attempt, e.status.as_deref(), e.exit_code))
        .collect();
    assert_eq!(
        attempts,
        vec![
            (Some(1), Some("failed"), Some(1)),
            (Some(2), Some("success"), Some(0))
        ]
    );
}
