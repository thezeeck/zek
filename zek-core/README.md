# zek-core

The Rust library behind [`zek-cli`](https://github.com/thezeeck/zek/tree/main/zek-cli). Load YAML commands and flows, execute them asynchronously, and inspect captured output and execution reports from your own application.

The library provides process execution, retries, conditions, Handlebars templates, flow composition, parallel command groups, cleanup blocks, and clients for `claude -p` and `opencode run`.

## Add to your application

```toml
[dependencies]
zek-core = "0.2.1"
tokio = { version = "1", features = ["macros", "rt", "process", "io-util", "time", "sync"] }
```

The crate is imported as `zek_core`. Execution requires a Tokio runtime; YAML loading and validation are synchronous. Claude and OpenCode steps require their corresponding executable on `PATH`.

## Run a flow from YAML

This complete example runs without a global zek configuration:

```rust
use std::collections::HashMap;
use std::path::Path;

use zek_core::error::ZekError;
use zek_core::execution::FlowRunner;
use zek_core::flows::{Flow, LineIndex};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ZekError> {
    let yaml = r#"
name: greeting
steps:
  - name: hello
    type: command
    command: echo "Hello {{args.name}}"
    cwd: .
    timeout: 10
"#;
    let source = Path::new("greeting.yaml");
    let flow = Flow::from_str(yaml, source)?;
    let warnings = flow.validate(source, &LineIndex::build(yaml))?;
    for warning in warnings {
        eprintln!("warning: {warning}");
    }

    let commands = HashMap::new();
    let args = HashMap::from([("name".to_string(), "Ada".to_string())]);
    let runner = FlowRunner::new(
        &flow,
        &commands,
        std::env::current_dir()?,
        false,
    )
    .with_args(args);

    let report = runner.run().await?;
    println!("status={} exit_code={}", report.status.as_str(), report.exit_code());
    if let Some(result) = report.results.get("hello") {
        print!("{}", result.status.stdout());
    }
    Ok(())
}
```

`Flow::from_str` only deserializes YAML; call `validate` when constructing flows this way. `Flow::load_with_validation` reads and validates a file and returns both the flow and its warnings.

The `stream` argument to `FlowRunner::new` controls live stdout/stderr streaming. Output is captured in the report with either setting.

## Load an existing configuration

To use the same global and project configuration as the CLI:

```rust
use zek_core::config;
use zek_core::error::ZekError;
use zek_core::execution::{FlowReport, FlowRunner};
use zek_core::{commands, flows, lang};

pub async fn run_configured_flow(name: &str) -> Result<FlowReport, ZekError> {
    let config = config::load_effective()?;
    config.validate()?;
    lang::set(config.language);

    let commands = commands::load_all(&config.commands_dir())?;
    let flows = flows::load_all(&config.flows_dir())?;
    let loaded = flows.get(name).ok_or_else(|| {
        ZekError::InvalidConfig(format!("flow not found: {name}"))
    })?;
    for warning in &loaded.warnings {
        eprintln!("warning: {warning}");
    }
    for warning in loaded.flow.validate_flow_refs(&flows) {
        eprintln!("warning: {warning}");
    }

    FlowRunner::new(&loaded.flow, &commands, config.workdir, false)
        .with_flows(&flows)
        .run()
        .await
}
```

`load_effective` requires a valid global configuration and overlays the nearest `zek.yaml` found from the current directory upward. It does not initialize configuration interactively. You can instead provide your own workdir and load its `commands/` and `flows/` directories directly.

`commands::load_all` and `flows::load_all` index definitions by their YAML `name`, retaining the source paths. The flow loader rejects recursive references between catalog entries, including references inside `finally`.

## Runner configuration

| Method | Purpose |
| --- | --- |
| `with_args(HashMap<String, String>)` | Supply values for `{{args.<key>}}` |
| `with_flows(&HashMap<String, LoadedFlow>)` | Supply the catalog required by `type: flow` steps |
| `on_progress(Arc<...>)` | Receive `StepProgress::Started` and `StepProgress::Finished` events |
| `on_confirm(Arc<...>)` | Decide whether a `confirm: true` step may run |
| `FlowRunner::with_claude(...)` | Construct a runner with a custom Claude executable |
| `FlowRunner::with_opencode(...)` | Construct a runner with a custom OpenCode executable |

Progress callbacks implement `Fn(StepProgress) + Send + Sync`; confirmation callbacks implement `Fn(&str) -> bool + Send + Sync`. Steps requiring confirmation are skipped unless a callback returns `true`.

## Results and errors

`FlowRunner::run` returns `Result<FlowReport, ZekError>`. An ordinary failed command produces a report whose status is `Failed`; configuration, template, or validation errors produce `Err`.

`FlowReport` includes the flow name, final status, exit reason, duration, failed and skipped step names, and an `ExecutionContext` containing step results. Its exit-code mapping is `Success → 0`, `Failed → 2`, and `Aborted → 3`.

Each `SerializedStepResult` contains a `StepExecutionStatus`, duration, and attempt count. Status variants are `Success`, `Failed`, and `TimedOut`; use `stdout()`, `stderr()`, `exit_code()`, and the status predicates to inspect them. A timeout has no exit code.

Use `ExecutionContext::ordered_results()` for deterministic result order, or `get(name)` for a specific step.

## Execution rules

- Command steps resolve a reusable command name first and otherwise execute the value as a shell command. The shell is `sh -c` on Unix and `cmd /C` on Windows.
- Relative command `cwd` values are resolved against the runner's workdir. Without `cwd`, commands inherit the host application's current directory. Claude and OpenCode steps run in the runner's workdir.
- `retries` counts additional attempts; `retry_delay` is measured in seconds. Retries apply to command, Claude, OpenCode, and flow steps. Configuration and template errors are not retried.
- Retrying a subflow starts from the parent's original context and reruns its cleanup. External effects from earlier attempts remain.
- `finally` is attempted after normal completion, failed steps, and runtime configuration or template errors. External cancellation drops the running future; it does not execute asynchronous cleanup blocks.
- Timeouts cover process waiting and output capture. Timeout or cancellation terminates the managed process group on Unix or job on Windows and closes the readers.
- Consecutive `parallel: true` steps execute together against the same prior context. Parallel steps cannot invoke flows or specify `on_error` / `on_success` actions.
- Runtime invocation rejects active recursion and limits nesting to 16 flows. Sequential invocations of the same flow are allowed.
- Templates expose `steps`, `args`, and the flow result available to `finally`. Missing values become empty strings. Values are inserted literally; shell quoting is the caller's responsibility.

For YAML fields, examples of conditions, `goto`, and session continuation, see the [project reference](https://github.com/thezeeck/zek#flows-flowsyaml).

## Modules

| Module | Main responsibilities |
| --- | --- |
| `config` | Global / project configuration and workdir paths |
| `commands`, `flows`, `step` | YAML definitions, catalogs, and validation |
| `execution` | `FlowRunner`, progress events, and `FlowReport` |
| `exec` | `CommandExecutor`, `run_program`, process status, and command retries |
| `context`, `condition` | Captured results, templates, and boolean expressions |
| `claude`, `opencode` | Direct clients and session ID extraction |
| `parser` | JSON block extraction and parsing |
| `error`, `lang`, `util` | Errors, message language, and shared utilities |

## Development

From the repository root:

```bash
cargo test -p zek-core
cargo clippy -p zek-core --all-targets --all-features -- -D warnings
cargo doc -p zek-core --no-deps
```

License: MIT.
