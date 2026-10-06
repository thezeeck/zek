# Plan de ejecución de zek

Convertir las doce propuestas del roadmap en entregas verificables, manteniendo la compatibilidad de los flujos existentes. La base de este plan es el código de la versión **0.2.1**.

Las tareas marcadas como pendientes describen funcionalidades por implementar. Sus nombres de campos y opciones son contratos propuestos que deben quedar definidos antes de programar cada entrega. Las entregas implementadas registran abajo su contrato y validación.

## Punto de partida

El proyecto ya incluye comandos reutilizables, flujos secuenciales, grupos `parallel: true`, condiciones, `goto`, subflujos, confirmación, reintentos con demora fija y bloques `finally`. También dispone de integración con Claude y OpenCode, argumentos `{{args.*}}`, configuración por proyecto, reportes JSON/Markdown y logs de texto mediante `--log`.

Estos mecanismos se reutilizan. El historial necesita persistencia estructurada; el parser JSON actual no expone campos de salida de comandos en el contexto; el paralelismo actual depende de grupos consecutivos, sin un grafo de dependencias.

## Orden y dependencias

Seguir el orden de la tabla para una ejecución con un solo responsable. Las dependencias indican qué entregas deben estar terminadas antes de empezar una tarea. P01–P04 y P08 pueden desarrollarse de forma independiente.

| ID | Entrega | Depende de | Estado |
| --- | --- | --- | --- |
| P01 | Variables de flujo y overrides de CLI | — | Implementado; revisión pendiente |
| P02 | Salida JSON de comandos en el contexto | — | Pendiente |
| P03 | Reintentos con backoff exponencial | — | Pendiente |
| P04 | Archivos de entorno y gestores de secretos | — | Pendiente |
| P05 | Ejecución por dependencias con `needs` | — | Pendiente |
| P06 | Visualización con `zek graph` | P05 | Pendiente |
| P07 | Ejecución parcial con `--step` y `--until` | P05, P06 | Pendiente |
| P08 | Historial estructurado y consulta de logs | — | Pendiente |
| P09 | Reejecución con `zek watch` | P08 | Pendiente |
| P10 | Caché de pasos por entradas declaradas | P01, P02, P04, P05, P08 | Pendiente |
| P11 | Notificaciones de finalización | P08 | Pendiente |
| P12 | Exportación a GitHub Actions | P04, P05, P06, P10 | Pendiente |

Cada tarea debe tener un responsable asignado al iniciarse y dividirse en PRs revisables. P04 y P05 se entregan por partes, como se detalla abajo. Registrar el PR y marcar la tarea como completada solo después de cumplir sus criterios de aceptación.

## Fase 1: datos y políticas de ejecución

### P01 — Variables de flujo

**Resultado:** declarar `vars` en el flujo, acceder mediante `{{vars.nombre}}` y sobrescribir valores desde la CLI sin cambiar `{{args.*}}`.

**Archivos:** `zek-core/src/flows.rs`, `context.rs`, `execution.rs`; `zek-cli/src/main.rs`, `commands.rs`.

- [x] Añadir `vars` con valores tipados compatibles con JSON y renderizarlos en comandos, prompts, condiciones y entorno.
- [x] Incorporar `--var clave=valor` antes del nombre del flujo. Documentar el parseo de valores y la precedencia: override de CLI sobre valor YAML.
- [x] Definir el alcance de variables en subflujos: heredar las del padre, aplicar las del hijo y restaurar las del padre al regresar.

**Aceptación:** pruebas de valores escalares y objetos, overrides, variables ausentes y aislamiento entre subflujos; los argumentos actuales conservan su comportamiento.

**Registro de implementación:** responsable, Codex; cambios locales listos para revisión, sin PR creado. API: `Flow::vars`, `FlowRunner::with_vars` y `ExecutionContext::{set_vars, vars}`. JSON válido en `--var` conserva su tipo; texto no JSON queda como string. La última asignación reemplaza el valor completo de la clave, sin merge profundo. Los overrides se aplican al flujo raíz; las claves locales de un hijo prevalecen sobre lo heredado y se restauran al regresar, después de su `finally`, incluso ante errores. `{{args.*}}` y los argumentos posteriores al nombre del flujo mantienen su comportamiento.

**Validación:** 154 pruebas del workspace pasan en Linux, incluyendo 14 pruebas nuevas de tipos, CLI, precedencia, herencia, errores, reintentos y prompts de IA. Clippy con advertencias como errores y comprobación de formato aprobados. Documentación actualizada en los tres README.

