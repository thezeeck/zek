mod common;

use std::collections::HashMap;
use std::path::Path;
use zek_core::context::ExecutionContext;
use zek_core::error::FlowFinalStatus;
use zek_core::execution::{FlowRunner, MAX_FLOW_DEPTH};
use zek_core::flows::{self, Flow, LoadedFlow};

fn parse(yaml: &str) -> Flow {
    Flow::from_str(yaml, Path::new("test.yaml")).unwrap()
}

fn catalog(items: Vec<Flow>) -> HashMap<String, LoadedFlow> {
    items
        .into_iter()
        .map(|flow| {
            (
                flow.name.clone(),
                LoadedFlow {
                    source: format!("{}.yaml", flow.name).into(),
                    flow,
                    warnings: Vec::new(),
                },
            )
        })
        .collect()
}

#[test]
fn templates_preserve_shell_and_prompt_characters() {
    let value = "a&b\"<c>'=/";
    let mut ctx = ExecutionContext::new();
    ctx.set_args(HashMap::from([("value".into(), value.into())]));
    assert_eq!(ctx.render("{{args.value}}").unwrap(), value);
}

#[tokio::test]
async fn finally_runs_after_condition_template_and_missing_flow_errors() {
    for step in [
        "type: command\n    command: echo unused\n    when: '=='",
        "type: command\n    command: '{{#if}}'",
        "type: flow\n    flow: missing",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let flow = parse(&format!(
            "name: main\nsteps:\n  - name: bad\n    {step}\nfinally:\n  steps:\n    - name: cleanup\n      type: command\n      command: echo '{{{{flow.status}}}}' > cleaned\n      cwd: .\n"
        ));
        let commands = HashMap::new();
        let flows = HashMap::new();
        let err = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
            .with_flows(&flows)
            .run()
            .await
            .unwrap_err();
        assert!(!err.to_string().is_empty());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("cleaned"))
                .unwrap()
                .trim()
                .trim_matches('\''),
            "failed"
        );
    }
}

#[tokio::test]
async fn finally_preserves_main_error_when_cleanup_also_errors() {
    let flow = parse(
        "name: main\nsteps:\n  - name: bad\n    type: command\n    command: echo unused\n    when: '=='\nfinally:\n  steps:\n    - name: cleanup\n      type: command\n      command: '{{#if}}'\n",
    );
    let commands = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let err = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
        .run()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("when"));
}

#[tokio::test]
async fn finally_failure_updates_report_context() {
    let flow = parse(
        "name: main\nsteps: []\nfinally:\n  fail_flow_on_error: true\n  steps:\n    - name: cleanup\n      type: command\n      command: exit 1\n",
    );
    let commands = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let report = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
        .run()
        .await
        .unwrap();
    assert_eq!(report.status, FlowFinalStatus::Failed);
    assert_eq!(report.results.flow_status, Some(report.status));
    assert_eq!(report.results.exit_reason, report.exit_reason);
}

#[test]
fn loading_rejects_direct_indirect_and_finally_recursion() {
    for (a, b) in [
        (
            "name: a\nsteps:\n  - name: self\n    type: flow\n    flow: a\n",
            None,
        ),
        (
            "name: a\nsteps:\n  - name: b\n    type: flow\n    flow: b\n",
            Some("name: b\nsteps:\n  - name: a\n    type: flow\n    flow: a\n"),
        ),
        (
            "name: a\nfinally:\n  steps:\n    - name: self\n      type: flow\n      flow: a\n",
            None,
        ),
    ] {
        let ws = common::Workspace::new();
        ws.write_flow("a", a);
        if let Some(b) = b {
            ws.write_flow("b", b);
        }
        let err = flows::load_all(&ws.flows_dir()).unwrap_err();
        assert!(err.to_string().contains("recursive"));
    }
}

