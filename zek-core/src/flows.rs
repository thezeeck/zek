use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::commands::LoadedCommand;
use crate::error::ZekError;
use crate::step::{Step, StepType};
use crate::util::yaml_files;

pub const DEFAULT_MAX_JUMPS: usize = 20;

fn default_max_jumps() -> usize {
    DEFAULT_MAX_JUMPS
}

/// Bloque opcional `finally` que siempre se ejecuta al finalizar el flujo.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Finally {
    #[serde(default)]
    pub fail_flow_on_error: bool,

    #[serde(default)]
    pub max_jumps: Option<usize>,

    #[serde(default)]
    pub steps: Vec<Step>,
}

/// Flujo definido en `flows/*.yaml`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Flow {
    pub name: String,

    #[serde(default)]
    pub description: String,

    #[serde(default = "default_max_jumps")]
    pub max_jumps: usize,

    #[serde(default)]
    pub steps: Vec<Step>,

    #[serde(default)]
    pub finally: Option<Finally>,
}

/// Un flujo cargado junto con su archivo de origen y sus warnings.
#[derive(Debug, Clone)]
pub struct LoadedFlow {
    pub flow: Flow,
    pub source: PathBuf,
    pub warnings: Vec<String>,
}

/// Índice nombre de step -> número de línea (para errores con `archivo:línea`).
#[derive(Debug, Default)]
pub struct LineIndex {
    lines: HashMap<String, usize>,
}

impl LineIndex {
    pub fn build(text: &str) -> Self {
        let mut lines = HashMap::new();
        for (i, line) in text.lines().enumerate() {
            if let Some(name) = step_name_from_line(line) {
                lines.entry(name.to_string()).or_insert(i + 1);
            }
        }
        Self { lines }
    }

    pub fn line_of(&self, name: &str) -> usize {
        self.lines.get(name).copied().unwrap_or(0)
    }
}

/// Extrae el nombre de un step de una línea `- name: <nombre>`.
fn step_name_from_line(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix('-')?;
    let rest = rest.trim_start();
    let name = rest.strip_prefix("name:")?.trim();
    let name = name.trim_matches('"').trim_matches('\'');
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

impl Flow {
    pub fn from_str(content: &str, path: &Path) -> Result<Self, ZekError> {
        serde_yaml::from_str(content).map_err(|e| {
            let line = e.location().map(|l| l.line()).unwrap_or(0);
            ZekError::Yaml {
                path: path.to_path_buf(),
                line,
                source: e,
            }
        })
    }

    /// Lee, deserializa y valida un flujo, devolviendo (flujo, warnings).
    pub fn load_with_validation(path: &Path) -> Result<(Self, Vec<String>), ZekError> {
        let content = fs::read_to_string(path)?;
        let flow = Self::from_str(&content, path)?;
        let line_index = LineIndex::build(&content);
        let warnings = flow.validate(path, &line_index)?;
        Ok((flow, warnings))
    }

    /// Valida la estructura del flujo. Errores duros -> `Err`; avisos -> `Ok(warnings)`.
    pub fn validate(&self, path: &Path, li: &LineIndex) -> Result<Vec<String>, ZekError> {
        let mut warnings = Vec::new();

        // Nombres duplicados en `steps`.
        let mut seen = HashSet::new();
        for step in &self.steps {
            if !seen.insert(step.name.clone()) {
                return Err(ZekError::validation(
                    path,
                    li.line_of(&step.name),
                    crate::t!(val_step_duplicate, step.name),
                ));
            }
        }
        let main_names: HashSet<String> = self.steps.iter().map(|s| s.name.clone()).collect();

        // goto de los steps principales (debe apuntar dentro de `steps`).
        for step in &self.steps {
            self.validate_step_gotos(step, &main_names, path, li)?;
        }

        // finally.
        if let Some(fin) = &self.finally {
            let mut finally_seen = HashSet::new();
            for step in &fin.steps {
                if main_names.contains(&step.name) {
                    return Err(ZekError::validation(
                        path,
                        li.line_of(&step.name),
                        crate::t!(val_finally_overlap, step.name),
                    ));
                }
                if !finally_seen.insert(step.name.clone()) {
                    return Err(ZekError::validation(
                        path,
                        li.line_of(&step.name),
                        crate::t!(val_finally_duplicate, step.name),
                    ));
                }
            }
            let finally_names: HashSet<String> = fin.steps.iter().map(|s| s.name.clone()).collect();
            for step in &fin.steps {
                self.validate_step_gotos(step, &finally_names, path, li)?;
            }
            self.collect_dead_steps(&fin.steps, &mut warnings);
            self.collect_static_cycles(&fin.steps, &mut warnings);
        }

        self.collect_dead_steps(&self.steps, &mut warnings);
        self.collect_static_cycles(&self.steps, &mut warnings);

        Ok(warnings)
    }

    /// Comandos referenciados por steps `type: command`.
    pub fn referenced_commands(&self) -> Vec<String> {
        let mut refs = Vec::new();
        let mut collect = |steps: &[Step]| {
            for s in steps {
                if s.step_type == StepType::Command {
                    if let Some(cmd) = &s.command {
                        refs.push(cmd.clone());
                    }
                }
            }
        };
        collect(&self.steps);
        if let Some(fin) = &self.finally {
            collect(&fin.steps);
        }
        refs
    }

    /// Advertencias por comandos referenciados que no existen.
    pub fn validate_command_refs(&self, commands: &HashMap<String, LoadedCommand>) -> Vec<String> {
        self.referenced_commands()
            .into_iter()
            .filter(|name| !commands.contains_key(name))
            .map(|name| crate::t!(val_cmd_ref_missing, name))
            .collect()
    }

    fn validate_step_gotos(
        &self,
        step: &Step,
        scope: &HashSet<String>,
        path: &Path,
        li: &LineIndex,
    ) -> Result<(), ZekError> {
        for target in step.goto_targets() {
            if target == step.name {
                return Err(ZekError::validation(
                    path,
                    li.line_of(&step.name),
                    crate::t!(val_goto_self, step.name),
                ));
            }
            if !scope.contains(&target) {
                return Err(ZekError::validation(
                    path,
                    li.line_of(&step.name),
                    crate::t!(val_goto_missing, target, step.name),
                ));
            }
        }
        Ok(())
    }

    fn collect_dead_steps(&self, steps: &[Step], warnings: &mut Vec<String>) {
        let targeted: HashSet<String> = steps.iter().flat_map(|s| s.goto_targets()).collect();
        for step in steps {
            if step.entry_only_via_goto && !targeted.contains(&step.name) {
                warnings.push(crate::t!(val_dead_step, step.name));
            }
        }
    }

    fn collect_static_cycles(&self, steps: &[Step], warnings: &mut Vec<String>) {
        let index: HashMap<&str, usize> = steps
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name.as_str(), i))
            .collect();

        let mut graph: Vec<Vec<usize>> = vec![Vec::new(); steps.len()];
        for (i, step) in steps.iter().enumerate() {
            for target in step.goto_targets() {
                if let Some(&j) = index.get(target.as_str()) {
                    graph[i].push(j);
                }
            }
        }

        for cycle in detect_cycles(&graph) {
            let names: Vec<String> = cycle.iter().map(|&i| steps[i].name.clone()).collect();
            warnings.push(crate::t!(val_static_cycle, names.join(" -> ")));
        }
    }
}

