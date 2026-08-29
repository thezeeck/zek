# Resumen de estado — zek (para retomar en otra máquina)

**Proyecto:** `zek` — orquestador de flujos de comandos en Rust, con pasos de Claude (`claude -p`).
**Plan maestro:** `PLAN.md` (9 fases).

## Dónde estamos

**Fases 0–7 completas y commiteadas** en `main`. Rama limpia.

```
d3474ea Fase 7: testing (fixtures, fake claude, tests de integracion)
1d10d3d Fase 6: UX de CLI (run, dry-run, resumen, progreso)
5fcf5eb Fase 5: integracion con Claude
50bedf9 Fase 4: motor de flujos con reintentos, goto y finally
633f5d1 Fase 3: motor de ejecucion de comandos
108c3c2 Fase 2: carga de comandos y flujos con validaciones
603dd8f docs: marcar Fase 1 como completada
3e7de77 Fase 0-1: scaffolding del workspace y config con wizard de init
```

**Pendiente:**
- **Fase 8 — Empaquetado** (empezada, sin decidir). Ver "Decisiones abiertas" abajo.
- **Fase 9 — Documentación** (README.md).
- **PLAN.md** tiene marcadas solo Fases 0–1; falta tildar 2–7 (cosmético).

## Estructura del código

```
zek/
├── Cargo.toml               (workspace: zek-core + zek-cli)
├── PLAN.md
├── .github/workflows/ci.yml (fmt + clippy -D warnings + test)
├── zek-core/                (librería)
│   ├── src/
│   │   ├── config.rs        Config + config_dir (XDG/APPDATA)
│   │   ├── error.rs         ZekError + FlowFinalStatus
│   │   ├── step.rs          Step, StepType, OnErrorAction/OnSuccessAction
│   │   ├── commands.rs      Command + load_all
│   │   ├── flows.rs         Flow/Finally + validaciones + LineIndex
│   │   ├── exec.rs          StepExecutionStatus + CommandExecutor + run_program + retries
│   │   ├── context.rs       ExecutionContext + render (handlebars)
│   │   ├── execution.rs     FlowRunner + FlowReport + StepProgress
│   │   ├── claude.rs        ClaudeClient + ClaudeOptions
│   │   ├── parser.rs        extract_json_block + parse_claude_json
│   │   └── util.rs          yaml_files
│   └── tests/               (tests de integración, NÓ usar subdirs: cargo no los descubre)
│       ├── common/mod.rs    Workspace + write_fake_claude + fixture
│       ├── fixtures/*.yaml  valid/invalid_goto/static_cycle/infinite_loop
│       ├── test_commands.rs / test_flows.rs / test_claude.rs
└── zek-cli/                 (binario `zek`)
    └── src/main.rs + commands.rs
```

## Comandos de verificación (siempre correrlos al terminar una fase)

```bash
cargo build --workspace
cargo test --workspace          # 71 unit + 11 integración = 82
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

## Decisiones de diseño tomadas (desviaciones del plan, ya acordadas/implementadas)

1. **`on_error`/`on_success` son `Option`** en `Step` para soportar defaults distintos según scope (`finally` → `continue`, `steps` → `stop`). Resolver con `effective_on_error(bool)/effective_on_success()`.
2. **`command` de un step** se resuelve primero como comando nombrado (commands/), si no como comando crudo. La "validación de que el command exista" es **warning**, no error (por los comandos crudos de `finally`).
3. **Estado final del flujo = `failed` si algún step falló**, aunque llegue a fin natural (semántica CI). Exit codes: `0` success, `2` failed, `3` aborted (loop infinito).
4. **`max_jumps` default 20** (el struct de Fase 2 decía `default_20`; el resto del plan dice 50 — sin resolver, trivial de cambiar).
5. Se **descartaron** los campos redundantes `goto_target`/`go_to`/`entry_only` del struct de Fase 2 (cubiertos por `on_success/on_error: goto` y `entry_only_via_goto`).
6. **Claude**: timeout usa `step.timeout` (default 300s, no 120); `continue_session` default `false`; se usa `--resume <session_id>` para continuar sesión.
7. **`--timeout-global` en segundos** (el plan decía ms).
8. **Progreso a stderr** con líneas (`▶ build (1/3)`, `✓`/`✗`); se reemplazó el spinner `indicatif` porque pelea con el streaming de output (se puede añadir después).
9. **serde_yaml** (deprecated pero funcional); se extrae `file:line` vía `LineIndex` (escaneo de `- name:`) y `Error::location()`.
10. **Tests de integración en `tests/` plano** (cargo NO descubre `tests/subdir/*.rs`). Módulo compartido en `tests/common/mod.rs` con `#![allow(dead_code)]`.

## Funcionalidad implementada (resumen por CLI)

```bash
zek init [dir]              # wizard dialoguer + auto-init si no hay config
zek config show | set-dir <path>
zek list                    # comandos y flujos
zek commands <nombre>
zek ask "<mensaje>"         # claude -p suelto
zek <flujo>|<comando>       # resuelve flujo primero, luego comando
zek --dry-run <flujo>
zek --verbose/--debug --timeout-global <seg> <flujo>
```

- Flujos: `steps` + `finally`, `retries`/`retry_delay`, `on_error` (stop|continue|goto:x), `on_success` (continue|end|goto:x), `entry_only_via_goto`, protección `max_jumps`, ciclos estáticos (Tarjan, warning), `fail_flow_on_error`.
- Claude: template `{{steps.<name>.<campo>}}` y `{{flow.<campo>}}`, `output_format`, `session_id`, `continue_session`.
- Confirmación de steps con `confirm: true`.

## Fase 8 — Empaquetado (pendiente de decidir)

Objetivos del plan: build/install scripts, `cargo-dist` (metadata), GH Actions de release multi-OS, `cargo publish` (una vez), shell completions (`clap_complete`).

**Bloqueado por dos datos que faltan:**
- **URL del repositorio GitHub** (el plan tiene placeholder `https://github.com/youruser/zek`). Se preguntó al usuario y descartó la pregunta.
- **Autor** para metadata. Git config actual: `Juan Sánchez <t-juansanchez@itesm.mx>`.

**Recomendación al retomar:** confirmar con el usuario la URL del repo (o saltar cargo-dist/release y hacer solo):
1. Shell completions con `clap_complete`: subcomando `zek completion --shell <bash|zsh|fish>`.
2. Script local de build/install (`cargo build --release` + copiar binario).

## Notas sueltas

- `claude` está instalado en la máquina pero **sin login** ("Not logged in · Please run /login"), por lo que los pasos `type: claude` fallan hasta autenticar.
- El workdir se configura vía `~/.config/zek/config.yaml` (o `$XDG_CONFIG_HOME/zek`). Para pruebas se usa `XDG_CONFIG_HOME=<tmp>`.

## Cómo retomar

1. `git status` debe estar limpio (todo commiteado). Si clonás, `git clone` + `cargo build`.
2. Resolver las preguntas de la Fase 8 (repo URL / autor).
3. Seguir con Fase 8 → Fase 9 → tildar fases en `PLAN.md` y commitear.
