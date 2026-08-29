# Plan de Trabajo: zek

> **Estado: completo.** Fases 0–9 finalizadas. El soporte de OpenCode (`type: opencode`)
> se agregó como extra (análogo a la Fase 5 de Claude).

**zek** es una app de terminal escrita en Rust que ejecuta flujos de comandos predefinidos. Se invoca como `zek` y permite:

- Ejecutar comandos definidos en YAML
- Usar flujos que encadenan comandos y pasos de Claude (`claude -p`)
- Reintentos automáticos con manejo de errores por paso
- Comunicación con Claude para diagnóstico y resúmenes

---

## Arquitectura General

```
zek
├── ~/.config/zek/config.yaml     # Ruta de la carpeta del usuario (se pregunta solo 1 vez)
└── <carpeta del usuario>/
    ├── commands/*.yaml           # Comandos reutilizables
    └── flows/*.yaml              # Flujos que encadenan comandos + pasos de Claude
```

### Dependencias Clave

| Crate | Propósito |
|-------|----------|
| `clap` (derive) | Parseo de CLI y subcomandos |
| `serde + serde_yaml` | Definición y parseo de commands/flows |
| `dialoguer` | Wizard interactivo del primer init |
| `directories` | Resolver rutas de config multiplataforma (`~/.config/zek` en Linux/Mac, `%APPDATA%` en Windows) |
| `tokio + tokio::process` | Ejecución de subprocesos (comandos y `claude -p`) |
| `handlebars` | Templating para pasar outputs entre pasos (`{{steps.build.stdout}}`) |
| `anyhow + thiserror` | Manejo de errores |
| `indicatif + console` | Spinners y salida con color |
| `notify` (fase posterior, opcional) | Hot-reload si el usuario edita la carpeta |

---

## Esquema YAML Propuesto

### Comandos (`commands/*.yaml`)

```yaml
name: build
description: "Compila el proyecto"
run: "cargo build --release"
cwd: "."
timeout: 300
```

### Flujos (`flows/*.yaml`)

```yaml
name: deploy
description: "Build, test y resumen con Claude"
steps:
  - name: build
    type: command
    command: build
    retries: 2
    retry_delay: 5
    on_error: stop        # stop | continue | goto:<step_name>

  - name: diagnosticar_falla
    type: claude
    goto_target: true       # solo se ejecuta si build falló y su on_error apunta acá
    prompt: >
      El build falló con este error: {{steps.build.stderr}}.
      ¿Cuál es la causa raíz probable?

  - name: test
    type: command
    command: test
    retries: 1
    on_error: continue

  - name: resumen
    type: claude
    prompt: >
      Resumí el resultado del flujo: build={{steps.build.status}},
      test={{steps.test.status}}.
```

---

## Esquema Completo de Step

```yaml
steps:
  - name: string              # único dentro del flow (obligatorio)
    type: command | claude
    retries: 0                # default 0, reintentos ANTES de considerar el step fallido
    retry_delay: 0             # segundos entre reintentos, default 0
    on_error: stop             # stop | continue | "goto:<step_name>"  (default: stop)
    on_success: continue       # continue | end | "goto:<step_name>"  (default: continue)
    entry_only_via_goto: false # si es true, el step NO corre en el flujo secuencial normal,
                                # solo se ejecuta si algún on_error/on_success apunta a él
```

### Reglas de Ejecución

1. **Retries primero, on_error después.** Un step falla → se reintenta `retries` veces con `retry_delay` entre intentos → si sigue fallando, recién ahí se evalúa `on_error`.

2. **stop corta el flujo inmediatamente**, exit code ≠ 0, no ejecuta nada más.

3. **continue** guarda el resultado como `Failed` en el contexto (disponible para templating en steps siguientes) y avanza al siguiente índice del array.

4. **goto:<step>** salta directo a ese step, sin volver — es un salto real, no una subrutina. Al llegar por `goto`, ese step corre normalmente (incluyendo sus propios retries/on_error).