/// Carga todos los flujos de `dir`, indexados por nombre.
pub fn load_all(dir: &Path) -> Result<HashMap<String, LoadedFlow>, ZekError> {
    let mut flows = HashMap::new();
    for path in yaml_files(dir)? {
        let (flow, warnings) = Flow::load_with_validation(&path)?;
        if flows.contains_key(&flow.name) {
            return Err(ZekError::validation(
                &path,
                0,
                crate::t!(val_flow_duplicate, flow.name),
            ));
        }
        flows.insert(
            flow.name.clone(),
            LoadedFlow {
                flow,
                source: path,
                warnings,
            },
        );
    }
    Ok(flows)
}

/// Detección de ciclos con el algoritmo de Tarjan (componentes fuertemente conexas).
fn detect_cycles(graph: &[Vec<usize>]) -> Vec<Vec<usize>> {
    Tarjan::new(graph).run()
}

struct Tarjan<'a> {
    graph: &'a [Vec<usize>],
    index: Vec<Option<usize>>,
    low: Vec<usize>,
    on_stack: Vec<bool>,
    stack: Vec<usize>,
    counter: usize,
    cycles: Vec<Vec<usize>>,
}

impl<'a> Tarjan<'a> {
    fn new(graph: &'a [Vec<usize>]) -> Self {
        let n = graph.len();
        Self {
            graph,
            index: vec![None; n],
            low: vec![0; n],
            on_stack: vec![false; n],
            stack: Vec::new(),
            counter: 0,
            cycles: Vec::new(),
        }
    }

    fn run(mut self) -> Vec<Vec<usize>> {
        for v in 0..self.graph.len() {
            if self.index[v].is_none() {
                self.strongconnect(v);
            }
        }
        self.cycles
    }

