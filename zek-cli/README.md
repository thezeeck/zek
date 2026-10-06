# zek-cli

The `zek` command-line application. Define reusable commands and flows in YAML, run them from the terminal, and optionally use Claude or OpenCode to diagnose failures or summarize results.

The package is named `zek-cli`; the installed executable is named `zek`. Execution is implemented by the companion [`zek-core`](https://github.com/thezeeck/zek/tree/main/zek-core) library.

## Installation

```bash
cargo install zek-cli
```

To install from a checkout, run this from the repository root:

```bash
cargo install --path zek-cli
```

For binary downloads and the full YAML reference, see the [project README](https://github.com/thezeeck/zek#zek). Claude and OpenCode steps require their respective executables on `PATH`; ordinary command flows do not need either tool.

## Quick start

Initialize zek with the absolute path to the project where your commands should run:

```bash
zek init /absolute/path/to/project
```

This saves the global configuration and offers to create the `commands/` and `flows/` directories. Add these files under that project:

```text
project/
├── commands/
│   └── build.yaml
└── flows/
    └── ci.yaml
```

`commands/build.yaml`:

```yaml
name: build
description: Build the Rust workspace
run: cargo build --workspace
cwd: .
timeout: 300
```

`flows/ci.yaml`:

```yaml
name: ci
description: Build and test the Rust workspace
steps:
  - name: build
    type: command
    command: build
    retries: 1
    retry_delay: 2

  - name: test
    type: command
    command: cargo test --workspace
    cwd: .

finally:
  steps:
    - name: summary
      type: command
      command: echo "Flow status={{flow.status}}"
      cwd: .
```

Run or inspect the configuration:

```bash
zek list
zek commands build
zek build
zek --dry-run ci
zek ci
zek --report json ci > report.json
```

Names come from the YAML `name` fields. If a command and a flow share a name, zek runs the flow.

## Configuration

The global `config.yaml` is stored under:

| Platform | Directory |
| --- | --- |
| Linux / macOS | `$XDG_CONFIG_HOME/zek`, or `~/.config/zek` |
| Windows | `%APPDATA%\zek` |

```yaml
workdir: /absolute/path/to/project
language: en
```

Messages support `en` and `es`. Initialize the global configuration first; a `zek.yaml` in the current directory or an ancestor can then override `workdir` and `language`:

```yaml
workdir: .
language: es
```

A relative project `workdir` is resolved from the directory containing `zek.yaml`. Set `cwd: .` on commands that should run in that workdir; without `cwd`, command execution inherits the CLI's current directory.

## Commands and options

| Command | Purpose |
| --- | --- |
| `zek init [dir]` | Initialize or reconfigure the global workdir |
| `zek config show` | Show the effective configuration |
| `zek config set-dir <path>` | Change the global workdir |
| `zek config set-language <en\|es>` | Change the message language |
| `zek list` | List commands and flows |
| `zek commands <name>` | Show a reusable command's definition |
| `zek history [--flow name] [--status status] [--limit n] [--json]` | Query recorded executions |
| `zek logs <run-id>` | Read structured execution events |
| `zek watch <flow> [options] [-- arguments]` | Rerun a flow when files change |
| `zek graph <flow> [--format text\|mermaid]` | Render a validated flow graph without execution |
| `zek ask "<message>"` | Send a standalone prompt to Claude |
| `zek completion --shell <shell>` | Generate shell completions |
| `zek <name> [--key value]` | Run a flow or command |

Completion shells: `bash`, `zsh`, `fish`, `powershell`, and `elvish`.

Place global options **before** the flow or command name:

| Option | Behavior |
| --- | --- |
| `--dry-run` | Print a flow's execution plan |
| `--verbose`, `-v` | Include failed-step output in the flow summary |
| `--debug` | Include failed-step output in the flow summary |
| `--timeout-global <seconds>` | Limit the whole flow, or override a direct command's timeout |
| `--log <file>` | Write execution events to a log file |
| `--report <json\|markdown>` | Export a flow report to stdout |
| `--step <name>` | Run a single main step, validating its required inputs |
| `--until <name>` | Run an inclusive prefix or DAG target and its ancestors |
| `--var KEY=VALUE` | Override a root flow variable; repeat before the flow name |

Plan previews and structured reports apply to flows. Direct commands stream their output.

## Graphs and selected execution

Set `execution: dag` in a flow and declare `needs: [build]` on dependent steps. Independent ready steps run concurrently, up to `max_concurrency` (default 4, positive integer) active main steps per DAG. Dependencies require success; failed or skipped prerequisites skip their descendants. `on_error: stop` drains active steps before cleanup; `continue` preserves independent branches. DAG mode rejects legacy parallel flags and jumps, while sequential mode remains the default.

```bash
zek graph ci-dag
zek graph ci-dag --format mermaid
zek --until test --dry-run ci-dag
zek --until test --report json ci-dag
zek --step build ci-dag
```

`--step` and `--until` are mutually exclusive and must precede the flow name; trailing options remain ordinary flow arguments. `--step` executes only the named main step. `--until` selects a sequential prefix or the DAG target with all its ancestors. Both retain `finally`; unknown names, missing inputs, and jumps outside the selection fail before processes start. Validation includes templates in reusable commands, nested flows, and cleanup. Dynamic lookups of the whole `steps` object are rejected for partial execution.

Dry-run shows the same selected nodes as `graph`. JSON and Markdown reports separate excluded steps from selected steps skipped by conditions or dependency failures. JSON adds `excluded_steps` and `skip_reasons` alongside `skipped_steps`.

Concurrent subflows keep branch results isolated. DAG child results use names such as `build-flow::compile`, accessible as `{{steps.[build-flow::compile].stdout}}`. For a complete YAML example and scheduling rules, see [dependency graphs](../README.md#dependency-graphs-and-partial-execution).

## History and watch

CLI flow executions now persist per-run events and summaries. Use the run ID printed in the summary or exported report:

```bash
zek history --flow ci --status failed --limit 10
zek history --json
zek logs <run-id>
zek watch ci --debounce-ms 300 --interval-ms 100
zek watch ci --include 'src/**/*.rs' --include 'flows/**' --exclude 'generated/**'
zek --var enabled=true watch ci -- --branch main
```

`logs` returns versioned JSONL events for attempts, retries, skipped steps, errors, cancellation, and finalization. It records metadata; process output remains available through `--report`, and `--log` preserves its text progress format. Dry-run and graph output create no history.

`history` defaults to the newest 20 runs; status filters include `success`, `failed`, `aborted`, `error`, `cancelled`, `running`, and `incomplete`. Configure `history.directory`, `retention_days` (default 30), and `max_runs` (default 1000) in global/project YAML. Zero disables the corresponding retention limit; active/incomplete records are preserved. The default location is the global config directory's `history/` subdirectory. Relative project paths resolve beside `zek.yaml`; relative global paths resolve against workdir. A project history section replaces global history settings.

Watch runs immediately, then polls file content every 100 ms and debounces changes for 300 ms by default. It keeps one run active and queues one rerun for changes received during that run. Definitions and configuration reload for each attempt; invalid YAML is recorded and observation continues. `Ctrl+C` cancels active processes, records cancellation, and exits 130. Cancellation does not run async cleanup.

Repeat `--include` / `--exclude` with glob patterns using `/`, `*`, `?`, and `**` (`**/` can match zero directories). Includes default to `**`. Built-in exclusions cover `target`, `.git`, `.zek`, `history`, `logs`, `cache`, `.cache`, `.log` files, configured history paths, and `--log`. Symlinks are skipped. Explicitly exclude other generated output files. Watch remains rooted in its initial workdir. Flow arguments follow `--`; global options precede `watch`.

See the [history and watch reference](../README.md#execution-history-and-watch) for storage layout and recovery behavior.

## Parameters and AI steps

Declare typed defaults in a flow's top-level `vars` mapping:

```yaml
name: welcome
vars:
  name: Ada
  enabled: true
  settings: {environment: dev}
steps:
  - name: hello
    type: command
    command: echo "Hello {{vars.name}} from {{vars.settings.environment}}"
    when: "{{vars.enabled}}"
```

```bash
zek --var name=Grace --var 'settings={"environment":"prod"}' welcome
```

`--var` accepts JSON values (including numbers, booleans, null, objects, and arrays), or plain text as a string. JSON-quote a value to force a string: `--var 'name="true"'`. Empty values and text that is not valid JSON remain strings. Missing `=` or an empty key is an error; duplicate keys use the last assignment.

Overrides replace entire top-level values. Subflows inherit resolved parent variables, shadow keys declared locally, and restore the parent's scope on return. Variables can be used in prompts, conditions, commands, and environment values. YAML values must be JSON-compatible, with finite numbers, string object keys, and no custom tags.

Place `--var` before the flow name. After the name, `--var` remains an ordinary flow argument exposed as `{{args.var}}`; it does not override variables. Direct commands and built-in subcommands do not accept flow overrides.

Flow parameters accept `--key value` and `--key=value`. For example, `zek greet --name Ada` makes `{{args.name}}` available to the flow:

```yaml
name: greet
steps:
  - name: hello
    type: command
    command: echo "Hello {{args.name}}"
```

Values are inserted literally, without HTML escaping or shell escaping. Use appropriate shell quoting for command parameters.

Use a Claude step to process a prior command's output:

```yaml
name: review
steps:
  - name: check
    type: command
    command: cargo check
    cwd: .
    on_error: continue

  - name: explain
    type: claude
    prompt: "Explain this check result: {{steps.check.stderr}}"
    output_format: json
```

For OpenCode, use `type: opencode`; its steps also accept `model` and `agent`. Both integrations support `session_id`, `continue_session`, `retries`, and `retry_delay`.

Flows also support conditions, `goto`, consecutive parallel steps, subflows, and `finally`. A `confirm: true` step is skipped when interactive confirmation is unavailable. See the [YAML reference](https://github.com/thezeeck/zek#flows-flowsyaml) for the complete field list and execution rules.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Successful execution |
| `1` | CLI, configuration, or execution error, including a global timeout |
| `2` | Failed flow |
| `3` | Flow aborted after exceeding its `goto` limit |

A directly executed command returns its own exit code when available; a direct-command timeout returns `1`.

## Development

From the repository root:

```bash
cargo run -p zek-cli -- --help
cargo test -p zek-cli
cargo clippy -p zek-cli --all-targets -- -D warnings
```

License: MIT.