#[tokio::test]
async fn runtime_recursion_guard_still_runs_parent_cleanup() {
    let tmp = tempfile::tempdir().unwrap();
    let flow = parse("name: a\nsteps:\n  - name: call\n    type: flow\n    flow: b\nfinally:\n  steps:\n    - name: cleanup\n      type: command\n      command: echo cleaned > cleaned\n      cwd: .\n");
    let child = parse("name: b\nsteps:\n  - name: back\n    type: flow\n    flow: a\n");
    let flows = catalog(vec![flow.clone(), child]);
    let commands = HashMap::new();
    let err = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
        .with_flows(&flows)
        .run()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("a -> b -> a"));
    assert!(tmp.path().join("cleaned").exists());
}

#[tokio::test]
async fn runtime_limits_acyclic_nesting_depth() {
    let flows = catalog(
        (0..=MAX_FLOW_DEPTH)
            .map(|i| {
                if i == MAX_FLOW_DEPTH {
                    parse(&format!("name: f{i}\nsteps: []\n"))
                } else {
                    parse(&format!(
                        "name: f{i}\nsteps:\n  - name: call{i}\n    type: flow\n    flow: f{}\n",
                        i + 1
                    ))
                }
            })
            .collect(),
    );
    let commands = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    flows::validate_flow_cycles(&flows).unwrap();
    let err = FlowRunner::new(&flows["f0"].flow, &commands, tmp.path().into(), false)
        .with_flows(&flows)
        .run()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("depth"));

    let mut flows = flows;
    flows
        .get_mut(&format!("f{}", MAX_FLOW_DEPTH - 1))
        .unwrap()
        .flow
        .steps
        .clear();
    let report = FlowRunner::new(&flows["f0"].flow, &commands, tmp.path().into(), false)
        .with_flows(&flows)
        .run()
        .await
        .unwrap();
    assert_eq!(report.status, FlowFinalStatus::Success);
}