5. **on_success** funciona igual pero para el camino feliz, con la opción extra `end` para terminar el flow ahí aunque queden steps después en el archivo.

6. **entry_only_via_goto**: si es `true`, el step no corre en el flujo secuencial normal, solo se ejecuta si algún `on_error`/`on_success` apunta a él con `goto`. Esto permite poner handlers (diagnóstico, limpieza) en cualquier posición del YAML sin romper el flujo "feliz".

---

## Bloque `finally` (Nivel Flow)

Un bloque opcional `finally` que contiene steps que **siempre se ejecutan** al finalizar el flujo, pase lo que pase (excepto errores de carga del YAML).

```yaml
finally:
  fail_flow_on_error: false   # default: false. Si true, un error en finally tumba el flow principal
  max_jumps: 5                # independiente del max_jumps de steps, para controlar loops dentro de finally
  steps:
    - name: cleanup
      type: command
      command: "rm -rf ./tmp/build-*"
      retries: 1
      on_error: continue       # en finally, el default de on_error es continue (no stop)

    - name: notificar
      type: claude
      prompt: >
        El flow terminó con status {{flow.status}}.
        Fallaron estos steps: {{flow.failed_steps}}.
        Redactá un mensaje corto para notificar por Slack.
```

### Reglas de Ejecución de `finally`

| Regla | Comportamiento |
|-------|---------------|
| **Siempre corre** | Se ejecuta una sola vez, secuencial, en el orden del YAML. |
| **No altera el resultado** | El exit code y el status final del flow principal (`steps`) quedan fijados antes de entrar a `finally`. Si quieres que un fallo en `finally` tumbe el resultado, debes poner `fail_flow_on_error: true`. |
| **goto restringido** | `goto` solo puede apuntar a otros steps dentro del mismo bloque `finally`. No se puede saltar de `finally` de vuelta a `steps`, ni de `steps` directo a un step de `finally`. |
| **Default de on_error** | En `finally`, el default de `on_error` es `continue` (no `stop`), para no cortar la limpieza a mitad de camino. |
| **Variables de contexto** | Solo dentro de `finally` se ven las variables de flujo: `<code>{{flow.status}}</code>`, `<code>{{flow.failed_steps}}</code>`, `<code>{{flow.exit_reason}}</code>`. |
| **Fallback en templating** | Si `finally` falla por template porque alguna variable no existió en el flujo principal, esa variable se resuelve como cadena vacía + warning en log, sin abortar `finally`. |

### Variables de Contexto en `finally`

| Variable | Contenido |
|----------|----------|
| `{{flow.status}}` | `success` \| `failed` \| `aborted` (por loop infinito) |
| `{{flow.failed_steps}}` | Lista de nombres de steps con status `Failed`, join por coma |
| `{{flow.exit_reason}}` | `"natural_end"` \| `"stop:<step>"` \| `"end:<step>"` \| `"infinite_loop"` |

---

## Fases de Desarrollo

### Fase 0 — Scaffolding

- [x] `cargo new zek --bin` con estructura `lib+bin`
- [x] Repositorio Git inicializado
- [x] `zek-core/` como librería reusable
- [x] `zek-cli/` como binario (llamado `zek`)
- [x] `tests/integration/` para tests de integración
- [x] CI en GitHub Actions: `fmt`, `clippy`, `test` en cada push

**Progreso actual:**
- `zek-core/src/error.rs`: Definición de errores completada (`ZekError`, `StepExecutionStatus`, `FlowFinalStatus`)
- `zek-core/src/config.rs`: Gestión de `~/.config/zek/config.yaml` (migrada a serde en Fase 1)
- `zek-core/src/lib.rs`: Module definitions completas

**Pendientes:**
- `zek-core/src/step.rs`
- `zek-core/src/commands.rs`
- `zek-core/src/flows.rs`
- `zek-core/src/context.rs`

