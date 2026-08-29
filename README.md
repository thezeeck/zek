# zek

Orquestador de flujos de comandos para la terminal, escrito en Rust. Ejecuta
comandos definidos en YAML, los encadena en flujos con reintentos y manejo de
errores, y se integra con **Claude** (`claude -p`) y **OpenCode** (`opencode run`)
para pasos de diagnóstico y resúmenes.

## Instalación

```bash
# Desde crates.io
cargo install zek-cli

# O desde el código fuente
git clone https://github.com/thezeeck/zek
cd zek
./scripts/install.sh          # build release + copia a ~/.local/bin
```

También hay binarios precompilados para Linux, macOS y Windows en las
[releases de GitHub](https://github.com/thezeeck/zek/releases), e installers
`shell`/`powershell`/`npm` generados con cargo-dist.

### Shell completions

```bash
zek completion --shell bash  # también: zsh, fish, powershell, elvish
```

## Primeros pasos

```bash
# Configura tu carpeta de trabajo (contendrá commands/ y flows/)
zek init ~/mis-proyectos

# Lista comandos y flujos disponibles
zek list

# Ejecuta un flujo
zek deploy

# Ejecuta un comando directamente
zek build

# Pregunta algo a Claude fuera de flujos
zek ask "¿Cómo optimizo este código?"
```

La configuración se guarda en `~/.config/zek/config.yaml`
(o `$XDG_CONFIG_HOME/zek`, `%APPDATA%\zek` en Windows).

## Estructura de carpetas

```
~/.config/zek/config.yaml
<workdir>/
├── commands/*.yaml   # Comandos reutilizables
└── flows/*.yaml      # Flujos que encadenan comandos + pasos de IA
```

## Comandos (`commands/*.yaml`)

```yaml
name: build
description: "Compila el proyecto"
run: "cargo build --release"
cwd: "."
timeout: 300
```

## Flujos (`flows/*.yaml`)

```yaml
name: deploy
description: "Build, test y resumen con OpenCode"
steps:
  - name: build
    type: command
    command: build
    retries: 2
    retry_delay: 5
    on_error: goto:diagnosticar_falla

  - name: diagnosticar_falla
    type: opencode
    entry_only_via_goto: true
    prompt: >
      El build falló con este error: {{steps.build.stderr}}.
      ¿Cuál es la causa raíz probable?
    on_success: end

  - name: test
    type: command
    command: test
    retries: 1
    on_error: continue

  - name: resumen
    type: opencode
    prompt: >
      Resumí el resultado del flujo:
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

### Tipos de step

| Tipo | Descripción |
|------|-------------|
| `command` | Ejecuta un comando (nombrado desde `commands/` o crudo) |
| `claude` | Envía un prompt a `claude -p` |
| `opencode` | Envía un prompt a `opencode run` |

### Campos de un step

| Campo | Tipo | Default | Descripción |
|-------|------|---------|-------------|
| `name` | string | obligatorio | Nombre único dentro del flujo |
| `type` | enum | obligatorio | `command`, `claude` u `opencode` |
| `retries` | u32 | 0 | Reintentos antes de considerar el step fallido |
| `retry_delay` | u32 | 0 | Segundos entre reintentos |
| `on_error` | enum | `stop` | `stop`, `continue` o `goto:<name>` |
| `on_success` | enum | `continue` | `continue`, `end` o `goto:<name>` |
| `entry_only_via_goto` | bool | false | Solo se ejecuta si otro step lo referencia con `goto` |
| `confirm` | bool | false | Pide confirmación antes de ejecutar |
| `command` | string | - | Solo `command`: comando a ejecutar |
| `cwd` | string | - | Solo `command` |
| `timeout` | u32 | 300 | Timeout en segundos |
| `prompt` | string | - | Solo `claude`/`opencode`: prompt con placeholders |
| `output_format` | string | - | Solo `claude`/`opencode`: `json` para parseo |
| `session_id` | string | - | Solo `claude`/`opencode`: ID de sesión previa |
| `continue_session` | bool | false | Solo `claude`/`opencode`: continuar la sesión anterior |
| `model` | string | - | Solo `opencode`: modelo (`provider/model`) |
| `agent` | string | - | Solo `opencode`: agente |

### Templating

Los prompts y comandos usan [Handlebars](https://handlebarsjs.com/). Están
disponibles `{{steps.<name>.<campo>}}` (con `status`, `stdout`, `stderr`,
`exit_code`, `attempts`), `{{args.<clave>}}` (argumentos pasados por CLI) y,
dentro de `finally`, `{{flow.<campo>}}` (con `status`, `failed_steps` y
`exit_reason`). Las variables faltantes se resuelven a cadena vacía.

### Argumentos por CLI

Podés pasar parámetros a un flujo con `--clave valor` (o `--clave=valor`) y
referenciarlos en el YAML como `{{args.<clave>}}`:

```bash
zek feature --branch mi-rama
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

## Referencia de CLI

```bash
zek init [dir]              # configura por primera vez (o re-configura)
zek config show             # muestra la config actual
zek config set-dir <path>   # cambia la carpeta de trabajo
zek list                    # comandos y flujos
zek commands <nombre>       # muestra un comando
zek ask "<mensaje>"         # pregunta a Claude fuera de flujos
zek completion --shell <sh> # genera completions (bash|zsh|fish|powershell|elvish)
zek <flujo>|<comando>       # resuelve flujo primero, luego comando
zek <flujo> --clave valor   # argumentos accesibles como {{args.clave}}
```

Flags globales: `--dry-run`, `--verbose/-v`, `--debug`, `--timeout-global <seg>`.

Exit codes: `0` success, `2` failed, `3` aborted (loop infinito).

## Desarrollo

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

## Release

El tag de una versión dispara el workflow `.github/workflows/release.yml`
(cargo-dist), que genera binarios multi-OS, installers y una GitHub Release:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

## Licencia

MIT
