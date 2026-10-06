use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use zek_core::{
    flows::Flow,
    plan::{ExecutionMode, ExecutionPlan, Selection},
};
fn parse(value: Value) -> Flow {
    Flow::from_str(&value.to_string(), Path::new("plan.yaml")).unwrap()
}
fn diamond() -> Flow {
    parse(json!({"name":"diamond","execution":"dag","steps":[
        {"name":"root","type":"command","command":"printf root"},
        {"name":"left","type":"command","needs":["root"],"command":"printf left"},
        {"name":"right","type":"command","needs":["root"],"command":"printf right"},
        {"name":"join","type":"command","needs":["left","right"],"command":"printf '{{steps.left.stdout}}+{{steps.right.stdout}}'"}
    ]}))
}
#[test]
fn plans_select_dag_ancestors_and_sequential_prefix() {
    let flow = diamond();
    let plan = ExecutionPlan::build(&flow, &Selection::Until("left".into())).unwrap();
    assert_eq!(plan.selected, vec![0, 1]);
    assert_eq!(plan.excluded, vec!["right", "join"]);
    assert!(ExecutionPlan::build(&flow, &Selection::Step("join".into())).is_err());
    assert!(ExecutionPlan::build(&flow, &Selection::Until("missing".into())).is_err());
    let seq = parse(
        json!({"name":"seq","steps":[{"name":"a","type":"command"},{"name":"b","type":"command"},{"name":"c","type":"command"}]}),
    );
    assert_eq!(
        ExecutionPlan::build(&seq, &Selection::Until("b".into()))
            .unwrap()
            .selected,
        vec![0, 1]
    );
    assert_eq!(seq.execution, ExecutionMode::Sequential);
}
#[test]
fn rejects_invalid_dependencies_and_legacy_dag_controls() {
    for steps in [
        json!([{"name":"a","type":"command","needs":["missing"]}]),
        json!([{"name":"a","type":"command","needs":["a"]}]),
        json!([{"name":"a","type":"command","needs":["b"]},{"name":"b","type":"command","needs":["a"]}]),
        json!([{"name":"a","type":"command","parallel":true}]),
        json!([{"name":"a","type":"command","entry_only_via_goto":true}]),
        json!([{"name":"a","type":"command","on_success":"goto:b"},{"name":"b","type":"command"}]),
    ] {
        assert!(ExecutionPlan::build(
            &parse(json!({"name":"bad","execution":"dag","steps":steps})),
            &Selection::All
        )
        .is_err());
    }
    assert!(ExecutionPlan::build(
        &parse(json!({"name":"zero","execution":"dag","max_concurrency":0})),
        &Selection::All
    )
    .is_err());
    assert!(ExecutionPlan::build(
        &parse(json!({"name":"seq","steps":[{"name":"a","type":"command","needs":["x"]}]})),
        &Selection::All
    )
    .is_err());
}
#[test]
fn selection_rejects_missing_inputs_cleanup_and_external_gotos() {
    let flow = parse(
        json!({"name":"seq","steps":[{"name":"a","type":"command"},{"name":"b","type":"command","command":"echo {{steps.a.stdout}}"}]}),
    );
    let plan = ExecutionPlan::build(&flow, &Selection::Step("b".into())).unwrap();
    assert!(plan.validate_inputs(&flow, &HashMap::new()).is_err());
    let flow = parse(
        json!({"name":"seq","steps":[{"name":"a","type":"command","on_error":"goto:b"},{"name":"b","type":"command"}]}),
    );
    assert!(ExecutionPlan::build(&flow, &Selection::Until("a".into())).is_err());
    let flow = parse(
        json!({"name":"seq","steps":[{"name":"a","type":"command"},{"name":"b","type":"command"}],"finally":{"steps":[{"name":"clean","type":"command","command":"echo {{steps.b.stdout}}"}]}}),
    );
    assert!(ExecutionPlan::build(&flow, &Selection::Step("a".into()))
        .unwrap()
        .validate_inputs(&flow, &HashMap::new())
        .is_err());
}
#[test]
fn graph_is_stable_and_escapes_labels_and_describes_cleanup() {
    let flow = parse(
        json!({"name":"graph","steps":[{"name":"a\"\n[x]","type":"flow","flow":"child","parallel":false},{"name":"b","type":"command","on_error":"goto:a\"\n[x]"}],"finally":{"steps":[{"name":"clean","type":"command"}]}}),
    );
    let plan = ExecutionPlan::build(&flow, &Selection::All).unwrap();
    let graph = plan.graph(&flow, true);
    assert_eq!(graph, plan.graph(&flow, true));
    assert!(graph.contains("&quot;<br/>#91;x#93;"));
    assert!(graph.contains("goto (conditional)"));
    assert!(graph.contains("finally"));
    assert!(graph.contains("child"));
    let flow = diamond();
    let plan = ExecutionPlan::build(&flow, &Selection::All).unwrap();
    assert!(plan.graph(&flow, true).contains("s2 -->|\"needs\"| s3"));
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::sync::{Arc, Mutex};
    use zek_core::{
        error::FlowFinalStatus,
        execution::{FlowRunner, StepProgress},
        flows::LoadedFlow,
    };
    #[tokio::test]
    async fn diamond_executes_each_node_once_and_publishes_dependency_outputs() {
        let flow = diamond();
        let tmp = tempfile::tempdir().unwrap();
        let commands = HashMap::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let copy = events.clone();
        let report = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
            .on_progress(Arc::new(move |e| {
                if let StepProgress::Started { name, .. } = e {
                    copy.lock().unwrap().push(name);
                }
            }))
            .run()
            .await
            .unwrap();
        assert_eq!(report.status, FlowFinalStatus::Success);
        assert_eq!(events.lock().unwrap().len(), 4);
        assert_eq!(
            report.results.get("join").unwrap().status.stdout(),
            "left+right"
        );
    }
    #[tokio::test]
    async fn ready_branches_overlap_within_limit() {
        for limit in [1, 2] {
            let flow = parse(
                json!({"name":"parallel","execution":"dag","max_concurrency":limit,"steps":[{"name":"a","type":"command","command":"sleep 0.05"},{"name":"b","type":"command","command":"sleep 0.05"},{"name":"c","type":"command","command":"sleep 0.05"}]}),
            );
            let tmp = tempfile::tempdir().unwrap();
            let commands = HashMap::new();
            let counts = Arc::new(Mutex::new((0usize, 0usize)));
            let copy = counts.clone();
            FlowRunner::new(&flow, &commands, tmp.path().into(), false)
                .on_progress(Arc::new(move |event| {
                    let mut c = copy.lock().unwrap();
                    match event {
                        StepProgress::Started { .. } => {
                            c.0 += 1;
                            c.1 = c.1.max(c.0);
                        }
                        StepProgress::Finished { .. } => c.0 -= 1,
                    }
                }))
                .run()
                .await
                .unwrap();
            assert_eq!(*counts.lock().unwrap(), (0, limit));
        }
    }
    #[tokio::test]
    async fn failed_and_skipped_prerequisites_skip_descendants_but_continue_independent_work() {
        let flow = parse(
            json!({"name":"skip","execution":"dag","steps":[{"name":"bad","type":"command","command":"exit 7","on_error":"continue"},{"name":"disabled","type":"command","command":"touch forbidden","when":"false"},{"name":"downstream","type":"command","needs":["bad"],"command":"touch forbidden"},{"name":"transitive","type":"command","needs":["downstream"],"command":"touch forbidden"},{"name":"skip_child","type":"command","needs":["disabled"],"command":"touch forbidden"},{"name":"independent","type":"command","command":"printf ok"}]}),
        );
        let tmp = tempfile::tempdir().unwrap();
        let commands = HashMap::new();
        let report = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
            .run()
            .await
            .unwrap();
        assert_eq!(report.status, FlowFinalStatus::Failed);
        assert!(report.results.get("independent").is_some());
        assert!(!tmp.path().join("forbidden").exists());
        for name in ["downstream", "transitive", "skip_child"] {
            assert_eq!(report.skip_reasons[name], "dependency");
        }
    }
    #[tokio::test]
    async fn stop_drains_active_branches_before_cleanup_and_prevents_new_launches() {
        let flow = parse(
            json!({"name":"stop","execution":"dag","max_concurrency":2,"steps":[{"name":"bad","type":"command","command":"exit 1"},{"name":"active","type":"command","command":"sleep 0.05; touch done","cwd":"."},{"name":"pending","type":"command","command":"touch forbidden","cwd":"."}],"finally":{"steps":[{"name":"cleanup","type":"command","command":"test -f done; touch cleaned","cwd":"."}]}}),
        );
        let tmp = tempfile::tempdir().unwrap();
        let commands = HashMap::new();
        let report = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
            .run()
            .await
            .unwrap();
        assert_eq!(report.exit_reason, "stop:bad");
        assert!(report.results.get("cleanup").unwrap().status.is_success());
        assert!(tmp.path().join("cleaned").exists());
        assert!(!tmp.path().join("forbidden").exists());
        assert_eq!(report.skip_reasons["pending"], "stop");
    }
    #[tokio::test]
    async fn concurrent_subflows_keep_colliding_child_results_isolated() {
        let child_a = parse(
            json!({"name":"child_a","steps":[{"name":"same","type":"command","command":"printf A"},{"name":"assert_a","type":"command","command":"test '{{steps.same.stdout}}' = A"}]}),
        );
        let child_b = parse(
            json!({"name":"child_b","steps":[{"name":"same","type":"command","command":"printf B"},{"name":"assert_b","type":"command","command":"test '{{steps.same.stdout}}' = B"}]}),
        );
        let flows = [child_a, child_b]
            .into_iter()
            .map(|flow| {
                (
                    flow.name.clone(),
                    LoadedFlow {
                        flow,
                        source: "test.yaml".into(),
                        warnings: vec![],
                    },
                )
            })
            .collect();
        let flow = parse(
            json!({"name":"parent","execution":"dag","steps":[{"name":"a","type":"flow","flow":"child_a"},{"name":"b","type":"flow","flow":"child_b"}]}),
        );
        let tmp = tempfile::tempdir().unwrap();
        let commands = HashMap::new();
        let report = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
            .with_flows(&flows)
            .run()
            .await
            .unwrap();
        assert_eq!(report.status, FlowFinalStatus::Success);
        assert_eq!(report.results.get("a::same").unwrap().status.stdout(), "A");
        assert_eq!(report.results.get("b::same").unwrap().status.stdout(), "B");
        assert!(report.results.get("same").is_none());
    }
    #[tokio::test]
    async fn partial_execution_preserves_cleanup_and_records_excluded_nodes() {
        let flow = parse(
            json!({"name":"partial","execution":"dag","steps":[{"name":"a","type":"command","command":"printf A"},{"name":"b","type":"command","needs":["a"],"command":"printf '{{steps.a.stdout}}B'"},{"name":"outside","type":"command","command":"touch forbidden","cwd":"."}],"finally":{"steps":[{"name":"cleanup","type":"command","command":"touch cleaned","cwd":"."}]}}),
        );
        let tmp = tempfile::tempdir().unwrap();
        let commands = HashMap::new();
        let report = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
            .with_selection(Selection::Until("b".into()))
            .run()
            .await
            .unwrap();
        assert_eq!(report.excluded_steps, vec!["outside"]);
        assert!(report.skipped_steps.is_empty());
        assert_eq!(report.results.get("b").unwrap().status.stdout(), "AB");
        assert!(tmp.path().join("cleaned").exists());
        assert!(!tmp.path().join("forbidden").exists());
    }
    #[tokio::test]
    async fn cycle_rejection_has_no_side_effects() {
        let flow = parse(
            json!({"name":"bad","execution":"dag","steps":[{"name":"a","type":"command","needs":["b"],"command":"touch forbidden","cwd":"."},{"name":"b","type":"command","needs":["a"],"command":"true"}]}),
        );
        let tmp = tempfile::tempdir().unwrap();
        let commands = HashMap::new();
        assert!(FlowRunner::new(&flow, &commands, tmp.path().into(), false)
            .run()
            .await
            .is_err());
        assert!(!tmp.path().join("forbidden").exists());
    }
}