---

### Fase 1 — Config y Primer Arranque

**Objetivo:** `zek init` con wizard interactivo que configura la primera vez.

- [x] `zek init`: wizard con `dialoguer` que pregunta la carpeta, valida, guarda `config.yaml`
  - Si `zek` corre sin config, dispara `init` automáticamente
- [x] `zek config show`: muestra la config actual
- [x] `zek config set-dir <path>`: cambia la carpeta de trabajo
- [x] Validaciones de config:
  - `workdir` sea una ruta válida
  - Carpetas `commands/` y `flows/` existan en el workdir
  - Errores con formato `archivo:línea` (via serde)
- [x] Tests unitarios de `config.rs`

---

### Fase 2 — Carga de Comandos y Flujos

**Objetivo:** Parsear comandos y flujos del YAML con validaciones estrictas.

**Estructuras:**

```rust
// zek-core/src/step.rs
#[derive(Debug, Clone, Deserialize)]
pub struct Step {
    pub name: String,           // único en el flow (obligatorio)
    #[serde(rename = "type")]
    pub step_type: StepType,    // Command | Claude
    #[serde(default = "default_zero")]
    pub retries: u32,
    #[serde(default = "default_zero")]
    pub retry_delay: u32,       // segundos
    #[serde(default = "default_stop")]
    pub on_error: OnErrorAction,
    #[serde(default = "default_continue")]
    pub on_success: OnSuccessAction,
    #[serde(default = "false")]
    pub entry_only_via_goto: bool,
    #[serde(skip)]
    pub command: String,         // solo en CommandStep
    #[serde(skip)]
    pub prompt: String,          // solo en ClaudeStep
    #[serde(skip)]
    pub goto_target: Option<String>, // solo en ClaudeStep
    #[serde(skip)]
    pub cwd: Option<String>,     // solo en CommandStep
    #[serde(skip)]
    pub timeout: u32,            // solo en CommandStep
    #[serde(skip)]
    pub go_to: Option<String>,   // solo en ClaudeStep (para goto al finalizar)
    #[serde(skip)]
    pub confirm: bool,           // true = pedir confirmación antes de correr
    #[serde(skip)]
    pub entry_only: bool,        // alias de entry_only_via_goto
}

pub enum StepType {
    Command,
    Claude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnErrorAction {
    Stop,
    Continue,
    Goto(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnSuccessAction {
    Continue,
    End,
    Goto(String),
}

// zek-core/src/commands.rs
#[derive(Debug, Clone, Deserialize)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub run: String,
    pub cwd: Option<String>,
    #[serde(default = "default_300")]
    pub timeout: u32,
    pub author: Option<String>,
}

// zek-core/src/flows.rs
#[derive(Debug, Clone, Deserialize)]
pub struct Flow {
    pub name: String,
    pub description: String,
    #[serde(default = "default_20")]
    pub max_jumps: usize,
    pub steps: Vec<Step>,
    #[serde(skip)]
    pub step_indices: HashMap<String, usize>,
    pub goto_graph: DirectedGraph<usize>, // mapa de índices -> índices destino por goto
    pub valid_goto: bool,
    pub has_dead_steps: Vec<String>,
    pub has_static_cycles: Vec<String>,
    pub finally_steps: Vec<Step>,
}
```

**Validaciones al cargar:**

1. **Todo `goto:<step>` debe apuntar a un `name` que exista en el mismo flow.** Si no, error de carga con `filename:line_number`.
2. **Un step no puede hacer `goto` a sí mismo** (para eso están los `retries`).
3. **Si un step tiene `entry_only_via_goto: true` pero ningún otro step lo referencia con `goto`:** warning (no error), "step muerto: nunca se alcanza".
4. **Detecar ciclos estáticos:** Se arma el grafo de saltos completo en memoria antes de correr, para poder detectar ciclos estáticos (ciclos simples entre 2-3 steps sin condición) y avisar al cargar.
5. **Nombres duplicados:** Todos los `name` en `steps` deben ser únicos.
6. **Si hay `finally`**, sus steps deben tener `name` único dentro de `finally` y no solaparse con los de `steps`.

