---
name: zek-flow
description: Use when creating, editing, or reviewing zek workflow YAML files (flows/*.yaml and commands/*.yaml). Covers flow/step schema, step types, on_error/on_success, goto, when conditions, parallel steps, finally blocks, Handlebars templating, and validation rules.
---

# Creating zek flows

This skill describes how to write flows and commands for **zek** (command flow orchestrator in Rust). Flows chain steps that execute commands, call Claude/OpenCode, or invoke other flows.

## File Locations

```
<workdir>/
├── commands/*.yaml   # Reusable commands
└── flows/*.yaml      # Flows (one flow per file, .yaml or .yml extension)
```

The `workdir` is defined in `~/.config/zek/config.yaml` (`workdir` key) or in a per-project `zek.yaml`. Files are loaded from `<workdir>/commands/` and `<workdir>/flows/`, indexed by `name`.

## Commands (`commands/*.yaml`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Unique name among all commands |
| `run` | string | required | Command to execute |
| `description` | string | `""` | Description |
| `cwd` | string | - | Working directory |
| `timeout` | u32 | `300` | Timeout in seconds |
| `author` | string | - | Author |
| `env` | map | - | Extra environment variables |

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

Top-level structure:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Unique name among all flows |
| `description` | string | `""` | Description |
| `max_jumps` | usize | `20` | Maximum number of `goto` jumps (anti infinite loop) |
| `steps` | list | `[]` | Main steps (executed sequentially) |
| `finally` | map | - | Block that always executes at the end |

```yaml
name: deploy
description: "Build, test, and summary"
steps:
  - name: build
    type: command
    command: build
finally:
  steps:
    - name: cleanup
      type: command
      command: "rm -rf ./tmp/build-*"
```

## Step Types (`type`)

| Type | Usage |
|------|-------|
| `command` | Executes a command (name of a command in `commands/` or a raw command) |
| `claude` | Sends a prompt to `claude -p` |
| `opencode` | Sends a prompt to `opencode run` |
| `flow` | Invokes another flow as a subroutine |

## Step Fields

| Field | Type | Default | Applies to | Description |
|-------|------|---------|------------|-------------|
| `name` | string | required | all | Unique within the flow |
| `type` | enum | required | all | `command`, `claude`, `opencode`, `flow` |
| `retries` | u32 | `0` | all | Retries before failing |
| `retry_delay` | u32 | `0` | all | Seconds between retries |
| `on_error` | enum | `stop` (steps) / `continue` (finally) | all | `stop`, `continue`, `goto:<name>` |
| `on_success` | enum | `continue` | all | `continue`, `end`, `goto:<name>` |
| `entry_only_via_goto` | bool | `false` | all | Only executes if another step references it via `goto` |
| `confirm` | bool | `false` | all | Asks for confirmation before executing |
| `when` | string | - | all | Condition; if false, the step is skipped |
| `parallel` | bool | `false` | all | Runs in parallel with consecutive `parallel: true` steps |
| `env` | map | - | all | Extra environment variables |
| `command` | string | - | `command` | Command to execute |
| `cwd` | string | - | `command` | Working directory |
| `timeout` | u32 | `300` | `command` | Timeout in seconds |
| `flow` | string | - | `flow` | Name of the flow to invoke |
| `prompt` | string | - | `claude`/`opencode` | Prompt with placeholders |
| `output_format` | string | - | `claude`/`opencode` | `json` for automatic parsing |
| `session_id` | string | - | `claude`/`opencode` | Previous session ID |
| `continue_session` | bool | `false` | `claude`/`opencode` | Continue previous session |
| `model` | string | - | `opencode` | Model (`provider/model`) |
| `agent` | string | - | `opencode` | Agent |

## Validation Rules (Hard Errors)

- Duplicate `name` within the same flow → error.
- `goto` to a non-existent step → error. Target must be in the same scope (`steps` can only target `steps`; `finally` can only target `finally`).
- `goto` to self → error.
- `parallel: true` step with `on_error` or `on_success` → error.
- `finally` block containing a step whose name matches a step in `steps` → error.
- `on_error: goto:` (empty) → error.
- File loading with duplicate names across files (`commands/` or `flows/`) → error.

Errors are reported with `file:line`.

## Warnings (Non-blocking)

- Step with `entry_only_via_goto: true` that no other step references with `goto` (dead step).
- Static `goto` cycles (e.g., `a` → `b` → `a`), detected using Tarjan's algorithm.
- References to non-existent `command` or `flow`.

## Flow Control (`on_error` / `on_success`)

```yaml
steps:
  - name: build
    type: command
    command: build
    retries: 2
    retry_delay: 5
    on_error: goto:diagnose_failure   # jumps to another step on failure

  - name: diagnose_failure
    type: opencode
    entry_only_via_goto: true           # only executes via goto
    prompt: "The build failed: {{steps.build.stderr}}"
    on_success: end                     # terminates the flow successfully

  - name: test
    type: command
    command: test
    on_error: continue                  # ignores failure and continues
```

- Default `on_error`: `stop` (in `steps`), `continue` (in `finally`).
- Default `on_success`: `continue`. `end` terminates the flow successfully.

## `finally` Block

Always executes when the flow finishes (whether successful or failed). Its steps follow the same rules as `steps`, but its `goto` scope is independent.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `fail_flow_on_error` | bool | `false` | If a step in `finally` fails, mark the flow as failed |
| `max_jumps` | usize | - | Maximum jumps within `finally` |
| `steps` | list | `[]` | Block steps |

```yaml
finally:
  fail_flow_on_error: false
  steps:
    - name: cleanup
      type: command
      command: "rm -rf ./tmp/build-*"
      on_error: continue
```

## Templating (Handlebars)

`prompt`, `command`, `when`, and `env` values are rendered with Handlebars. Missing variables resolve to an empty string.

- Step results: `{{steps.<name>.<field>}}` with fields `status`, `success`, `failed`, `stdout`, `stderr`, `exit_code`, `attempts`, `duration_ms`.
- CLI arguments: `{{args.<key>}}` (passed via `zek <flow> --key value`).
- Flow state (only inside `finally`): `{{flow.status}}`, `{{flow.failed_steps}}`, `{{flow.exit_reason}}`.

```yaml
steps:
  - name: summary
    type: opencode
    prompt: >
      build={{steps.build.status}} (exit={{steps.build.exit_code}})
      test={{steps.test.status}}
```

## Conditionals (`when`)

Rendered with Handlebars and then evaluated as a boolean expression. Supports comparisons (`==`, `!=`, `<`, `<=`, `>`, `>=`), logic (`&&`, `||`, `!`), and parentheses. False values: empty string, `false`, `no`, `0`.

```yaml
steps:
  - name: diagnose
    type: claude
    when: "{{steps.build.failed}}"
    prompt: "The build failed: {{steps.build.stderr}}"
```

## Flow Composition (`type: flow`)

Invokes another flow as a subroutine. Its steps are registered in the same context, referenceable via `{{steps.<name>.<field>}}`.

```yaml
name: ci
steps:
  - name: build
    type: flow
    flow: build
  - name: test
    type: flow
    flow: test
```

## Parallel Steps

Consecutive steps with `parallel: true` run simultaneously; the flow waits for all to complete before continuing. They cannot define `on_error` or `on_success`.

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
```

## Environment Variables and Secrets

`env` supports placeholders and process variable references (`$VAR` / `${VAR}`), preventing hardcoded secrets in YAML.

```yaml
steps:
  - name: deploy
    type: command
    command: ./deploy.sh
    env:
      DEPLOY_TOKEN: "${DEPLOY_TOKEN}"
      ENV: staging
```

```bash
DEPLOY_TOKEN=secret zek deploy
```

## Verification

```bash
zek list                    # lists loaded commands and flows (validates parsing)
zek <flow> --dry-run       # shows the plan without executing
zek --debug <flow>         # validation detail and warnings
```

Reference tests live in `zek-core/tests/fixtures/` and `zek-core/src/flows.rs` (`validate`), as well as `zek-core/src/step.rs`.
