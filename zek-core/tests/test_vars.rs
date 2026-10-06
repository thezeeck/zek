use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value};
use zek_core::context::ExecutionContext;
use zek_core::flows::Flow;

fn parse(yaml: &str) -> Flow {
    Flow::from_str(yaml, Path::new("vars.yaml")).unwrap()
}

#[test]
fn yaml_variables_preserve_json_types_and_missing_vars_default_to_empty() {
    let flow = parse(
        "name: typed\nvars:\n  text: 'a&b'\n  number: 42\n  float: 1.5\n  enabled: false\n  empty: null\n  object: {nested: value}\n  items: [first, 2, true]\n",
    );
    assert_eq!(flow.vars["text"], json!("a&b"));
    assert_eq!(flow.vars["number"], json!(42));
    assert_eq!(flow.vars["float"], json!(1.5));
    assert_eq!(flow.vars["enabled"], json!(false));
    assert_eq!(flow.vars["empty"], Value::Null);
    assert_eq!(flow.vars["object"], json!({"nested": "value"}));
    assert_eq!(flow.vars["items"], json!(["first", 2, true]));
    assert!(parse("name: legacy\nsteps: []\n").vars.is_empty());
    assert!(Flow::from_str("name: bad\nvars: [1, 2]\n", Path::new("bad.yaml")).is_err());
    assert!(Flow::from_str("name: bad\nvars: {number: .nan}\n", Path::new("bad.yaml")).is_err());
    for yaml in [
        "name: bad\nvars: {number: [.inf]}\n",
        "name: bad\nvars: {1: value}\n",
        "name: bad\nvars: {nested: {1: value}}\n",
        "name: bad\nvars: {tagged: !custom value}\n",
    ] {
        assert!(
            Flow::from_str(yaml, Path::new("bad.yaml")).is_err(),
            "{yaml}"
        );
    }
}

#[test]
fn context_renders_nested_vars_without_changing_args_or_escaping_values() {
    let mut ctx = ExecutionContext::new();
    ctx.set_args(HashMap::from([("value".into(), "legacy".into())]));
    ctx.set_vars(HashMap::from([
        ("value".into(), json!("a&b\"<c>")),
        ("object".into(), json!({"nested": 3})),
        ("items".into(), json!(["first", "second"])),
        ("enabled".into(), json!(false)),
        ("empty".into(), Value::Null),
    ]));
    assert_eq!(
        ctx.render("{{vars.value}}|{{args.value}}|{{vars.object.nested}}|{{vars.items.1}}|{{vars.enabled}}|{{vars.empty}}|{{vars.missing}}").unwrap(),
        "a&b\"<c>|legacy|3|second|false||"
    );
    assert_eq!(ctx.vars()["object"], json!({"nested": 3}));
}

#[cfg(unix)]
mod unix {
    use super::*;
    use zek_core::error::FlowFinalStatus;
    use zek_core::execution::FlowRunner;
    use zek_core::flows::LoadedFlow;