**Comandos:**

- `zek list`: lista todos los comandos disponibles con su fuente de archivo
- `zek commands <nombre>`: muestra un command específico
- Validación de que el command exista si se usa en un flow
- Validación de que el flow exista si se invoca directamente

---

### Fase 3 — Motor de Ejecución de Comandos

**Objetivo:** Ejecutar subprocesos, streaming en vivo, captura de outputs, manejo de timeouts y exit codes.

**Estructuras:**

```rust
// zek-core/src/exec.rs
#[derive(Debug, Clone)]
pub enum StepExecutionStatus {
    Success {
        stdout: String,
        stderr: String,
        exit_code: i32,
    },
    Failed {
        stdout: String,
        stderr: String,
        exit_code: i32,
    },
    TimedOut {
        stdout: String,
        stderr: String,
    },
}

pub struct StepExecutor {
    // config: &Config
    // timeout: u32
    // retries: u32
    // retry_delay: u32
    // command: String
    // cwd: Option<String>
    // env: HashMap<String, String>
}

impl StepExecutor {
    pub async fn execute(&self, step: &Step, ctx: &mut ExecutionContext) -> StepExecutionStatus;
}
```

**Comportamiento:**

1. **Ejecución básica:**
   - Spawn `tokio::process::Command::new(command)` con `current_dir(cwd)`
   - Configurar `stdout`, `stderr` como `tokio::process::Stdio::piped()`
   - `env` sobrescribe el entorno actual

2. **Streaming en vivo:**
   - `tokio::spawn` el proceso en un hijo
   - Leer `stdout`/`stderr` en chunks con `read()`
   - Escribir en la terminal con color (usando `console::colors` o `colored`)
   - Cada línea del output se imprime con spinner activo (usando `indicatif`)

3. **Captura de outputs:**
   - Guardar todo el `stdout` y `stderr` en strings después de terminar
   - Estos se inyectan en el `ExecutionContext` para que los siguientes steps los usen en sus templates

4. **Timeout:**
   - `tokio::time::timeout(Duration::from_secs(timeout as u64))` el await del `.wait_with_output()`
   - Si expira: `TimedOut{stdout, stderr}`, propagar a siguiente step

5. **Exit code:**
   - Si termina: `Success{exit_code == 0}` o `Failed{exit_code != 0}`

6. **Retry logic:**
   ```rust
   if status.is_failed() {
       for _ in 0..retries {
           wait retry_delay segundos
           status = execute(...)
           if !status.is_failed() { break }
       }
   }
   ```

7. **Confirmación:**
   - Si `step.confirm == true`, pedir confirmación al usuario con `dialoguer::Confirm`
   - Solo ejecutar si el usuario dice `yes`

8. **Resultado final:** `StepExecutionStatus` guardado en `ExecutionContext` bajo la clave `steps.<step.name>`

---

### Fase 4 — Motor de Flujos (Reintentos + Manejo de Errores + Goto)

**Objetivo:** Ejecutar una secuencia de steps con lógica de control de flujo avanzada.

**Estructuras:**

```rust
// zek-core/src/execution.rs
pub struct ExecutionContext {
    // context compartido entre todos los steps del flow
    // formato: HashMap<String, SerializedStepResult>
    // las claves son: "steps.<name>" o directamente el nombre si es una variable simple
    
    // variables de flujo (solo visibles desde finally)
    flow_status: FlowFinalStatus,
    flow_failed_steps: Vec<String>,
    flow_exit_reason: String,
    
    // para detección de loops en runtime
    jumps: usize,
    jump_history: Vec<JumpRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SerializedStepResult {
    pub status: StepExecutionStatus,
    pub duration: Duration,
    pub attempt: u32,
    // si es CommandStep: stdout, stderr, exit_code
    // si es ClaudeStep: response (JSON string), last_prompt, session_id
}

pub struct ExecutionLoop {
    steps: Vec<Step>,
    ctx: ExecutionContext,
    flow_max_jumps: usize,
    finally_steps: Vec<Step>,
}
```