### P02 — Salida JSON de comandos

**Resultado:** un paso `command` con `output_format: json` expone `{{steps.nombre.json.campo}}`.

**Archivos:** `zek-core/src/step.rs`, `parser.rs`, `context.rs`, `execution.rs`; reportes en `zek-cli/src/commands.rs`.

- [ ] Añadir un campo opcional de resultado JSON conservando `stdout` y `stderr` originales.
- [ ] Parsear un documento JSON completo en comandos exitosos que soliciten ese formato. Mantener aparte la extracción de bloques utilizada por los clientes de IA.
- [ ] Tratar el JSON inválido como fallo del paso, con diagnóstico visible y aplicación de `retries` y `on_error`.

**Aceptación:** objetos y arrays funcionan en plantillas y condiciones; salida inválida y comandos fallidos conservan su diagnóstico; los reportes incluyen el valor parseado; comandos sin `output_format` mantienen su comportamiento.

### P03 — Backoff exponencial

**Resultado:** habilitar `retry_backoff: true` conservando `retry_delay` como demora inicial y la demora fija como comportamiento por defecto.

**Archivos:** `zek-core/src/step.rs`, `exec.rs`, `execution.rs`.

- [ ] Centralizar el cálculo de espera para comandos, IA y subflujos, evitando políticas distintas entre motores.
- [ ] Definir la secuencia `delay`, `2 × delay`, `4 × delay` y un límite configurable `retry_max_delay`; validar valores y evitar desbordamientos.
- [ ] Registrar la demora aplicada a cada reintento y permitir que una cancelación interrumpa la espera.

**Aceptación:** pruebas con tiempo controlado verifican progresión, límite, demora cero y número de intentos en todos los tipos de paso; los errores de configuración siguen sin reintentarse.

### P04 — Entorno y secretos

**Resultado:** cargar `env_file` y resolver referencias explícitas a secretos sin guardar tokens en las definiciones YAML.

**Archivos:** `zek-core/src/commands.rs`, `step.rs`, `execution.rs`, `util.rs`; nuevo módulo de resolución de entorno; logs y reportes de la CLI.

- [ ] **PR 1:** admitir `env_file` en comandos y pasos, resolver rutas desde el workdir y definir precedencia: entorno del proceso, archivo del comando, `env` del comando, archivo del paso, `env` del paso.
- [ ] **PR 2:** definir una interfaz de proveedor y añadir un adaptador para sops con referencias explícitas, errores identificables y límite de ejecución.
- [ ] **PR 3:** añadir el adaptador de 1Password reutilizando la misma interfaz y pruebas con ejecutables simulados.
- [ ] Aplicar la misma resolución a comandos directos y pasos de flujos; excluir valores resueltos de secretos de logs y reportes, también cuando aparezcan en salidas capturadas.

**Aceptación:** pruebas de precedencia, rutas, archivos ausentes, proveedores fallidos y redacción de secretos; no hay llamadas a proveedores cuando una definición no los solicita. Los adaptadores se prueban sin cuentas ni credenciales reales.

**Salida de fase:** P01–P04 están documentadas en ambos paquetes y los flujos de ejemplo originales siguen ejecutándose igual.

## Fase 2: planificación y selección de pasos

### P05 — Dependencias declaradas y ejecución del grafo

**Resultado:** `needs: [build, test]` determina cuándo puede empezar un paso y permite ejecutar ramas independientes en paralelo.

**Archivos:** `zek-core/src/flows.rs`, `step.rs`, `execution.rs`; nuevo módulo de planificación; vista previa en `zek-cli/src/commands.rs`.

- [ ] **PR 1:** introducir un modo explícito `execution: dag`, manteniendo la ejecución secuencial como valor por defecto. Validar referencias, ciclos y dependencias de un paso hacia sí mismo.
- [ ] Construir un plan compartido por el motor, `--dry-run`, el grafo y la selección parcial. Definir un límite configurable de concurrencia.
- [ ] **PR 2:** ejecutar pasos listos, publicar resultados antes de habilitar dependientes y propagar estados de fallo o salto. Por defecto, `needs` exige dependencias exitosas.
- [ ] Definir `on_error: stop` como detención del arranque de nuevos pasos; esperar los ya activos dentro de sus límites y ejecutar `finally` al cerrar el bloque principal. `continue` permite seguir con ramas independientes.
- [ ] En el modo DAG inicial, rechazar `parallel: true`, `goto` y `entry_only_via_goto`; conservarlos en modo secuencial. Mantener `finally` secuencial y habilitar subflujos sin compartir resultados mutables entre ramas activas.

