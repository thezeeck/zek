---
name: zek-flow
description: Use when creating, editing, or reviewing zek workflow YAML files (flows/*.yaml and commands/*.yaml). Covers flow/step schema, step types, on_error/on_success, goto, when conditions, parallel steps, finally blocks, Handlebars templating, and validation rules.
---

# Crear flujos de zek

Esta skill describe cómo escribir flujos y comandos para **zek** (orquestador de
flujos de comandos en Rust). Los flujos encadenan steps que ejecutan comandos,
llaman a Claude/OpenCode o invocan otros flujos.

## Dónde van los archivos

```
<workdir>/
├── commands/*.yaml   # Comandos reutilizables
└── flows/*.yaml      # Flujos (un flujo por archivo, extensión .yaml o .yml)
```

El `workdir` se define en `~/.config/zek/config.yaml` (clave `workdir`) o en un
`zek.yaml` por proyecto. Los archivos se cargan desde `<workdir>/commands/` y
`<workdir>/flows/`, indexados por `name`.

## Comandos (`commands/*.yaml`)

| Campo | Tipo | Default | Descripción |
|-------|------|---------|-------------|
| `name` | string | obligatorio | Nombre único entre todos los comandos |
| `run` | string | obligatorio | Comando a ejecutar |
| `description` | string | `""` | Descripción |
| `cwd` | string | - | Directorio de trabajo |
| `timeout` | u32 | `300` | Timeout en segundos |
| `author` | string | - | Autor |
| `env` | map | - | Variables de entorno extra |

```yaml
name: build
description: "Compila el proyecto"
run: "cargo build --release"
cwd: "."
timeout: 300
env:
  RUST_BACKTRACE: "1"
```

## Flujos (`flows/*.yaml`)

Estructura de nivel superior:

| Campo | Tipo | Default | Descripción |
|-------|------|---------|-------------|
| `name` | string | obligatorio | Nombre único entre todos los flujos |
| `description` | string | `""` | Descripción |
| `max_jumps` | usize | `20` | Máximo de saltos `goto` (anti loop infinito) |
| `steps` | list | `[]` | Steps principales (se ejecutan en orden) |
| `finally` | map | - | Bloque que siempre se ejecuta al final |

```yaml
name: deploy
description: "Build, test y resumen"
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

## Tipos de step (`type`)

| Tipo | Uso |
|------|-----|
| `command` | Ejecuta un comando (nombre de un command de `commands/` o comando crudo) |
| `claude` | Envía un prompt a `claude -p` |
| `opencode` | Envía un prompt a `opencode run` |
| `flow` | Invoca otro flujo como subrutina |

## Campos de un step

| Campo | Tipo | Default | Aplica a | Descripción |
|-------|------|---------|----------|-------------|
| `name` | string | obligatorio | todos | Único dentro del flujo |
| `type` | enum | obligatorio | todos | `command`, `claude`, `opencode`, `flow` |
| `retries` | u32 | `0` | todos | Reintentos antes de fallar |
| `retry_delay` | u32 | `0` | todos | Segundos entre reintentos |
| `on_error` | enum | `stop` (steps) / `continue` (finally) | todos | `stop`, `continue`, `goto:<name>` |
| `on_success` | enum | `continue` | todos | `continue`, `end`, `goto:<name>` |
| `entry_only_via_goto` | bool | `false` | todos | Solo se ejecuta si otro step lo referencia con `goto` |
| `confirm` | bool | `false` | todos | Pide confirmación antes de ejecutar |
| `when` | string | - | todos | Condición; si es falsa, se salta el step |
| `parallel` | bool | `false` | todos | Corre en paralelo con steps `parallel: true` consecutivos |
| `env` | map | - | todos | Variables de entorno extra |
| `command` | string | - | `command` | Comando a ejecutar |
| `cwd` | string | - | `command` | Directorio de trabajo |
| `timeout` | u32 | `300` | `command` | Timeout en segundos |
| `flow` | string | - | `flow` | Nombre del flujo a invocar |
| `prompt` | string | - | `claude`/`opencode` | Prompt con placeholders |
| `output_format` | string | - | `claude`/`opencode` | `json` para parseo automático |
| `session_id` | string | - | `claude`/`opencode` | ID de sesión previa |
| `continue_session` | bool | `false` | `claude`/`opencode` | Continuar la sesión anterior |
| `model` | string | - | `opencode` | Modelo (`provider/model`) |
| `agent` | string | - | `opencode` | Agente |

## Reglas de validación (errores duros)

- `name` duplicado dentro de un mismo flujo → error.
- `goto` a un step inexistente → error. El destino debe estar en el mismo scope
  (`steps` solo puede apuntar a `steps`; `finally` solo a `finally`).
- `goto` a sí mismo → error.
- Step `parallel: true` con `on_error` u `on_success` → error.
- `finally` con un step cuyo nombre coincide con uno de `steps` → error.
- `on_error: goto:` (vacío) → error.
- Cargas con nombre duplicado entre archivos (`commands/` o `flows/`) → error.

Los errores se reportan con `archivo:línea`.

## Warnings (no bloquean)

- Step `entry_only_via_goto: true` que nadie referencia con `goto` (step muerto).
- Ciclos estáticos de `goto` (ej. `a` → `b` → `a`), detectados con Tarjan.
- Referencias a `command` o `flow` que no existen.

## Control de flujo (`on_error` / `on_success`)

```yaml
steps:
  - name: build
    type: command
    command: build
    retries: 2
    retry_delay: 5
    on_error: goto:diagnosticar_falla   # salta a otro step si falla

  - name: diagnosticar_falla
    type: opencode
    entry_only_via_goto: true           # solo se ejecuta vía goto
    prompt: "El build falló: {{steps.build.stderr}}"
    on_success: end                     # termina el flujo con éxito

  - name: test
    type: command
    command: test
    on_error: continue                  # ignora el fallo y sigue
```

- `on_error` default: `stop` (en `steps`), `continue` (en `finally`).
- `on_success` default: `continue`. `end` termina el flujo con éxito.

## Bloque `finally`

Siempre se ejecuta al terminar el flujo (con éxito o fallo). Sus steps siguen
las mismas reglas que `steps`, pero su scope de `goto` es independiente.

| Campo | Tipo | Default | Descripción |
|-------|------|---------|-------------|
| `fail_flow_on_error` | bool | `false` | Si un step de `finally` falla, marcar el flujo como fallido |
| `max_jumps` | usize | - | Máximo de saltos dentro de `finally` |
| `steps` | list | `[]` | Steps del bloque |

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

Los `prompt`, `command`, `when` y valores de `env` se renderizan con Handlebars.
Las variables faltantes se resuelven a cadena vacía.

- Resultados de steps: `{{steps.<name>.<campo>}}` con campos `status`, `success`,
  `failed`, `stdout`, `stderr`, `exit_code`, `attempts`, `duration_ms`.
- Argumentos CLI: `{{args.<clave>}}` (pasados con `zek <flujo> --clave valor`).
- Estado del flujo (solo dentro de `finally`): `{{flow.status}}`,
  `{{flow.failed_steps}}`, `{{flow.exit_reason}}`.

```yaml
steps:
  - name: resumen
    type: opencode
    prompt: >
      build={{steps.build.status}} (exit={{steps.build.exit_code}})
      test={{steps.test.status}}
```

## Condicionales (`when`)

Se renderiza con Handlebars y luego se evalúa como expresión booleana. Soporta
comparaciones (`==`, `!=`, `<`, `<=`, `>`, `>=`), lógica (`&&`, `||`, `!`) y
paréntesis. Falsos: cadena vacía, `false`, `no`, `0`.

```yaml
steps:
  - name: diagnosticar
    type: claude
    when: "{{steps.build.failed}}"
    prompt: "El build falló: {{steps.build.stderr}}"
```

## Composición de flujos (`type: flow`)

Invoca otro flujo como subrutina. Sus steps se registran en el mismo contexto,
referenciables con `{{steps.<name>.<campo>}}`.

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

## Steps en paralelo

Steps consecutivos con `parallel: true` corren a la vez; el flujo espera a todos
antes de continuar. No pueden definir `on_error` ni `on_success`.

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

## Variables de entorno y secretos

`env` admite placeholders y referencias a variables del proceso (`$VAR` /
`${VAR}`), para no hardcodear secretos en el YAML.

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

## Verificación

```bash
zek list                    # lista comandos y flujos cargados (valida el parseo)
zek <flujo> --dry-run       # muestra el plan sin ejecutar
zek --debug <flujo>         # detalle de validación y warnings
```

Los tests de referencia viven en `zek-core/tests/fixtures/` y `zek-core/src/flows.rs`
(`validate`), y `zek-core/src/step.rs`.