**Loop del Motor (Pseudocódigo):**

```rust
let mut idx = 0;
let mut jumps = 0;

loop {
    // Fin natural del array
    if idx >= steps.len() { 
        break; 
    }

    let step = &steps[idx];
    
    // entry_only_via_goto: si nadie lo apuntó, se salta
    if step.entry_only_via_goto && !ctx.was_targeted(&step.name) {
        idx += 1;
        continue;
    }

    // Ejecutar con retries AQUÍ
    let result = run_with_retries(step); 
    
    // Guardar resultado en contexto
    ctx.record(&step.name, &result);

    // Decidir qué hacer después
    let next_action = match result.status {
        StepExecutionStatus::Success => step.on_success.clone(),
        StepExecutionStatus::Failed => step.on_error.clone(),
    };

    match parse_action(&next_action) {
        Action::Stop(step_name) => {
            // Corta el flujo, exit code 2, exit_reason = "stop:step_name"
            return Err(FlowFailure {
                final_step: step_name,
                final_status: FlowFinalStatus::Stopped,
                exit_reason: format!("stop:{}", step_name),
                failed_steps: ctx.failed_steps(),
            });
        }
        Action::End => {
            // Termina el flujo, sale del loop
            break;
        }
        Action::Continue => {
            // Avanza al siguiente step (idx + 1)
            idx += 1;
        }
        Action::Goto(target_name) => {
            jumps += 1;
            
            // Protección contra infinite loops
            if jumps > flow_max_jumps {
                return Err(FlowFailure {
                    final_step: "infinite_loop",
                    final_status: FlowFinalStatus::InfiniteLoop,
                    exit_reason: format!("infinite_loop: max jumps {} exceeded", flow_max_jumps),
                    failed_steps: ctx.failed_steps(),
                    cycle_path: detect_cycle_path(&jump_history),
                });
            }
            
            // Salta directo a ese step
            if !ctx.steps.contains(&target_name) {
                return Err(FlowFailure {
                    final_step: target_name,
                    final_status: FlowFinalStatus::NotFound,
                    exit_reason: format!("goto_target not found: {}", target_name),
                    failed_steps: ctx.failed_steps(),
                });
            }
            
            idx = ctx.steps.iter().position(|s| s.name == target_name).unwrap();
            ctx.mark_targeted(&target_name);
        }
    }
}
```

**Protección contra Ciclos:**

1. **Ciclos estáticos:** Detectados en Fase 2 (al cargar el YAML). Si hay 2+ steps en un ciclo cerrado sin posibilidad de salida (sin `end`), lanzar error al cargar.

2. **Ciclos dinámicos:** Detectados en runtime con el contador `jumps`. Si se supera `max_jumps` (`default: 50`), abortar con error: "posible loop infinito: se superaron 50 saltos". Guardar el path del ciclo en el error.

---

### Fase 5 — Integración con Claude

**Objetivo:** Ejecutar pasos del tipo `ClaudeStep` que envían prompts a `claude -p`.

**Estructuras:**

```rust
// zek-core/src/claude.rs

pub struct ClaudeStep {
    pub prompt: String,      // prompt con placeholders
    pub session_id: Option<String>,
    pub output_format: Option<String>,  // "json" para parseo automático
    pub timeout: u32,  // segundos, default 120
    #[serde(default = "default_true")]
    pub continue_session: bool,  // si true, usa --resume para continuar sesión
    // ... resto del Step common
}

// Client wrapper simple:
pub struct ClaudeClient {
    command: String,  // "claude" o path al binary
    config: HashMap<String, String>,
}
```