#[test]
fn partial_input_validation_uses_handlebars_syntax() {
    for template in [
        "{{{@root.steps.a.stdout}}}",
        "{{steps/[a]/stdout}}",
        "{{#if steps.a.success}}yes{{/if}}",
        "{{lookup steps 'a'}}",
    ] {
        let flow = parse(
            json!({"name":"templates","steps":[{"name":"a","type":"command"},{"name":"b","type":"command","command":template}]}),
        );
        assert!(
            ExecutionPlan::build(&flow, &Selection::Step("b".into()))
                .unwrap()
                .validate_inputs(&flow, &HashMap::new())
                .is_err(),
            "{template}"
        );
    }
    for template in [r"\{{steps.a.stdout}}", "{{! steps.a.stdout }} literal"] {
        let flow = parse(
            json!({"name":"templates","steps":[{"name":"a","type":"command"},{"name":"b","type":"command","command":template}]}),
        );
        ExecutionPlan::build(&flow, &Selection::Step("b".into()))
            .unwrap()
            .validate_inputs(&flow, &HashMap::new())
            .unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn independent_processes_really_overlap() {
    use zek_core::execution::FlowRunner;
    let flow = parse(
        json!({"name":"overlap","execution":"dag","max_concurrency":2,"steps":[
            {"name":"a","type":"command","command":"touch a-ready; while [ ! -f b-ready ]; do sleep 0.01; done","cwd":".","timeout":2},
            {"name":"b","type":"command","command":"touch b-ready; while [ ! -f a-ready ]; do sleep 0.01; done","cwd":".","timeout":2}
        ]}),
    );
    let tmp = tempfile::tempdir().unwrap();
    let commands = HashMap::new();
    let report = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
        .run()
        .await
        .unwrap();
    assert_eq!(report.exit_code(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn dag_internal_errors_drain_active_processes_before_finally() {
    use zek_core::execution::FlowRunner;
    let flow = parse(json!({"name":"internal","execution":"dag","steps":[
        {"name":"active","type":"command","command":"sleep 0.03; touch done","cwd":"."},
        {"name":"bad","type":"command","command":"echo bad","when":"{{#if}}"}
    ],"finally":{"steps":[{"name":"cleanup","type":"command","command":"test -f done && touch cleaned","cwd":"."}]}}));
    let tmp = tempfile::tempdir().unwrap();
    let commands = HashMap::new();
    assert!(FlowRunner::new(&flow, &commands, tmp.path().into(), false)
        .run()
        .await
        .is_err());
    assert!(tmp.path().join("done").exists());
    assert!(tmp.path().join("cleaned").exists());
}

#[test]
fn partial_selection_rejects_inputs_from_parallel_siblings() {
    let flow = parse(json!({"name":"parallel","steps":[
        {"name":"a","type":"command","parallel":true,"command":"printf A"},
        {"name":"b","type":"command","parallel":true,"command":"echo {{steps.a.stdout}}"},
        {"name":"outside","type":"command","command":"echo outside"}
    ]}));
    let plan = ExecutionPlan::build(&flow, &Selection::Until("b".into())).unwrap();
    assert!(plan.validate_inputs(&flow, &HashMap::new()).is_err());
}

#[test]
fn overridden_command_environment_does_not_require_unused_results() {
    use zek_core::commands::{Command, LoadedCommand};
    let command = Command::from_str(
        &json!({"name":"reusable","run":"echo ready","env":{"VALUE":"{{steps.outside.stdout}}"}})
            .to_string(),
        Path::new("command.yaml"),
    )
    .unwrap();
    let commands = HashMap::from([(
        "reusable".into(),
        LoadedCommand {
            command,
            source: "command.yaml".into(),
        },
    )]);
    let flow = parse(
        json!({"name":"env","steps":[{"name":"a","type":"command","command":"reusable","env":{"VALUE":"override"}},{"name":"outside","type":"command"}]}),
    );
    ExecutionPlan::build(&flow, &Selection::Step("a".into()))
        .unwrap()
        .validate_inputs(&flow, &commands)
        .unwrap();
}
