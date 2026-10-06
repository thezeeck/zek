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

Plan previews and structured reports apply to flows. Direct commands stream their output.

## Parameters and AI steps

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