**Prompt Templating:**

```rust
// Usando handlebars
let rendered_prompt = render_template(&step.prompt, &context);
```

- Se llena el `ExecutionContext` con todos los `steps.<name>` disponibles
- Placeholders: `{{steps.<step_name>.<field>}}`, `{{flow.<field>}}`
- Si un step no corrió (porque se saltó con `goto` o el flow abortó antes), su campo se resuelve como cadena vacía + warning en log

**Ejecución:**

```rust
pub async fn execute_claude_step(&self, step: &Step) -> StepExecutionStatus {
    let prompt = render_template(&step.prompt, &self.ctx);
    
    // Construir argumentos para claude
    let args = vec![
        "-p".to_string(),
        prompt,
    ];
    
    if let Some(format) = &step.output_format {
        args.push("--output-format".to_string());
        args.push(format.clone());
    }
    
    if let Some(session_id) = &step.session_id {
        // ...
    }
    
    if step.continue_session {
        // ... usar --resume ...
    }
    
    let status = execute_command(&self.command, &args, None, step.timeout);
    status
}
```

**Parsing del Output (si es JSON):**

```rust
// zek-core/src/parser.rs
pub fn parse_claude_json(output: &str) -> anyhow::Result<serde_json::Value> {
    // Robear el bloque JSON del output de claude
    // Output suele ser: "Here's the JSON: ```json\n{...}\n```\n\n<extra text>"
    let json_block = extract_json_block(output);
    serde_json::from_str(&json_block)
}

pub fn extract_json_block(text: &str) -> String {
    // Regex simple para encontrar ```json ... ``` o { ... }
    // Retorna el contenido JSON sin los ticks
}
```

**StepResult para ClaudeStep:**

```rust
pub enum StepExecutionStatus {
    // ... comandos ...
    ClaudeResponse {
        response: String,      // el output crudo o parseado
        session_id: String,
    },
}
```

**Continuidad de Sesión:**

- Si `continue_session: true` y el paso anterior del mismo flujo fue de tipo `Claude` con una `session_id`, automáticamente pasamos `--resume <session_id>` para el siguiente `ClaudeStep`
- Esto permite flujos largos sin reiniciar el contexto

**Comando Especial:**

```rust
// zek-cli/src/main.rs
// zek ask "<mensaje>"  // comando suelto, fuera de flujos
```

- Ejecuta directamente `claude -p "<mensaje>"`
- Por defecto: `output-format: none`
- Por defecto: `timeout: 60s`

---

### Fase 6 — UX de CLI

**Subcomandos:**

```bash
zek init <dir>
zek config show
zek config set-dir <dir>
zek <command_name>
zek <flow_name>
zek --dry-run <flow_name>
zek --list
zek --help
```

**Flags Globales:**

| Flag | Propósito |
|------|-----------|
| `--dry-run` | Previsualizar el plan de ejecución sin correrlo (solo parsea y simula) |
| `--verbose`, `-v` | Logs detallados |
| `--debug` | Debug extra (paths, timestamps, full outputs) |
| `--color auto` | Auto-detectar soporte de color |
| `--timeout-global <ms>` | Timeout global para todo el flow |

**Progreso en vivo (indicatif):**

- Spinner: "Ejecutando: build (1/5) ..."
- Al terminar cada step: barra de progreso
- Al terminar el flow: resumen en console con `colored` y `humansize`

**Confirmaciones:**

- Si un step tiene `confirm: true`, pedir confirmación interactiva antes de ejecutarlo
- `y/n` interactivo o silencioso si no se envía por stdin

**Resumen Final:**

```
█████ Flow: deploy ████
Status: failed
Exit code: 2
Duration: 1m 23s

Steps:
  [✓] build (success, 45s, cargo build --release)
  [✗] test (failed, 2x retry, timeout after 60s)
  [→] diagnosticar_falla (skipped, goto_target: true, on_error=goto)
  [✓] diagnosticar_falla (Claude, 32s, session continued)
  [✓] resumen (Claude, 18s, output parsed)

Failed steps: test
Failed reason: timeout

Run: zek --retry-flow max=5
```

