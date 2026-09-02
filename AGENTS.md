# zek — Orquestador de flujos de comandos

Orquestador de flujos de comandos para la terminal, escrito en Rust. Ejecuta
comandos definidos en YAML, los encadena en flujos con reintentos y manejo de
errores, y se integra con **Claude** (`claude -p`) y **OpenCode** (`opencode run`).

## Estructura del proyecto

```
zek/
├── Cargo.toml              # Workspace (resolver = "2")
├── Cargo.lock
├── dist-workspace.toml      # Config de cargo-dist (release)
├── README.md
├── ROADMAP.md
├── LICENSE
├── .gitignore
├── .github/
│   └── workflows/           # CI / release (cargo-dist)
├── scripts/
│   └── install.sh           # build release + copia a ~/.local/bin
├── zek-cli/                 # Binario `zek` (CLI)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs          # Argumentos CLI (clap), subcomandos
│       └── commands.rs      # Implementación de los subcomandos
└── zek-core/                # Librería (núcleo: config, comandos, flujos)
    ├── Cargo.toml
    ├── src/
    │   ├── lib.rs           # Re-exporta todos los módulos
    │   ├── config.rs        # Config global (~/.config/zek) y por proyecto (zek.yaml)
    │   ├── commands.rs      # Comandos reutilizables (commands/*.yaml)
    │   ├── flows.rs         # Flujos (flows/*.yaml) y bloque finally
    │   ├── step.rs          # Tipos y campos de un step
    │   ├── execution.rs     # Motor de ejecución de flujos (FlowRunner)
    │   ├── exec.rs          # Ejecución de procesos + reintentos
    │   ├── context.rs       # Contexto compartido + templating Handlebars
    │   ├── condition.rs     # Evaluación de condiciones `when`
    │   ├── claude.rs        # Cliente de `claude -p`
    │   ├── opencode.rs      # Cliente de `opencode run`
    │   ├── parser.rs        # Extracción de bloques JSON de outputs
    │   ├── error.rs         # Tipo de error unificado (ZekError)
    │   ├── lang.rs          # Idioma de mensajes (en/es)
    │   └── util.rs          # Utilidades (listar YAML, expandir env vars)
    └── tests/
        ├── common/mod.rs
        ├── fixtures/        # YAML de prueba (flows válidos e inválidos)
        ├── test_claude.rs
        ├── test_commands.rs
        ├── test_flows.rs
        └── test_opencode.rs
```

## Workspace

- `zek-core`: librería con toda la lógica (config, comandos, flujos, ejecución).
- `zek-cli`: binario `zek` que consume `zek-core` (clap, dialoguer, colored).

Dependencias clave de `zek-core`: `serde`, `serde_yaml`, `serde_json`,
`directories`, `handlebars` (templating), `futures`, `tokio`.

## Conceptos

- **Comando** (`commands/*.yaml`): acción reutilizable con `name`, `run`, `cwd`,
  `timeout`, `env`.
- **Flujo** (`flows/*.yaml`): encadena steps y un bloque opcional `finally`.
- **Step**: tipos `command`, `claude`, `opencode` o `flow`; con campos como
  `retries`, `retry_delay`, `on_error`, `on_success`, `when`, `parallel`, etc.
- **Templating**: Handlebars con `{{steps.<name>.<campo>}}`, `{{args.<clave>}}` y
  `{{flow.<campo>}}`.
- **Condiciones** (`when`): comparaciones, lógica booleana y paréntesis.

## Configuración

- Global: `~/.config/zek/config.yaml` (`$XDG_CONFIG_HOME/zek`, `%APPDATA%\zek`).
- Por proyecto: `zek.yaml` en el directorio actual (o padres), solapa la global.

## Comandos útiles

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```