**Aceptación:** un grafo en diamante ejecuta cada paso una vez; ramas independientes se solapan y respetan el límite; dependientes de pasos fallidos o saltados quedan identificados; ciclos fallan antes de lanzar procesos; fixtures secuenciales y limpieza conservan su comportamiento.

### P06 — Visualización del flujo

**Resultado:** `zek graph <flujo>` muestra relaciones sin ejecutar comandos.

**Archivos:** módulo de planificación de P05; `zek-cli/src/main.rs`, `commands.rs`.

- [ ] Producir una vista de texto y una exportación Mermaid a partir del plan compartido.
- [ ] En modo DAG, dibujar las dependencias; en modo secuencial, representar orden, grupos paralelos y saltos condicionales. Identificar subflujos y `finally`.
- [ ] Etiquetar por separado las aristas de `goto`: un flujo secuencial con saltos puede contener ciclos y no es un DAG.

**Aceptación:** salidas deterministas para los fixtures de ambos modos, nombres escapados correctamente y ningún proceso o proveedor de secretos invocado al visualizar.

### P07 — Ejecución parcial

**Resultado:** depurar un paso o ejecutar hasta un punto definido mediante `zek --step <nombre> <flujo>` y `zek --until <nombre> <flujo>`.

**Archivos:** planificación y ejecución en `zek-core`; argumentos y vista previa en `zek-cli`.

- [ ] Definir `--step` como ejecución exclusiva del paso indicado; rechazar antes de ejecutar si necesita resultados previos que no están disponibles.
- [ ] Definir `--until` como prefijo inclusivo en modo secuencial y como conjunto de ancestros más el destino en modo DAG. Rechazar saltos que salgan del conjunto seleccionado.
- [ ] Validar nombres inexistentes y opciones incompatibles. Aplicar la selección al bloque principal y conservar el bloque `finally` del flujo.
- [ ] Mostrar la selección real con `--dry-run` y distinguir los pasos excluidos de los saltados por condiciones en el reporte.

**Aceptación:** no se ejecutan pasos fuera de la selección; errores de selección se detectan antes de lanzar procesos; las dependencias elegidas y la limpieza se verifican con pruebas de integración de CLI.

**Salida de fase:** motor, vista previa, visualización y ejecución parcial utilizan el mismo plan; el YAML anterior no requiere migración.

## Fase 3: historial y ciclo de desarrollo

### P08 — Historial y logs consultables

**Resultado:** `zek history` lista ejecuciones y `zek logs <run-id>` recupera los eventos de una ejecución concreta.

**Archivos:** nuevo módulo de eventos y persistencia en `zek-core`; `zek-cli/src/main.rs`, `commands.rs`, `config.rs`.

- [ ] Definir eventos versionados con ID de ejecución, flujo, paso, intento, tiempos y estado; incluir finalización, errores internos y cancelaciones.
- [ ] Persistir eventos estructurados y un resumen por ejecución en un directorio configurable. Mantener compatible el log de texto de `--log`.
- [ ] Implementar consulta por flujo y estado, límite de resultados, lectura de logs y una política explícita de retención.

**Aceptación:** dos ejecuciones concurrentes no mezclan eventos; un registro incompleto sigue siendo consultable; IDs y orden son deterministas dentro de cada ejecución; valores de secretos no quedan persistidos cuando se integra P04.

### P09 — Modo watch

**Resultado:** `zek watch <flujo>` reejecuta el flujo ante cambios relevantes de archivos.

**Archivos:** nuevo módulo de observación; CLI y eventos de P08.

- [ ] Añadir patrones de inclusión/exclusión y debounce configurable; excluir por defecto `target/`, historial, logs y caché para evitar reejecuciones causadas por zek.
- [ ] Ejecutar una corrida a la vez. Si llegan cambios mientras corre, acumular una única reejecución al terminar.
- [ ] Recargar y validar definiciones antes de cada corrida; mantener el observador activo ante un YAML inválido y registrar cada corrida en el historial.
- [ ] Manejar interrupción del usuario y cierre de procesos sin dejar nuevas corridas pendientes.

**Aceptación:** cambios agrupados generan una sola corrida; un cambio durante la ejecución genera exactamente una corrida posterior; archivos excluidos no disparan ejecuciones; hay pruebas del observador en las plataformas soportadas.