**Error messages:**

- Formato unificado: `ZekError { message, source, filename:line, line_col }`
- Para flujo: `FlowFailed: deploy (exit_reason: stop:build)`
- Para step: `StepFailed: build (reason: exit_code=101, stdout: ..., stderr: ...)`

---

### Fase 7 — Testing

**Unit Tests:**

- `config.rs`: load/save, parse, get_base_dir en diferentes OS
- `step.rs`: deserialization de YAML, validaciones, default values
- `commands.rs`: deserialization, validaciones
- `flows.rs`: validation de goto references, dead steps, static cycles
- `context.rs`: template rendering, variable resolution, fallbacks

**Integration Tests:**

```bash
tests/integration/test_commands.rs   # test ejecucion de commands
tests/integration/test_flows.rs      # test ejecucion de flows
tests/integration/test_claude.rs     # test integration con claude
```

**Fixtures:**

- Carpeta temporal con `commands/*.yaml` y `flows/*.yaml`
- YAML fixtures: `valid_flow.yaml`, `invalid_goto.yaml`, `static_cycle.yaml`, `infinite_loop.yaml`

**Fake Binary de Claude (para testeo):**

- Script/binario de prueba que:
  - Lee el prompt del stdin
  - Retorna un JSON predefinido en stdout
  - Simula tiempos de espera
- Permitir testear flujos completos sin API real de claude

**Run Tests:**

```bash
cargo test           # todos los unit tests
cargo test --test integration/test_flows.rs  # solo integration
```

---

### Fase 8 — Empaquetado

**Scripts de Build:**

```bash
# macOS / Linux
cargo build --release
cp target/release/zek /usr/local/bin/

# Windows
cargo build --release
install target\release\zek.exe
```

**cargo-dist:**

```toml
[package.metadata.dist]
name = "zek"
authors = ["Your Name"]
categories = ["command-line-utilities", "development-tools"]
description = "Terminal workflow orchestrator with Claude"
license = "MIT"
homepage = "https://github.com/youruser/zek"
repository = "https://github.com/youruser/zek"
keywords = ["cli", "workflow", "rust", "terminal", "claude"]

[package.metadata.dist.wheels]
target = ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-apple-darwin"]

[package.metadata.dist.npm]
name = "zek-cli"
```

**GH Actions:**

- Build para cada OS
- Check para `~/.config/zek` paths
- Upload binaries para dist

**Publicación:**

- `cargo login` y `cargo publish` en `zek-core` (una sola vez)
- Crear release en GitHub
- Publish via cargo-dist

**Shell Completions:**

- `clap_complete`: genera bash, zsh, fish
- `zek completion --shell <bash|zsh|fish>`
- Commit a `.git/`

---

### Fase 9 — Documentación

**README.md:**

```markdown
# zek

Terminal workflow orchestrator que ejecuta comandos y flujos con reintentos y Claude.

## Instalación

```bash
brew install zek        # macOS
cargo install zek       # cualquier sistema
# o descarga del release de GitHub
```

## Primeros Pasos

```bash
# Configurar tu carpeta de trabajo
zek init ~/proyectos/my-project

# Listar comandos y flujos disponibles
zek list

# Ejecutar un flujo
zek deploy

# Ejecutar un comando directamente
zek build

