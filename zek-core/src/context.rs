use std::collections::{HashMap, HashSet};
use std::time::Duration;

use handlebars::Handlebars;
use serde_json::json;

use crate::error::{FlowFinalStatus, ZekError};
use crate::exec::StepExecutionStatus;

/// Resultado serializado de un step, guardado en el contexto del flujo.
#[derive(Debug, Clone, PartialEq)]
pub struct SerializedStepResult {
    pub status: StepExecutionStatus,
    pub duration: Duration,
    pub attempts: u32,
}

/// Contexto compartido entre todos los steps de un flujo.
#[derive(Debug, Default, Clone)]
pub struct ExecutionContext {
    results: HashMap<String, SerializedStepResult>,
    pub(crate) recorded: Vec<String>,
    pub(crate) history: Option<crate::history::HistoryRun>,
    pub(crate) skip_reasons: HashMap<String, String>,
    /// Orden de grabación, para reportar pasos fallidos de forma determinista.
    order: Vec<String>,
    /// Estado final del flujo (se fija antes de ejecutar `finally`).
    pub flow_status: Option<FlowFinalStatus>,
    /// Razón de salida: `natural_end`, `stop:<step>`, `end:<step>`, `infinite_loop`.
    pub exit_reason: String,
    /// Steps que fueron alcanzados vía `goto` (para `entry_only_via_goto`).
    targeted: HashSet<String>,
    /// Argumentos pasados por CLI (accesibles como `{{args.<clave>}}`).
    args: HashMap<String, String>,
    /// Variables del scope activo; no se mezclan con los argumentos de CLI.
    vars: HashMap<String, serde_json::Value>,
    /// Pila de invocaciones activa, independiente de los saltos `goto`.
    pub(crate) flow_stack: Vec<String>,
}

