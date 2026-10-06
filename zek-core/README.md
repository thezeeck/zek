# zek-core

The Rust library behind [`zek-cli`](https://github.com/thezeeck/zek/tree/main/zek-cli). Load YAML commands and flows, execute them asynchronously, and inspect captured output and execution reports from your own application.

The library provides process execution, retries, conditions, Handlebars templates, flow composition, parallel command groups, cleanup blocks, and clients for `claude -p` and `opencode run`.

## Add to your application

```toml
[dependencies]
zek-core = "0.2.2"
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
| `with_vars(HashMap<String, serde_json::Value>)` | Override root flow defaults for `{{vars.<key>}}` |
| `with_history(HistoryStore)` | Persist per-run events, summary and cancellation |
| `with_selection(Selection::Step/Until(...))` | Select main steps while retaining cleanup |
| `with_flows(&HashMap<String, LoadedFlow>)` | Supply the catalog required by `type: flow` steps |
| `on_progress(Arc<...>)` | Receive `StepProgress::Started` and `StepProgress::Finished` events |
| `on_confirm(Arc<...>)` | Decide whether a `confirm: true` step may run |
| `FlowRunner::with_claude(...)` | Construct a runner with a custom Claude executable |
| `FlowRunner::with_opencode(...)` | Construct a runner with a custom OpenCode executable |

Progress callbacks implement `Fn(StepProgress) + Send + Sync`; confirmation callbacks implement `Fn(&str) -> bool + Send + Sync`. Steps requiring confirmation are skipped unless a callback returns `true`.

## Typed flow variables

`Flow::vars` is a `HashMap<String, serde_json::Value>` populated from the optional YAML `vars` mapping. It supports strings, finite numbers, booleans, null, arrays, and objects with string keys; non-finite numbers and YAML tags are rejected.

```yaml
name: greeting
vars:
  name: Ada
  settings: {count: 2}
  enabled: true
steps:
  - name: hello
    type: command
    command: echo "Hello {{vars.name}}"
    when: "{{vars.enabled}} && {{vars.settings.count}} > 0"
```

Pass a map to `FlowRunner::with_vars` to replace root defaults. Each entry replaces the entire value for that key; objects are not deep-merged. Library callers supply already typed JSON values. If constructing an override with `serde_json::json!`, add `serde_json = "1"` as a direct dependency of your application.

Subflows inherit the parent's resolved map, then apply their own local variables. Child variables are active through cleanup and are restored to the parent scope on return, including errors and retries. CLI overrides apply to root defaults and do not supersede explicit child defaults.

Use `ExecutionContext::vars()` to inspect the active map and `set_vars(...)` when constructing a context for direct template rendering. The returned `FlowReport.results` retains the resolved root variables. Access nested data with `{{vars.settings.count}}` or `{{vars.items.0}}`; missing variables render as empty strings. Variables contain literal data and remain separate from `{{args.*}}`.

## Execution plans and DAGs

`plan::ExecutionMode` defaults to `Sequential`. A flow with `execution: dag` uses `Step::needs` to require successful prerequisites and `Flow::max_concurrency` to limit active main steps (default 4). Cycles, unknown prerequisites, self-dependencies, zero concurrency, and legacy DAG controls (`parallel`, `goto`, `entry_only_via_goto`) are rejected during planning. DAG cleanup is sequential.

```rust
use zek_core::plan::{ExecutionPlan, Selection};

// Given an existing Flow and command catalog:
let selection = Selection::Until("test".to_string());
let plan = ExecutionPlan::build(&flow, &selection)?;
plan.validate_inputs(&flow, &commands)?;
println!("{}", plan.graph(&flow, true)); // Mermaid; false produces text

let report = FlowRunner::new(&flow, &commands, workdir, false)
    .with_selection(selection)
    .run()
    .await?;
```

When supplying a flow catalog, use `plan.validate_catalog_inputs(&flow, &commands, &flows)` to validate nested inputs; the runner performs this validation automatically. `selected` contains original YAML indices, `excluded` contains omitted main step names, and `dependencies` indexes prerequisite nodes. Graph rendering, CLI previews, selection, and execution use this shared plan.

`Selection::Step` includes only the target; `Until` includes a sequential prefix or the DAG target and its ancestors. Both preserve cleanup. Partial plans reject unavailable template results and external jumps before starting processes. Templates are parsed with Handlebars, including helpers, triple braces, and bracketed names. Dynamic use of the entire `steps` object is rejected when validating partial inputs.

The DAG executor publishes results before dependents start. Failed or skipped prerequisites skip descendants. `stop` and `end` halt new launches and drain active work before `finally`; `continue` permits independent branches to continue. Runtime errors also drain active work before cleanup, then return the original error.

Each active DAG branch owns a context snapshot. Subflow results are scoped under the invoking main step (`build-flow::compile`); access them using `{{steps.[build-flow::compile].stdout}}` or `report.results.get("build-flow::compile")`. Sequential subflows retain unqualified names. Each nested DAG applies its own concurrency limit. See the [complete YAML example](../README.md#dependency-graphs-and-partial-execution).

## History and file observation

Library history is opt-in; the CLI enables it for flow runs. Attach a store to a runner:

```rust
use zek_core::history::HistoryStore;

// Given an existing runner and a chosen storage path:
let store = HistoryStore::new(history_directory, 30, 1000);
let report = runner.with_history(store.clone()).run().await?;
let id = report.run_id.as_ref().expect("history enabled");
let events = store.events(id)?;
let runs = store.list(Some("ci"), Some("failed"), 20)?;
```

`HistoryStore::start` opens a unique run directory; `HistoryRun::event` writes versioned JSONL under a mutex with contiguous per-run sequence numbers. `finish` is idempotent and writes a terminal event plus an atomic summary replacement. Dropping the final run guard while unfinished records `cancelled`. `with_history_run` lets a host attach a run it already opened, including host-level timeouts. Guards represent one run and must not be reused across executions.

The runner records attempts, retries, skips (with a reason), internal errors, and completion. `FlowReport::run_id` is `None` without history. Events omit stdout/stderr, prompts, arguments, variables and environment; their schema version is currently 1. Nested flow scopes identify event origins. Readable records survive a truncated tail; missing or corrupt summaries are recovered from events. `prune` removes only finished complete runs using age/count limits; zero disables a limit. Retention runs before `start`, and the CLI also applies it before `history` queries.

`Config::history_store()` resolves `HistoryConfig`: default global config directory plus `history`, 30 days and 1000 completed runs. Relative global paths resolve against workdir; relative project paths resolve beside `zek.yaml`. Project history mappings replace global mappings.

`watch::PollWatcher` scans included file contents using bounded buffers, skips symlinks and configured output paths, and tracks one pending debounced change. `poll(Instant)` records changes; `take_ready(Instant)` consumes one rerun after the quiet window. Keep polling during active work but consume pending changes only after that work finishes. `ignore_path` adds output locations without generating a rerun. The caller owns the execution loop and cancellation policy. Globs support `*`, `?`, `**`, and zero-directory `**/`; separators are `/` on every platform.

The CLI supplies a polling interval, reloads definitions between runs, keeps observing after YAML errors, and kills managed processes on Ctrl+C. The existing process guard also handles cancellation when library callers drop a runner future. See the [complete reference](../README.md#execution-history-and-watch) for CLI defaults and exclusions.

## Results and errors

`FlowRunner::run` returns `Result<FlowReport, ZekError>`. An ordinary failed command produces a report whose status is `Failed`; configuration, template, or validation errors produce `Err`.

`FlowReport` includes the flow name, final status, exit reason, duration, failed, skipped, and excluded step names, skip reasons, and an `ExecutionContext` containing step results. Its exit-code mapping is `Success → 0`, `Failed → 2`, and `Aborted → 3`.

Each `SerializedStepResult` contains a `StepExecutionStatus`, duration, and attempt count. Status variants are `Success`, `Failed`, and `TimedOut`; use `stdout()`, `stderr()`, `exit_code()`, and the status predicates to inspect them. A timeout has no exit code.

Use `ExecutionContext::ordered_results()` for recording order (DAG completion order can vary), or `get(name)` for a specific step.

## Execution rules

- Command steps resolve a reusable command name first and otherwise execute the value as a shell command. The shell is `sh -c` on Unix and `cmd /C` on Windows.
- Relative command `cwd` values are resolved against the runner's workdir. Without `cwd`, commands inherit the host application's current directory. Claude and OpenCode steps run in the runner's workdir.
- `retries` counts additional attempts; `retry_delay` is measured in seconds. Retries apply to command, Claude, OpenCode, and flow steps. Configuration and template errors are not retried.
- Retrying a subflow starts from the parent's original context and reruns its cleanup. External effects from earlier attempts remain.
- `finally` is attempted after normal completion, failed steps, and runtime configuration or template errors. External cancellation drops the running future; it does not execute asynchronous cleanup blocks.
- Timeouts cover process waiting and output capture. Timeout or cancellation terminates the managed process group on Unix or job on Windows and closes the readers.
- Consecutive `parallel: true` steps execute together against the same prior context. Parallel steps cannot invoke flows or specify `on_error` / `on_success` actions.
- Runtime invocation rejects active recursion and limits nesting to 16 flows. Sequential invocations of the same flow are allowed.
- Templates expose `steps`, `args`, `vars`, and the flow result available to `finally`. Missing values become empty strings. Values are inserted literally; shell quoting is the caller's responsibility.

For YAML fields, examples of conditions, `goto`, and session continuation, see the [project reference](https://github.com/thezeeck/zek#flows-flowsyaml).

## Modules

| Module | Main responsibilities |
| --- | --- |
| `config` | Global / project configuration and workdir paths |
| `commands`, `flows`, `step` | YAML definitions, catalogs, and validation |
| `history` | Versioned per-run event storage, queries, recovery and retention |
| `watch` | Portable content polling, glob filtering and debounce |
| `plan` | Validated selection, dependencies, and text / Mermaid graphs |
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