## Fase 4: reutilización de resultados

### P10 — Caché de pasos

**Resultado:** evitar repetir comandos exitosos cuando sus entradas declaradas no cambiaron.

**Archivos:** nuevo módulo de caché; `step.rs`, `execution.rs`, contexto, reportes y eventos de historial.

- [ ] Limitar la primera entrega a pasos `command` con caché habilitada explícitamente y entradas/salidas declaradas. Excluir por defecto IA y subflujos.
- [ ] Definir una clave versionada con contenido de entradas, comando renderizado, cwd, plataforma, variables y entorno relevantes; evitar almacenar secretos en claro.
- [ ] Restaurar stdout, stderr y JSON del resultado junto con las salidas declaradas, para que los pasos dependientes vean el mismo contexto.
- [ ] Registrar aciertos de caché sin tratarlos como saltos por condición; añadir una opción para ignorar la caché y un comando para limpiarla.

**Aceptación:** cambiar una entrada, variable o comando invalida la clave; resultados fallidos no se reutilizan; entradas vacías o caché corrupta no producen éxitos falsos; restaurar salidas y contexto permite ejecutar correctamente los dependientes.

## Fase 5: integraciones

### P11 — Notificaciones

**Resultado:** enviar un resumen final por Slack, Discord o Telegram cuando el flujo lo solicite mediante `notify`.

**Archivos:** nuevo módulo de notificaciones; configuración y eventos de finalización de P08.

- [ ] Definir filtros de éxito, fallo o aborto y una interfaz común de proveedores. Resolver credenciales por entorno o mediante P04 cuando esté disponible.
- [ ] Entregar primero Slack y después Discord y Telegram en PRs separados, con un payload común: flujo, ID de ejecución, estado, duración y pasos fallidos.
- [ ] Enviar una notificación de finalización por ejecución, aplicar timeout y reintentos acotados, y registrar fallos de envío sin cambiar el resultado del flujo.

**Aceptación:** pruebas con servidores simulados verifican filtros, payloads, fallos y ausencia de duplicados tras errores; no se incluyen secretos ni salidas completas por defecto.

### P12 — Exportación a GitHub Actions

**Resultado:** `zek export github <flujo>` genera un workflow revisable a partir del plan de ejecución.

**Archivos:** nuevo módulo de exportación; CLI y planificación de P05.

- [ ] Definir un subconjunto inicial: comandos secuenciales y DAG, dependencias, cwd, timeout y entorno. Emitir errores explícitos para `goto`, watch, caché, IA y otros mecanismos cuya semántica no pueda preservarse.
- [ ] Mapear dependencias a `needs` y reutilizar las declaraciones de entradas/salidas de P10 para transferir archivos entre jobs; rechazar flujos que dependan de un estado compartido no transferible.
- [ ] Traducir referencias de secretos a referencias de GitHub Actions; validar qué condiciones, reintentos y bloques `finally` pueden representarse antes de exportar.
- [ ] Emitir YAML por stdout o archivo elegido, sin ejecutar el flujo ni publicar el workflow. Incluir en la salida los requisitos de runner y las diferencias de comportamiento admitidas.

**Aceptación:** workflows de fixtures compatibles pasan validación estructural y una ejecución de prueba en CI; casos no representables fallan con diagnóstico; la salida es determinista y no contiene valores de secretos.

## Criterio de cierre de cada entrega

- [ ] Contrato YAML y CLI definido, con defaults, precedencia, errores y comportamiento frente a subflujos, paralelismo y `finally` cuando correspondan.
- [ ] Implementación y pruebas de aceptación de la tarea, incluidas regresiones relevantes de los flujos existentes.
- [ ] Documentación y ejemplos actualizados en `README.md`, `zek-cli/README.md` y `zek-core/README.md` según el alcance; mensajes nuevos disponibles en inglés y español.
- [ ] Comprobaciones del workspace aprobadas y validación de plataforma cuando se modifiquen procesos, archivos o integración con el sistema operativo.
- [ ] PR registrado en este documento y estado de la entrega actualizado.

Ejecutar desde la raíz del repositorio:

```bash
cargo build --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all -- --check
```

## Siguiente paso

Revisar los cambios de **P01** y registrar su PR. La siguiente implementación es **P02**: definir el resultado JSON opcional, exponer `{{steps.nombre.json.campo}}` y conectar errores de parseo con los reintentos y reportes existentes.