    fn catalog(flows: Vec<Flow>) -> HashMap<String, LoadedFlow> {
        flows
            .into_iter()
            .map(|flow| {
                (
                    flow.name.clone(),
                    LoadedFlow {
                        flow,
                        source: "vars.yaml".into(),
                        warnings: Vec::new(),
                    },
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn subflows_without_vars_inherit_root_overrides() {
        let flow = parse("name: parent\nvars: {name: yaml}\nsteps:\n  - name: call\n    type: flow\n    flow: child\n");
        let child = parse("name: child\nsteps:\n  - name: inherited\n    type: command\n    command: 'printf %s {{vars.name}}'\n");
        let flows = catalog(vec![child]);
        let commands = HashMap::new();
        let tmp = tempfile::tempdir().unwrap();
        let report = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
            .with_flows(&flows)
            .with_vars(HashMap::from([("name".into(), json!("override"))]))
            .run()
            .await
            .unwrap();
        assert_eq!(
            report.results.get("inherited").unwrap().status.stdout(),
            "override"
        );
        assert_eq!(report.results.vars()["name"], json!("override"));
    }

    #[tokio::test]
    async fn vars_work_in_named_commands_environment_conditions_and_finally() {
        let tmp = tempfile::tempdir().unwrap();
        let flow = parse(
            r#"
name: root
vars:
  name: yaml
  enabled: false
  settings: {count: 1}
steps:
  - name: show
    type: command
    command: show
    when: '{{vars.enabled}} && {{vars.settings.count}} == 2'
  - name: skipped
    type: command
    command: echo unexpected
    when: '{{vars.missing}}'
finally:
  steps:
    - name: cleanup
      type: command
      command: 'printf "%s" "{{vars.name}}"'
"#,
        );
        std::fs::write(
            tmp.path().join("show.yaml"),
            r#"
name: show
run: 'printf "%s|%s|%s" "{{vars.name}}" "$VALUE" "{{args.name}}"'
env:
  VALUE: '{{vars.settings.count}}'
"#,
        )
        .unwrap();
        let commands = zek_core::commands::load_all(tmp.path()).unwrap();
        let overrides = HashMap::from([
            ("name".into(), json!("override")),
            ("enabled".into(), json!(true)),
            ("settings".into(), json!({"count": 2})),
        ]);
        let runner = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
            .with_vars(overrides.clone())
            .with_args(HashMap::from([("name".into(), "legacy".into())]));
        for _ in 0..2 {
            let report = runner.run().await.unwrap();
            assert_eq!(report.status, FlowFinalStatus::Success);
            assert_eq!(
                report.results.get("show").unwrap().status.stdout(),
                "override|2|legacy"
            );
            assert_eq!(
                report.results.get("cleanup").unwrap().status.stdout(),
                "override"
            );
            assert_eq!(report.skipped_steps, ["skipped"]);
            assert_eq!(report.results.vars(), &overrides);
        }
        assert_eq!(flow.vars["name"], json!("yaml"));
    }

    #[tokio::test]
    async fn nested_subflows_inherit_shadow_and_restore_vars_including_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let flow = parse(
            r#"
name: parent
vars:
  name: parent
  inherited: from-parent
  object: {parent: true}
steps:
  - name: call_child
    type: flow
    flow: child
  - name: after
    type: command
    command: 'printf "%s|%s|%s" "{{vars.name}}" "{{vars.child_only}}" "{{vars.object.parent}}"'
finally:
  steps:
    - name: parent_cleanup
      type: command
      command: 'printf "%s" "{{vars.name}}"'
"#,
        );
        let child = parse(
            r#"
name: child
vars:
  name: child
  child_only: local
  object: {child: true}
steps:
  - name: child_before
    type: command
    command: 'printf "%s|%s|%s|%s" "{{vars.name}}" "{{vars.inherited}}" "{{vars.object.child}}" "{{vars.object.parent}}"'
  - name: call_grandchild
    type: flow
    flow: grandchild
  - name: child_after
    type: command
    command: 'printf "%s|%s" "{{vars.name}}" "{{vars.child_only}}"'
finally:
  steps:
    - name: child_cleanup
      type: command
      command: 'printf "%s" "{{vars.name}}"'
"#,
        );
        let grandchild = parse(
            r#"
name: grandchild
vars: {name: grandchild}
steps:
  - name: deepest
    type: command
    command: 'printf "%s|%s|%s" "{{vars.name}}" "{{vars.inherited}}" "{{vars.child_only}}"'
"#,
        );
        let flows = catalog(vec![child, grandchild]);
        let commands = HashMap::new();
        let report = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
            .with_flows(&flows)
            .with_vars(HashMap::from([("name".into(), json!("cli"))]))
            .run()
            .await
            .unwrap();
        assert_eq!(report.status, FlowFinalStatus::Success);
        for (step, expected) in [
            ("child_before", "child|from-parent|true|"),
            ("deepest", "grandchild|from-parent|local"),
            ("child_after", "child|local"),
            ("child_cleanup", "child"),
            ("after", "cli||true"),
            ("parent_cleanup", "cli"),
        ] {
            assert_eq!(
                report.results.get(step).unwrap().status.stdout(),
                expected,
                "{step}"
            );
        }
        assert_eq!(report.results.vars()["name"], json!("cli"));
        assert!(!report.results.vars().contains_key("child_only"));
    }

    #[tokio::test]
    async fn parent_vars_survive_subflow_failures_and_internal_errors() {
        for command in ["exit 1", "{{#if}}"] {
            let tmp = tempfile::tempdir().unwrap();
            let flow = parse(
                r#"
name: parent
vars: {name: parent}
steps:
  - name: call
    type: flow
    flow: child
    on_error: continue
finally:
  steps:
    - name: parent_cleanup
      type: command
      command: 'printf "%s" "{{vars.name}}" > parent-cleanup'
      cwd: .
"#,
            );
            let child = parse(&format!(
                r#"
name: child
vars: {{name: child}}
steps:
  - name: fail
    type: command
    command: '{command}'
finally:
  steps:
    - name: child_cleanup
      type: command
      command: 'printf "%s" "{{{{vars.name}}}}" > child-cleanup'
      cwd: .
"#
            ));
            let flows = catalog(vec![child]);
            let commands = HashMap::new();
            let result = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
                .with_flows(&flows)
                .run()
                .await;
            if command == "exit 1" {
                let report = result.unwrap();
                assert_eq!(report.status, FlowFinalStatus::Failed);
                assert_eq!(report.results.vars()["name"], json!("parent"));
            } else {
                assert!(result.is_err());
            }
            assert_eq!(
                std::fs::read_to_string(tmp.path().join("child-cleanup")).unwrap(),
                "child"
            );
            assert_eq!(
                std::fs::read_to_string(tmp.path().join("parent-cleanup")).unwrap(),
                "parent"
            );
        }
    }

    #[tokio::test]
    async fn subflow_retry_reuses_the_same_inherited_and_local_vars() {
        let tmp = tempfile::tempdir().unwrap();
        let flow = parse("name: parent\nvars: {name: parent, inherited: base}\nsteps:\n  - name: call\n    type: flow\n    flow: child\n    retries: 1\n");
        let child = parse(
            r#"
name: child
vars: {name: child}
steps:
  - name: work
    type: command
    cwd: .
    command: 'printf "%s|%s\n" "{{vars.name}}" "{{vars.inherited}}" >> attempts; [ $(wc -l < attempts) -ge 2 ]'
"#,
        );
        let flows = catalog(vec![child]);
        let commands = HashMap::new();
        let report = FlowRunner::new(&flow, &commands, tmp.path().into(), false)
            .with_flows(&flows)
            .run()
            .await
            .unwrap();
        assert_eq!(report.status, FlowFinalStatus::Success);
        assert_eq!(report.results.get("call").unwrap().attempts, 2);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("attempts")).unwrap(),
            "child|base\nchild|base\n"
        );
        assert_eq!(report.results.vars()["name"], json!("parent"));
    }

    #[tokio::test]
    async fn ai_prompts_and_environment_receive_vars() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let program = tmp.path().join("fake-ai");
        std::fs::write(&program, "#!/bin/sh\nprintf '%s|%s' \"$2\" \"$VALUE\"\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let commands = HashMap::new();
        for kind in ["claude", "opencode"] {
            let flow = parse(&format!(
                r#"
name: ai
vars:
  settings: {{message: 'a&b<c>'}}
  number: 7
steps:
  - name: prompt
    type: {kind}
    prompt: '{{{{vars.settings.message}}}}'
    env:
      VALUE: '{{{{vars.number}}}}'
"#
            ));
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
            assert_eq!(report.status, FlowFinalStatus::Success);
            assert_eq!(
                report.results.get("prompt").unwrap().status.stdout(),
                "a&b<c>|7"
            );
        }
    }
}