impl ExecutionContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, name: impl Into<String>, result: SerializedStepResult) {
        let name = name.into();
        if !self.recorded.contains(&name) {
            self.recorded.push(name.clone());
        }
        if !self.results.contains_key(&name) {
            self.order.push(name.clone());
        }
        self.results.insert(name, result);
    }

    pub(crate) fn record_skip(&mut self, name: String, reason: &str) -> Result<(), ZekError> {
        if let Some(history) = &self.history {
            history.skip(&self.flow_stack.join("/"), &name, reason)?;
        }
        self.skip_reasons.insert(name, reason.into());
        Ok(())
    }

    pub(crate) fn clear_recorded(&mut self) {
        self.recorded.clear();
    }

    pub fn get(&self, name: &str) -> Option<&SerializedStepResult> {
        self.results.get(name)
    }

    pub fn results(&self) -> &HashMap<String, SerializedStepResult> {
        &self.results
    }

    /// Resultados en orden de ejecución.
    pub fn ordered_results(&self) -> Vec<(String, &SerializedStepResult)> {
        self.order
            .iter()
            .filter_map(|name| self.results.get(name).map(|r| (name.clone(), r)))
            .collect()
    }

    pub fn mark_targeted(&mut self, name: &str) {
        self.targeted.insert(name.to_string());
    }

    pub fn was_targeted(&self, name: &str) -> bool {
        self.targeted.contains(name)
    }

    pub fn set_flow_result(&mut self, status: FlowFinalStatus, exit_reason: String) {
        self.flow_status = Some(status);
        self.exit_reason = exit_reason;
    }

    /// Fija los argumentos pasados por CLI, disponibles como `{{args.<clave>}}`.
    pub fn set_args(&mut self, args: HashMap<String, String>) {
        self.args = args;
    }

    /// Reemplaza las variables del scope activo, disponibles como `{{vars.*}}`.
    pub fn set_vars(&mut self, vars: HashMap<String, serde_json::Value>) {
        self.vars = vars;
    }

    pub fn vars(&self) -> &HashMap<String, serde_json::Value> {
        &self.vars
    }

    /// Nombres de steps cuyo estado final no fue exitoso, en orden de ejecución.
    pub fn failed_step_names(&self) -> Vec<String> {
        self.order
            .iter()
            .filter(|name| {
                self.results
                    .get(name.as_str())
                    .map(|r| !r.status.is_success())
                    .unwrap_or(false)
            })
            .cloned()
            .collect()
    }

    /// Renderiza un template con handlebars, exponiendo `{{steps.<name>.<campo>}}`
    /// `{{args.<clave>}}`, `{{vars.<clave>}}` y `{{flow.<campo>}}`.
    /// Las variables faltantes se resuelven a cadena vacía.
    pub fn render(&self, template: &str) -> Result<String, ZekError> {
        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        let data = self.as_json_value();
        handlebars
            .render_template(template, &data)
            .map_err(|e| ZekError::Template(e.to_string()))
    }

    fn as_json_value(&self) -> serde_json::Value {
        let mut steps = serde_json::Map::new();
        for (name, result) in &self.results {
            steps.insert(
                name.clone(),
                json!({
                    "status": result.status.status_str(),
                    "success": result.status.is_success(),
                    "failed": result.status.is_failed(),
                    "stdout": result.status.stdout(),
                    "stderr": result.status.stderr(),
                    "exit_code": result.status.exit_code(),
                    "attempts": result.attempts,
                    "duration_ms": result.duration.as_millis(),
                }),
            );
        }

        let args = serde_json::to_value(&self.args).unwrap_or_else(|_| json!({}));

        json!({
            "steps": steps,
            "args": args,
            "vars": self.vars,
            "flow": {
                "status": self.flow_status.map(|s| s.as_str()).unwrap_or(""),
                "failed_steps": self.failed_step_names().join(","),
                "exit_reason": self.exit_reason,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarda_y_recupera_resultados() {
        let mut ctx = ExecutionContext::new();
        let result = SerializedStepResult {
            status: StepExecutionStatus::Success {
                stdout: "ok".into(),
                stderr: String::new(),
                exit_code: 0,
            },
            duration: Duration::from_millis(10),
            attempts: 1,
        };
        ctx.record("build", result.clone());
        assert_eq!(ctx.get("build"), Some(&result));
        assert_eq!(ctx.results().len(), 1);
        assert!(ctx.get("noexiste").is_none());
    }

    #[test]
    fn rastrea_targeted() {
        let mut ctx = ExecutionContext::new();
        assert!(!ctx.was_targeted("x"));
        ctx.mark_targeted("x");
        assert!(ctx.was_targeted("x"));
    }

    #[test]
    fn lista_pasos_fallidos_en_orden() {
        let mut ctx = ExecutionContext::new();
        let ok = SerializedStepResult {
            status: StepExecutionStatus::Success {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            },
            duration: Duration::ZERO,
            attempts: 1,
        };
        let fail = SerializedStepResult {
            status: StepExecutionStatus::Failed {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 1,
            },
            duration: Duration::ZERO,
            attempts: 1,
        };
        ctx.record("a", fail.clone());
        ctx.record("b", ok);
        ctx.record("c", fail);
        assert_eq!(ctx.failed_step_names(), vec!["a", "c"]);
    }

    #[test]
    fn render_expande_steps_y_flow() {
        let mut ctx = ExecutionContext::new();
        ctx.record(
            "build",
            SerializedStepResult {
                status: StepExecutionStatus::Success {
                    stdout: "compilado".into(),
                    stderr: String::new(),
                    exit_code: 0,
                },
                duration: Duration::ZERO,
                attempts: 1,
            },
        );
        ctx.set_flow_result(FlowFinalStatus::Success, "natural_end".into());

        let rendered = ctx
            .render("build={{steps.build.status}} out={{steps.build.stdout}} flow={{flow.status}}")
            .unwrap();
        assert_eq!(rendered, "build=success out=compilado flow=success");
    }

    #[test]
    fn render_resuelve_variable_faltante_a_vacio() {
        let ctx = ExecutionContext::new();
        let rendered = ctx.render("x={{steps.noexiste.stdout}}").unwrap();
        assert_eq!(rendered, "x=");
    }

    #[test]
    fn render_expande_args() {
        let mut ctx = ExecutionContext::new();
        let mut args = HashMap::new();
        args.insert("branch".to_string(), "feature-x".to_string());
        ctx.set_args(args);

        let rendered = ctx.render("branch={{args.branch}}").unwrap();
        assert_eq!(rendered, "branch=feature-x");
    }
}