#[tokio::test]
async fn confirmation_requires_a_callback() {
    let flow = parse("name: f\nsteps:\n  - name: guarded\n    type: command\n    command: echo ran\n    confirm: true\n");
    let commands = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let runner = FlowRunner::new(&flow, &commands, tmp.path().into(), false);
    let report = runner.run().await.unwrap();
    assert!(report.results.get("guarded").is_none());
    assert_eq!(report.skipped_steps, ["guarded"]);
    let report = runner
        .on_confirm(std::sync::Arc::new(|_| true))
        .run()
        .await
        .unwrap();
    assert!(report.results.get("guarded").unwrap().status.is_success());
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::time::{Duration, Instant};
    use zek_core::exec::CommandExecutor;

    #[tokio::test]
    async fn timeout_includes_pipes_after_shell_exits() {
        let start = Instant::now();
        let status = CommandExecutor::new("echo ready; sleep 2 &")
            .timeout(Duration::from_millis(100))
            .execute()
            .await;
        assert!(status.is_timed_out());
        assert_eq!(status.stdout().trim(), "ready");
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn timeout_and_external_cancellation_kill_descendants() {
        for cancel in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let executor = CommandExecutor::new(
                "(echo ready > ready; sleep 0.3; echo survived > survived) & wait",
            )
            .cwd(tmp.path().into())
            .timeout(if cancel {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(100)
            });
            if cancel {
                assert!(
                    tokio::time::timeout(Duration::from_millis(100), executor.execute())
                        .await
                        .is_err()
                );
            } else {
                assert!(executor.execute().await.is_timed_out());
            }
            assert!(tmp.path().join("ready").exists());
            tokio::time::sleep(Duration::from_millis(400)).await;
            assert!(!tmp.path().join("survived").exists());
        }
    }

    const COUNTER_SCRIPT: &str = "count=$(cat \"$COUNTER\" 2>/dev/null || echo 0); count=$((count+1)); echo $count > \"$COUNTER\"; [ $count -ge 2 ]";

    #[tokio::test]
    async fn ai_steps_retry_with_delay_and_stop_at_limit() {
        for kind in ["claude", "opencode"] {
            for succeed in [false, true] {
                let tmp = tempfile::tempdir().unwrap();
                let body = if succeed {
                    COUNTER_SCRIPT.to_string()
                } else {
                    format!("{COUNTER_SCRIPT}; exit 1")
                };
                let program = common::write_fake_claude(tmp.path(), &body);
                let delay = u32::from(succeed);
                let flow = parse(&format!(
                    "name: f\nsteps:\n  - name: ai\n    type: {kind}\n    prompt: hello\n    retries: 1\n    retry_delay: {delay}\n    env:\n      COUNTER: {}\n",
                    serde_json::to_string(&tmp.path().join("counter")).unwrap()
                ));
                let commands = HashMap::new();
                let runner = if kind == "claude" {
                    FlowRunner::with_claude(
                        &flow,
                        &commands,
                        tmp.path().into(),
                        false,
                        program.to_str().unwrap(),
                    )
                } else {
                    FlowRunner::with_opencode(
                        &flow,
                        &commands,
                        tmp.path().into(),
                        false,
                        program.to_str().unwrap(),
                    )
                };
                let report = runner.run().await.unwrap();
                let result = report.results.get("ai").unwrap();
                assert_eq!(result.status.is_success(), succeed);
                assert_eq!(result.attempts, 2);
                assert_eq!(
                    std::fs::read_to_string(tmp.path().join("counter"))
                        .unwrap()
                        .trim(),
                    "2"
                );
                if succeed {
                    assert!(result.duration >= Duration::from_secs(1));
                }
            }
        }
    }

    #[tokio::test]
    async fn subflows_retry_without_stale_results_and_allow_repeated_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let flow = parse("name: parent\nsteps:\n  - name: first\n    type: flow\n    flow: child\n    retries: 1\n  - name: second\n    type: flow\n    flow: child\n");
        let child = parse(&format!(
            "name: child\nsteps:\n  - name: work\n    type: command\n    command: '{COUNTER_SCRIPT}'\n    when: \"'{{{{steps.work.status}}}}' != 'failed'\"\n    env:\n      COUNTER: {}\nfinally:\n  steps:\n    - name: cleanup\n      type: command\n      command: echo cleaned >> cleaned\n      cwd: .\n",
            serde_json::to_string(&tmp.path().join("counter")).unwrap()
        ));
        let flows = catalog(vec![child]);
        let commands = HashMap::new();
        let report = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
            .with_flows(&flows)
            .run()
            .await
            .unwrap();
        assert_eq!(report.status, FlowFinalStatus::Success);
        assert_eq!(report.results.get("first").unwrap().attempts, 2);
        assert_eq!(report.results.get("second").unwrap().attempts, 1);
        assert!(report.failed_steps.is_empty());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("cleaned"))
                .unwrap()
                .lines()
                .count(),
            3
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("counter"))
                .unwrap()
                .trim(),
            "3"
        );
    }

    #[tokio::test]
    async fn subflow_retries_stop_at_limit_and_run_cleanup_each_time() {
        let tmp = tempfile::tempdir().unwrap();
        let flow = parse("name: parent\nsteps:\n  - name: sub\n    type: flow\n    flow: child\n    retries: 1\n    retry_delay: 1\n");
        let child = parse("name: child\nsteps:\n  - name: work\n    type: command\n    command: exit 1\nfinally:\n  steps:\n    - name: cleanup\n      type: command\n      command: echo cleaned >> cleaned\n      cwd: .\n");
        let flows = catalog(vec![child]);
        let commands = HashMap::new();
        let report = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
            .with_flows(&flows)
            .run()
            .await
            .unwrap();
        assert_eq!(report.status, FlowFinalStatus::Failed);
        let result = report.results.get("sub").unwrap();
        assert_eq!(result.attempts, 2);
        assert!(result.duration >= Duration::from_secs(1));
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("cleaned"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }
}
