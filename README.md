# zek

Command flow orchestrator for the terminal, written in Rust. Executes commands defined in YAML, chains them into flows with retries and error handling, and integrates with **Claude** (`claude -p`) and **OpenCode** (`opencode run`) for diagnostic steps and summaries.

Package guides: [zek-cli](zek-cli/README.md) for terminal usage and [zek-core](zek-core/README.md) for Rust integration.

## Installation

```bash
# From crates.io
cargo install zek-cli

# Or from source code
git clone https://github.com/thezeeck/zek
cd zek
./scripts/install.sh          # build release + copy to ~/.local/bin
```

Pre-built binaries for Linux, macOS, and Windows are also available on [GitHub Releases](https://github.com/thezeeck/zek/releases), along with `shell`/`powershell`/`npm` installers generated with cargo-dist.

### Shell completions

```bash
zek completion --shell bash  # also: zsh, fish, powershell, elvish
```

## Getting Started

```bash
# Set up your working directory (will contain commands/ and flows/)
zek init ~/my-projects

# List available commands and flows
zek list

# Run a flow
zek deploy

# Run a command directly
zek build

# Ask Claude something outside of flows
zek ask "How do I optimize this code?"
```

The configuration is saved in `~/.config/zek/config.yaml` (or `$XDG_CONFIG_HOME/zek`, `%APPDATA%\zek` on Windows).

The message language is configured using the `language` key (`en`/`es` values, default is `en`):

```yaml
workdir: /path/to/my-project
language: es
```

### Per-project configuration (`zek.yaml`)

In addition to global config, `zek` looks for a `zek.yaml` file in the current directory (and its parent directories). If found, it overrides the global config, allowing you to define `workdir` and `language` per repository. Relative `workdir` paths are resolved against the directory containing `zek.yaml`.

```yaml
# <repo>/zek.yaml
workdir: .            # use commands/ and flows/ from the repo itself
language: en
```

```bash
cd <repo> && zek list   # uses <repo>/commands and <repo>/flows
```

## Directory Structure

```
~/.config/zek/config.yaml
<workdir>/
├── commands/*.yaml   # Reusable commands
└── flows/*.yaml      # Flows chaining commands + AI steps
```

## Commands (`commands/*.yaml`)

```yaml
name: build
description: "Compiles the project"
run: "cargo build --release"
cwd: "."
timeout: 300
env:
  RUST_BACKTRACE: "1"
```

## Flows (`flows/*.yaml`)

```yaml
name: deploy
description: "Build, test, and summary with OpenCode"
steps:
  - name: build
    type: command
    command: build
    retries: 2
    retry_delay: 5
    on_error: goto:diagnose_failure

  - name: diagnose_failure
    type: opencode
    entry_only_via_goto: true
    prompt: >
      The build failed with this error: {{steps.build.stderr}}.
      What is the probable root cause?
    on_success: end

  - name: test
    type: command
    command: test
    retries: 1
    on_error: continue

  - name: summary
    type: opencode
    prompt: >
      Summarize the flow result:
      - build={{steps.build.status}} ({{steps.build.exit_code}})
      - test={{steps.test.status}} ({{steps.test.exit_code}})
    on_success: end

finally:
  fail_flow_on_error: false
  steps:
    - name: cleanup
      type: command
      command: "rm -rf ./tmp/build-*"
      on_error: continue
```

### Step types

| Type | Description |
|------|-------------|
| `command` | Executes a command (named from `commands/` or raw) |
| `claude` | Sends a prompt to `claude -p` |
| `opencode` | Sends a prompt to `opencode run` |
| `flow` | Invokes another flow as a subroutine |

### Step fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Unique name within the flow |
| `type` | enum | required | `command`, `claude`, `opencode`, or `flow` |
| `retries` | u32 | 0 | Additional attempts for command, Claude, OpenCode, and flow steps |
| `retry_delay` | u32 | 0 | Seconds between retries |
| `on_error` | enum | `stop` | `stop`, `continue`, or `goto:<name>` |
| `on_success` | enum | `continue` | `continue`, `end`, or `goto:<name>` |
| `entry_only_via_goto` | bool | false | Only executes if another step targets it via `goto` |
| `confirm` | bool | false | Asks for confirmation before executing |
| `when` | string | - | Condition to execute the step (skipped if false) |
| `parallel` | bool | false | Runs in parallel with consecutive `parallel: true` steps |
| `flow` | string | - | Only `flow`: name of the flow to invoke |
| `command` | string | - | Only `command`: command to run |
| `cwd` | string | - | Only `command` |
| `timeout` | u32 | 300 | Timeout in seconds |
| `prompt` | string | - | Only `claude`/`opencode`: prompt with placeholders |
| `output_format` | string | - | Only `claude`/`opencode`: `json` for parsing |
| `session_id` | string | - | Only `claude`/`opencode`: previous session ID |
| `continue_session` | bool | false | Only `claude`/`opencode`: continue previous session |
| `model` | string | - | Only `opencode`: model (`provider/model`) |
| `agent` | string | - | Only `opencode`: agent |
| `env` | map | - | Extra environment variables for the step |

### Templating

Prompts and commands use [Handlebars](https://handlebarsjs.com/). Available placeholders include `{{steps.<name>.<field>}}` (with `status`, `stdout`, `stderr`, `exit_code`, `attempts`), `{{args.<key>}}` (arguments passed via CLI), and, inside `finally`, `{{flow.<field>}}` (with `status`, `failed_steps`, and `exit_reason`). Missing variables resolve to an empty string.

Placeholder values are inserted literally, without HTML escaping. Commands still run through the shell, so use shell quoting appropriate to the values you pass.

`retries` and `retry_delay` apply to all step types. Process failures and timeouts can be retried; configuration and template errors are returned immediately after attempting `finally`. Retrying a flow reruns its main steps and cleanup, starting from the parent's original context; external effects from earlier attempts are not undone.

Step timeouts cover both the process and output capture. On timeout or cancellation, zek terminates the managed process group on Unix or job on Windows and closes the output readers. `confirm: true` steps are skipped when interactive confirmation is unavailable; library callers must supply an approving confirmation callback.

### Conditionals (`when`)

A step can include a `when` field with a condition. If it evaluates to false (or is empty), the step is skipped. The condition is rendered first with Handlebars and then evaluated as a boolean expression, supporting comparisons (`==`, `!=`, `<`, `<=`, `>`, `>=`), logic (`&&`, `||`, `!`), and parentheses.

```yaml
steps:
  - name: build
    type: command
    command: cargo build

  - name: diagnose
    type: claude
    when: "{{steps.build.failed}}"
    prompt: "The build failed: {{steps.build.stderr}}"
```

False values: empty string, `false`, `no`, `0`. Everything else is true. Placeholders expose `{{steps.<name>.success}}` and `{{steps.<name>.failed}}`.

### Environment Variables and Secrets

`steps` and `commands` can define an `env` map containing additional environment variables. Values accept placeholders (`{{args.<key>}}`, etc.) and process environment variable references (`$VAR` or `${VAR}`), enabling secrets to be passed without hardcoding them into YAML:

```yaml
steps:
  - name: deploy
    type: command
    command: ./deploy.sh
    env:
      DEPLOY_TOKEN: "${DEPLOY_TOKEN}"   # retrieved from process environment
      ENV: staging
```

```bash
DEPLOY_TOKEN=secret zek deploy
```

### Flow Composition (`type: flow`)

A `type: flow` step invokes another flow as a subroutine. Its steps are registered in the same context, allowing you to reference their results using `{{steps.<name>.<field>}}`:

Recursive references, including those in `finally`, are rejected when the catalog is loaded. Runtime invocation also checks for recursion and limits nesting to 16 active flows. Sequential calls to the same flow are allowed.

```yaml
# flows/ci.yaml
name: ci
steps:
  - name: build
    type: flow
    flow: build
  - name: test
    type: flow
    flow: test
```

```yaml
# flows/build.yaml
name: build
steps:
  - name: compile
    type: command
    command: cargo build
```

### Parallel Steps

Consecutive steps marked with `parallel: true` run simultaneously, and the flow waits for all of them to complete before moving forward. If any step fails, the flow stops. Parallel steps cannot define `on_error` or `on_success`.

```yaml
steps:
  - name: lint
    type: command
    command: cargo clippy
    parallel: true
  - name: typecheck
    type: command
    command: cargo check
    parallel: true
  - name: deploy
    type: command
    command: ./deploy.sh
```

### CLI Arguments

Parameters can be passed to a flow using `--key value` (or `--key=value`) and referenced inside the YAML as `{{args.<key>}}`:

```bash
zek feature --branch my-branch
```

```yaml
name: feature
steps:
  - name: switch_main
    type: command
    command: git checkout main
  - name: create_branch
    type: command
    command: git checkout -b "{{args.branch}}"
```

## CLI Reference

```bash
zek init [dir]              # initial setup (or re-configure)
zek config show             # displays current configuration
zek config set-dir <path>   # changes working directory
zek config set-language <lang>  # changes language (en | es, default en)
zek list                    # list commands and flows
zek commands <name>         # displays a command configuration
zek ask "<message>"         # ask Claude outside of flows
zek completion --shell <sh> # generates shell completions (bash|zsh|fish|powershell|elvish)
zek <flow>|<command>        # resolves flow first, then command
zek <flow> --key value      # arguments accessible as {{args.key}}
```

Global flags: `--dry-run`, `--verbose/-v`, `--debug`, `--timeout-global <sec>`, `--log <file>` (saves execution log), and `--report <json|markdown>` (exports a report to stdout instead of summary). Global flags must precede the flow/command name: `zek --log run.log deploy` or `zek --report json deploy > report.json`.

Exit codes: `0` success, `2` failed, `3` aborted (infinite loop detected).