    fn strongconnect(&mut self, v: usize) {
        self.index[v] = Some(self.counter);
        self.low[v] = self.counter;
        self.counter += 1;
        self.stack.push(v);
        self.on_stack[v] = true;

        let neighbors = self.graph[v].clone();
        for &w in &neighbors {
            if self.index[w].is_none() {
                self.strongconnect(w);
                self.low[v] = self.low[v].min(self.low[w]);
            } else if self.on_stack[w] {
                self.low[v] = self.low[v].min(self.index[w].unwrap());
            }
        }

        if self.low[v] == self.index[v].unwrap() {
            let mut scc = Vec::new();
            loop {
                let w = self.stack.pop().unwrap();
                self.on_stack[w] = false;
                scc.push(w);
                if w == v {
                    break;
                }
            }
            if scc.len() > 1 {
                self.cycles.push(scc);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "test.yaml";

    fn validate(yaml: &str) -> (Flow, Vec<String>) {
        let path = Path::new(PATH);
        let flow = Flow::from_str(yaml, path).unwrap();
        let li = LineIndex::build(yaml);
        let warnings = flow.validate(path, &li).unwrap();
        (flow, warnings)
    }

    fn validate_err(yaml: &str) -> ZekError {
        let path = Path::new(PATH);
        let flow = Flow::from_str(yaml, path).unwrap();
        let li = LineIndex::build(yaml);
        flow.validate(path, &li).unwrap_err()
    }

    #[test]
    fn flujo_valido_sin_warnings() {
        let (flow, warnings) = validate(
            "name: deploy\nsteps:\n  - name: build\n    type: command\n    command: build\n  - name: test\n    type: command\n    command: test\n",
        );
        assert_eq!(flow.steps.len(), 2);
        assert!(warnings.is_empty());
    }

    #[test]
    fn rechaza_nombres_duplicados() {
        let err = validate_err(
            "name: dup\nsteps:\n  - name: build\n    type: command\n    command: build\n  - name: build\n    type: command\n    command: test\n",
        );
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn rechaza_goto_a_step_inexistente() {
        let err = validate_err(
            "name: g\nsteps:\n  - name: build\n    type: command\n    command: build\n    on_error: goto:noexiste\n",
        );
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn rechaza_goto_a_si_mismo() {
        let err = validate_err(
            "name: s\nsteps:\n  - name: build\n    type: command\n    command: build\n    on_success: goto:build\n",
        );
        assert!(err.to_string().contains("cannot goto itself"));
    }

    #[test]
    fn avisa_step_muerto() {
        let (_, warnings) = validate(
            "name: d\nsteps:\n  - name: build\n    type: command\n    command: build\n  - name: handler\n    type: claude\n    prompt: x\n    entry_only_via_goto: true\n",
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("handler"));
    }

    #[test]
    fn no_avisa_si_step_es_referenciado() {
        let (_, warnings) = validate(
            "name: d\nsteps:\n  - name: build\n    type: command\n    command: build\n    on_error: goto:handler\n  - name: handler\n    type: claude\n    prompt: x\n    entry_only_via_goto: true\n",
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn detecta_ciclo_estatico() {
        let (_, warnings) = validate(
            "name: c\nsteps:\n  - name: a\n    type: command\n    command: a\n    on_error: goto:b\n  - name: b\n    type: command\n    command: b\n    on_error: goto:a\n",
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("cycle"));
    }

    #[test]
    fn rechaza_finally_que_solapa_con_steps() {
        let err = validate_err(
            "name: f\nsteps:\n  - name: build\n    type: command\n    command: build\nfinally:\n  steps:\n    - name: build\n      type: command\n      command: clean\n",
        );
        assert!(err.to_string().contains("overlaps"));
    }

    #[test]
    fn rechaza_goto_de_finally_hacia_steps() {
        let err = validate_err(
            "name: f\nsteps:\n  - name: build\n    type: command\n    command: build\nfinally:\n  steps:\n    - name: clean\n      type: command\n      command: clean\n      on_error: goto:build\n",
        );
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn error_incluye_numero_de_linea() {
        let err = validate_err(
            "name: g\nsteps:\n  - name: build\n    type: command\n    command: build\n    on_error: goto:noexiste\n",
        );
        // el step 'build' está en la línea 3 (1-based)
        assert!(err.to_string().contains("test.yaml:3"));
    }

    #[test]
    fn referenced_commands_recolecta_todos() {
        let (flow, _) = validate(
            "name: f\nsteps:\n  - name: build\n    type: command\n    command: build\n  - name: resumen\n    type: claude\n    prompt: x\nfinally:\n  steps:\n    - name: clean\n      type: command\n      command: clean\n",
        );
        assert_eq!(flow.referenced_commands(), vec!["build", "clean"]);
    }

    #[test]
    fn load_all_indexa_por_nombre() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("deploy.yaml"),
            "name: deploy\nsteps:\n  - name: build\n    type: command\n    command: build\n",
        )
        .unwrap();
        let map = load_all(tmp.path()).unwrap();
        assert_eq!(map.len(), 1);
        assert!(map["deploy"].warnings.is_empty());
    }
}