# Preguntar a Claude (modo interactivo)
zek ask "¿Cómo optimizo este código?"
```

## Estructura del Carpetas

```
~/config/zek
├── config.yaml
├── <workdir>/
│   ├── commands/
│   │   ├── build.yaml
│   │   ├── test.yaml
│   │   └── deploy.yaml
│   └── flows/
│       └── deploy.yaml
```

## Ejemplos de Comandos (commands/*.yaml)

```yaml
# build.yaml
name: build
description: "Compila el proyecto"
run: "cargo build --release"
cwd: "."
timeout: 300
```

## Ejemplos de Flujos (flows/*.yaml)

### Flujo Básico

```yaml
name: deploy
steps:
  - name: build
    type: command
    command: build
    retries: 2
    on_error: stop

  - name: test
    type: command
    command: test
    retries: 1
    on_error: continue
```

### Flujo con Claude

```yaml
name: deploy_with_review
steps:
  - name: build
    type: command
    command: build
    retries: 2
    on_error: goto:diagnosticar_falla

  - name: diagnosticar_falla
    type: claude
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
    type: claude
    prompt: >
      Resumí el resultado del flujo:
      - build={{steps.build.status}} ({{steps.build.exit_code}})
      - test={{steps.test.status}} ({{steps.test.exit_code}}).
    on_success: end
```

### Flujo con Finally

```yaml
name: deploy_with_cleanup
steps:
  - name: build
    type: command
    command: build
    retries: 2
    on_error: goto:diagnosticar_falla

  - name: test
    type: command
    command: test
    retries: 1
    on_error: continue

finally:
  fail_flow_on_error: false
  steps:
    - name: cleanup
      type: command
      command: "rm -rf ./tmp/build-*"
      retries: 1
      on_error: continue

    - name: notificar
      type: claude
      prompt: >
        El flow terminó con status {{flow.status}}.
        Fallaron estos steps: {{flow.failed_steps}}.
        Redactá un mensaje corto para notificar por Slack.
```

## Esquema YAML

### Command

| Campo | Tipo | Default | Descripción |
|-------|------|---------|-------------|
| `name` | string | obligatorio | Nombre único del command |
| `description` | string | "" | Descripción humana |
| `run` | string | obligatorio | Comando a ejecutar |
| `cwd` | string | "." | Directorio de trabajo |
| `timeout` | u32 | 300 | Timeout en segundos |
| `confirm` | bool | false | Pedir confirmación antes de ejecutar |

### Step

| Campo | Tipo | Default | Descripción |
|-------|------|---------|-------------|
| `name` | string | obligatorio | Nombre único en el flow |
| `type` | enum | obligatorio | `command` o `claude` |
| `retries` | u32 | 0 | Reintentos antes de considerar fallido |
| `retry_delay` | u32 | 0 | Segundos entre reintentos |
| `on_error` | enum | `stop` | Qué hacer si falla: `stop`, `continue`, `goto:<name>` |
| `on_success` | enum | `continue` | Qué hacer si logra: `continue`, `end`, `goto:<name>` |
| `entry_only_via_goto` | bool | false | Solo entra si alguien lo llama con goto |
| `command` | string | - | Solo en type=command: comando a ejecutar |
| `prompt` | string | - | Solo en type=claude: prompt con placeholders |
| `cwd` | string | - | Solo en type=command |
| `timeout` | u32 | - | Solo en type=command |
| `session_id` | string | - | Solo en type=claude: ID de sesión anterior |
| `continue_session` | bool | false | Solo en type=claude: reutilizar sesión |
| `confirm` | bool | false | Pedir confirmación |
| `go_to` | string | - | Solo en type=claude: a dónde ir al finalizar |

### Flow

| Campo | Tipo | Default | Descripción |
|-------|------|---------|-------------|
| `name` | string | obligatorio | Nombre único del flow |
| `description` | string | "" | Descripción |
| `max_jumps` | usize | 50 | Max saltos antes de abortar por loop |
| `steps` | array | obligatorio | Lista de steps |
| `finally` | object | - | Bloque finally opcional |
  | `steps` | array | - | Lista de steps en finally |
  | `fail_flow_on_error` | bool | false | Si true, error en finally tumba el flow |

## Variab

Es de tipo `String`.
Es de tipo `string`.