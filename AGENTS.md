# zek — Command flow orchestrator

Command flow orchestrator for the terminal, written in Rust. Executes commands defined in YAML, chains them into flows with retries and error handling, and integrates with **Claude** (`claude -p`) and **OpenCode** (`opencode run`).

## Project Structure

```
zek/
├── Cargo.toml              # Workspace (resolver = "2")
├── Cargo.lock
├── dist-workspace.toml      # cargo-dist config (release)
├── README.md
├── ROADMAP.md
├── LICENSE
├── .gitignore
├── .github/
│   └── workflows/           # CI / release (cargo-dist)
├── scripts/
│   └── install.sh           # build release + copy to ~/.local/bin
├── zek-cli/                 # `zek` binary (CLI)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs          # CLI arguments (clap), subcommands
│       └── commands.rs      # Subcommand implementations
└── zek-core/                # Library (core: config, commands, flows)
    ├── Cargo.toml
    ├── src/
    │   ├── lib.rs           # Re-exports all modules
    │   ├── config.rs        # Global config (~/.config/zek) and per-project config (zek.yaml)
    │   ├── commands.rs      # Reusable commands (commands/*.yaml)
    │   ├── flows.rs         # Flows (flows/*.yaml) and finally block
    │   ├── step.rs          # Step types and fields
    │   ├── execution.rs     # Flow execution engine (FlowRunner)
    │   ├── exec.rs          # Process execution + retries
    │   ├── context.rs       # Shared context + Handlebars templating
    │   ├── condition.rs     # Evaluation of `when` conditions
    │   ├── claude.rs        # `claude -p` client
    │   ├── opencode.rs      # `opencode run` client
    │   ├── parser.rs        # Extraction of JSON blocks from output
    │   ├── error.rs         # Unified error type (ZekError)
    │   ├── lang.rs          # Message language (en/es)
    │   └── util.rs          # Utilities (list YAML, expand env vars)
    └── tests/
        ├── common/mod.rs
        ├── fixtures/        # Test YAML (valid and invalid flows)
        ├── test_claude.rs
        ├── test_commands.rs
        ├── test_flows.rs
        └── test_opencode.rs
```

## Workspace

- `zek-core`: library containing all core logic (config, commands, flows, execution).
- `zek-cli`: `zek` binary consuming `zek-core` (clap, dialoguer, colored).

Key dependencies of `zek-core`: `serde`, `serde_yaml`, `serde_json`, `directories`, `handlebars` (templating), `futures`, `tokio`.

## Concepts

- **Command** (`commands/*.yaml`): reusable action with `name`, `run`, `cwd`, `timeout`, `env`.
- **Flow** (`flows/*.yaml`): chains steps and an optional `finally` block.
- **Step**: types `command`, `claude`, `opencode`, or `flow`; with fields such as `retries`, `retry_delay`, `on_error`, `on_success`, `when`, `parallel`, etc.
- **Templating**: Handlebars with `{{steps.<name>.<field>}}`, `{{args.<key>}}`, and `{{flow.<field>}}`.
- **Conditions** (`when`): comparisons, boolean logic, and parentheses.

## Configuration

- Global: `~/.config/zek/config.yaml` (`$XDG_CONFIG_HOME/zek`, `%APPDATA%\zek`).
- Per-project: `zek.yaml` in the current directory (or parent directories), overrides global config.

## Useful Commands

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```
