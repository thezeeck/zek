#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

struct Workspace {
    _tmp: tempfile::TempDir,
    workdir: PathBuf,
    config_dir: PathBuf,
}

impl Workspace {
    fn new(language: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let workdir = tmp.path().join("workspace");
        let config_dir = tmp.path().join("config");
        for dir in [
            workdir.join("commands"),
            workdir.join("flows"),
            config_dir.join("zek"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(
            config_dir.join("zek/config.yaml"),
            json!({"workdir": workdir, "language": language}).to_string(),
        )
        .unwrap();
        Self {
            _tmp: tmp,
            workdir,
            config_dir,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_zek"))
            .current_dir(&self.workdir)
            .env("XDG_CONFIG_HOME", &self.config_dir)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn flow(&self, value: Value) {
        std::fs::write(self.workdir.join("flows/sample.yaml"), value.to_string()).unwrap();
    }
}

#[test]
fn typed_cli_overrides_take_precedence_and_preserve_legacy_args() {
    let ws = Workspace::new("en");
    ws.flow(json!({
        "name": "sample",
        "vars": {"name": "yaml", "count": 1, "enabled": false, "settings": {"region": "us"}},
        "steps": [{
            "name": "show", "type": "command",
            "command": "printf '%s|%s|%s|%s|%s|%s|%s|%s' '{{vars.name}}' '{{args.name}}' '{{vars.count}}' '{{vars.settings.region}}' '{{vars.tags.1}}' '{{vars.enabled}}' '{{vars.empty}}' '{{vars.raw}}'",
            "when": "{{vars.enabled}} && {{vars.count}} == 3"
        }]
    }));
    let output = ws.run(&[
        "--report",
        "json",
        "--var",
        "name=first",
        "--var",
        "name=cli",
        "--var",
        "count=3",
        "--var",
        r#"settings={"region":"eu"}"#,
        "--var",
        r#"tags=["a","b"]"#,
        "--var",
        "enabled=true",
        "--var",
        "empty=null",
        "--var",
        "raw=a=b",
        "sample",
        "--name",
        "legacy",
    ]);
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["steps"][0]["stdout"], "cli|legacy|3|eu|b|true||a=b");
}

#[test]
fn trailing_var_stays_a_legacy_argument() {
    let ws = Workspace::new("en");
    ws.flow(json!({
        "name": "sample", "vars": {"name": "yaml"},
        "steps": [{"name": "show", "type": "command", "command": "printf '%s|%s' '{{vars.name}}' '{{args.var}}'"}]
    }));
    let output = ws.run(&["--report", "json", "sample", "--var", "name=legacy"]);
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["steps"][0]["stdout"], "yaml|name=legacy");
}

#[test]
fn malformed_var_overrides_fail_before_execution_in_both_languages() {
    for language in ["en", "es"] {
        let ws = Workspace::new(language);
        ws.flow(json!({"name": "sample", "steps": [{"name": "guard", "type": "command", "command": "touch should-not-exist", "cwd": "."}]}));
        for definition in ["missing-equals", "=value", " =value"] {
            let output = ws.run(&["--var", definition, "sample"]);
            assert_eq!(output.status.code(), Some(1));
            let error = String::from_utf8(output.stderr).unwrap();
            assert!(
                error.contains(if language == "en" {
                    "invalid --var"
                } else {
                    "--var inválido"
                }),
                "{error}"
            );
            assert!(!ws.workdir.join("should-not-exist").exists());
        }
    }
}

#[test]
fn overrides_are_rejected_for_direct_commands_and_builtin_subcommands() {
    let ws = Workspace::new("en");
    std::fs::write(
        ws.workdir.join("commands/direct.yaml"),
        "name: direct\nrun: touch should-not-exist\ncwd: .\n",
    )
    .unwrap();
    for name in ["direct", "list"] {
        let output = ws.run(&["--var", "name=value", name]);
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .contains("--var is only supported"));
        assert!(!ws.workdir.join("should-not-exist").exists());
    }
}
