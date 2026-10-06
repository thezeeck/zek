//! Validated execution plans shared by execution, previews and graph output.
use crate::{error::ZekError, flows::Flow, step::Step};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionMode {
    #[default]
    Sequential,
    Dag,
}

pub fn default_concurrency() -> usize {
    4
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum Selection {
    #[default]
    All,
    Step(String),
    Until(String),
}

#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    pub mode: ExecutionMode,
    /// Original YAML indices, in declaration order.
    pub selected: Vec<usize>,
    pub excluded: Vec<String>,
    /// Prerequisites indexed by original YAML position.
    pub dependencies: Vec<Vec<usize>>,
    pub max_concurrency: usize,
    partial: bool,
}

fn invalid(message: impl Into<String>) -> ZekError {
    ZekError::InvalidConfig(message.into())
}

impl ExecutionPlan {
    pub fn build(flow: &Flow, selection: &Selection) -> Result<Self, ZekError> {
        let names: HashMap<&str, usize> = flow
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name.as_str(), i))
            .collect();
        if names.len() != flow.steps.len() {
            return Err(invalid(crate::lang::messages().plan_duplicate));
        }
        if flow.max_concurrency == 0 {
            return Err(invalid(crate::lang::messages().plan_concurrency));
        }
        let mut dependencies = vec![Vec::new(); flow.steps.len()];
        for (i, step) in flow.steps.iter().enumerate() {
            if flow.execution == ExecutionMode::Sequential {
                if !step.needs.is_empty() {
                    return Err(invalid(crate::t!(plan_needs_mode, step.name)));
                }
            } else {
                if step.parallel || step.entry_only_via_goto || !step.goto_targets().is_empty() {
                    return Err(invalid(crate::t!(plan_dag_controls, step.name)));
                }
                for dependency in &step.needs {
                    let j = *names.get(dependency.as_str()).ok_or_else(|| {
                        invalid(crate::t!(plan_dependency_missing, step.name, dependency))
                    })?;
                    if j == i {
                        return Err(invalid(crate::t!(plan_dependency_self, step.name)));
                    }
                    if !dependencies[i].contains(&j) {
                        dependencies[i].push(j);
                    }
                }
            }
        }
        // Kahn's algorithm validates all nodes, even ones excluded by selection.
        let mut visited = vec![false; flow.steps.len()];
        loop {
            let ready: Vec<_> = (0..visited.len())
                .filter(|&i| !visited[i] && dependencies[i].iter().all(|&j| visited[j]))
                .collect();
            if ready.is_empty() {
                break;
            }
            for i in ready {
                visited[i] = true;
            }
        }
        if visited.iter().any(|v| !v) {
            return Err(invalid(crate::lang::messages().plan_cycle));
        }
        if let Some(fin) = &flow.finally {
            for s in &fin.steps {
                if !s.needs.is_empty() || (flow.execution == ExecutionMode::Dag && s.parallel) {
                    return Err(invalid(crate::t!(plan_finally_invalid, s.name)));
                }
            }
        }
        let mut included = vec![matches!(selection, Selection::All); flow.steps.len()];
        match selection {
            Selection::All => {}
            Selection::Step(name) | Selection::Until(name) => {
                let target = *names
                    .get(name.as_str())
                    .ok_or_else(|| invalid(crate::t!(plan_unknown_step, name)))?;
                included[target] = true;
                if matches!(selection, Selection::Until(_)) {
                    if flow.execution == ExecutionMode::Sequential {
                        included[..=target].fill(true);
                    } else {
                        let mut pending = vec![target];
                        while let Some(i) = pending.pop() {
                            for &j in &dependencies[i] {
                                if !included[j] {
                                    included[j] = true;
                                    pending.push(j);
                                }
                            }
                        }
                    }
                }
            }
        }
        for (i, step) in flow.steps.iter().enumerate().filter(|(i, _)| included[*i]) {
            for &j in &dependencies[i] {
                if !included[j] {
                    return Err(invalid(crate::t!(
                        plan_excluded_dependency,
                        step.name,
                        flow.steps[j].name
                    )));
                }
            }
            for target in step.goto_targets() {
                if !names.get(target.as_str()).is_some_and(|&j| included[j]) {
                    return Err(invalid(crate::t!(plan_external_goto, step.name, target)));
                }
            }
        }
        Ok(Self {
            mode: flow.execution,
            selected: (0..included.len()).filter(|&i| included[i]).collect(),
            excluded: flow
                .steps
                .iter()
                .enumerate()
                .filter(|(i, _)| !included[*i])
                .map(|(_, s)| s.name.clone())
                .collect(),
            dependencies,
            max_concurrency: flow.max_concurrency,
            partial: !matches!(selection, Selection::All),
        })
    }

    /// Reject references to unavailable results before any selected process starts.
    /// Also inspect reusable command bodies and cleanup templates.
    pub fn validate_inputs(
        &self,
        flow: &Flow,
        commands: &HashMap<String, crate::commands::LoadedCommand>,
    ) -> Result<(), ZekError> {
        self.validate_catalog_inputs(flow, commands, &HashMap::new())
    }

    /// Preflight templates in selected subflows with the caller's available results.
    pub fn validate_catalog_inputs(
        &self,
        flow: &Flow,
        commands: &HashMap<String, crate::commands::LoadedCommand>,
        flows: &HashMap<String, crate::flows::LoadedFlow>,
    ) -> Result<(), ZekError> {
        if !self.partial {
            return Ok(());
        }
        crate::flows::validate_flow_cycles(flows)?;
        validate_nested_inputs(flow, self, commands, flows, &HashSet::new())
    }

    /// Deterministic graph; goto edges are labeled separately from normal order.
    pub fn graph(&self, flow: &Flow, mermaid: bool) -> String {
        let mut out = if mermaid {
            "flowchart TD\n".to_string()
        } else {
            format!("{} ({:?})\n", flow.name.replace('\n', "\\n"), self.mode)
        };
        let mut edges = Vec::new();
        for &i in &self.selected {
            let step = &flow.steps[i];
            let label = step_label(step);
            if mermaid {
                out.push_str(&format!("  s{i}[\"{}\"]\n", escape(&label)));
            } else {
                out.push_str(&format!("  {}\n", label.replace('\n', "\\n")));
            }
            if self.mode == ExecutionMode::Dag {
                for &j in &self.dependencies[i] {
                    edges.push((j, i, "needs".to_string()));
                }
            }
            for target in step.goto_targets() {
                if let Some(j) = flow.steps.iter().position(|s| s.name == target) {
                    edges.push((i, j, "goto (conditional)".to_string()));
                }
            }
        }
        if self.mode == ExecutionMode::Sequential {
            let mut groups: Vec<Vec<usize>> = Vec::new();
            for &i in &self.selected {
                if flow.steps[i].parallel
                    && groups.last().is_some_and(|g| flow.steps[g[0]].parallel)
                {
                    groups.last_mut().unwrap().push(i);
                } else {
                    groups.push(vec![i]);
                }
            }
            for pair in groups.windows(2) {
                for &a in &pair[0] {
                    for &b in &pair[1] {
                        edges.push((a, b, "order".into()));
                    }
                }
            }
        }
        for (a, b, kind) in edges {
            if mermaid {
                out.push_str(&format!("  s{a} -->|\"{kind}\"| s{b}\n"));
            } else {
                out.push_str(&format!(
                    "  {:?} -> {:?} ({kind})\n",
                    flow.steps[a].name, flow.steps[b].name
                ));
            }
        }
        if let Some(fin) = &flow.finally {
            if mermaid {
                out.push_str("  subgraph cleanup[\"finally (always, sequential)\"]\n");
            } else {
                out.push_str("  finally (always, sequential):\n");
            }
            for (i, step) in fin.steps.iter().enumerate() {
                if mermaid {
                    out.push_str(&format!("    f{i}[\"{}\"]\n", escape(&step_label(step))));
                    if i > 0 {
                        out.push_str(&format!("    f{} --> f{i}\n", i - 1));
                    }
                } else {
                    out.push_str(&format!("    {}\n", step_label(step).replace('\n', "\\n")));
                }
                for target in step.goto_targets() {
                    if let Some(j) = fin.steps.iter().position(|s| s.name == target) {
                        if mermaid {
                            out.push_str(&format!("    f{i} -. goto .-> f{j}\n"));
                        } else {
                            out.push_str(&format!("    {:?} -> {target:?} (goto)\n", step.name));
                        }
                    }
                }
            }
            if mermaid {
                out.push_str("  end\n");
            }
        }
        if !self.excluded.is_empty() && !mermaid {
            out.push_str(&format!("  excluded: {:?}\n", self.excluded));
        }
        out
    }
}
fn step_label(step: &Step) -> String {
    let mut label = format!("{} ({:?})", step.name, step.step_type);
    if let Some(flow) = &step.flow {
        label.push_str(&format!(": {flow}"));
    }
    if step.parallel {
        label.push_str(" [parallel]");
    }
    if step.when.is_some() {
        label.push_str(" [when]");
    }
    if step.entry_only_via_goto {
        label.push_str(" [goto only]");
    }
    label
}
fn escape(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '&' => "&amp;".into(),
            '"' => "&quot;".into(),
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            '\n' => "<br/>".into(),
            c if !c.is_ascii_alphanumeric() && c != ' ' => format!("#{};", c as u32),
            c => c.to_string(),
        })
        .collect()
}
fn validate_step_inputs(
    step: &Step,
    commands: &HashMap<String, crate::commands::LoadedCommand>,
    available: &HashSet<String>,
) -> Result<(), ZekError> {
    let mut values: Vec<&str> = step.when.iter().map(String::as_str).collect();
    if step.step_type != crate::step::StepType::Flow {
        values.extend(step.env.values().map(String::as_str));
    }
    match step.step_type {
        crate::step::StepType::Command => {
            if let Some(command) = step.command.as_ref().and_then(|name| commands.get(name)) {
                values.push(&command.command.run);
                values.extend(
                    command
                        .command
                        .env
                        .iter()
                        .filter(|(key, _)| !step.env.contains_key(*key))
                        .map(|(_, value)| value.as_str()),
                );
            } else if let Some(command) = &step.command {
                values.push(command);
            }
        }
        crate::step::StepType::Claude | crate::step::StepType::Opencode => {
            if let Some(prompt) = &step.prompt {
                values.push(prompt);
            }
        }
        crate::step::StepType::Flow => {}
    }
    for value in values {
        for name in result_references(value)? {
            if !available.contains(name.as_str()) {
                return Err(invalid(crate::t!(plan_unavailable_result, step.name, name)));
            }
        }
    }
    Ok(())
}
// Parse real Handlebars expressions so escaped tags, comments, helpers,
// triple braces and bracketed names have the same syntax as execution.
fn result_references(value: &str) -> Result<Vec<String>, ZekError> {
    let template =
        handlebars::Template::compile(value).map_err(|e| ZekError::Template(e.to_string()))?;
    let mut names = Vec::new();
    visit_template(&template, &mut names);
    Ok(names)
}
fn visit_template(template: &handlebars::Template, names: &mut Vec<String>) {
    for element in &template.elements {
        visit_element(element, names);
    }
}
fn visit_element(element: &handlebars::template::TemplateElement, names: &mut Vec<String>) {
    use handlebars::template::TemplateElement::*;
    match element {
        Expression(h) | HtmlExpression(h) | HelperBlock(h) => {
            visit_parameter(&h.name, names);
            for parameter in h.params.iter().chain(h.hash.values()) {
                visit_parameter(parameter, names);
            }
            for template in h.template.iter().chain(h.inverse.iter()) {
                visit_template(template, names);
            }
        }
        DecoratorExpression(h) | DecoratorBlock(h) | PartialExpression(h) | PartialBlock(h) => {
            visit_parameter(&h.name, names);
            for parameter in h.params.iter().chain(h.hash.values()) {
                visit_parameter(parameter, names);
            }
            if let Some(template) = &h.template {
                visit_template(template, names);
            }
        }
        RawString(_) | Comment(_) => {}
    }
}
fn visit_parameter(parameter: &handlebars::template::Parameter, names: &mut Vec<String>) {
    use handlebars::template::Parameter;
    match parameter {
        Parameter::Path(path) => {
            let raw = match path {
                handlebars::Path::Relative((_, raw)) | handlebars::Path::Local((_, _, raw)) => {
                    raw.as_str()
                }
            };
            let raw = raw
                .trim_start_matches("../")
                .strip_prefix("@root.")
                .or_else(|| raw.trim_start_matches("../").strip_prefix("@root/"))
                .unwrap_or(raw.trim_start_matches("../"));
            let raw = raw
                .strip_prefix("this.")
                .or_else(|| raw.strip_prefix("this/"))
                .unwrap_or(raw);
            if let Some(rest) = raw
                .strip_prefix("steps.")
                .or_else(|| raw.strip_prefix("steps/"))
            {
                let name = if let Some(bracket) = rest.strip_prefix('[') {
                    bracket
                        .split(']')
                        .next()
                        .unwrap_or("")
                        .trim_matches(['\'', '"'])
                } else {
                    rest.split(['.', '/']).next().unwrap_or("")
                };
                if !name.is_empty() {
                    names.push(name.into());
                }
            } else if raw == "steps" {
                names.push("<dynamic steps lookup>".into());
            }
        }
        Parameter::Subexpression(expression) => visit_element(expression.as_element(), names),
        _ => {}
    }
}
fn validate_nested_inputs(
    flow: &Flow,
    plan: &ExecutionPlan,
    commands: &HashMap<String, crate::commands::LoadedCommand>,
    flows: &HashMap<String, crate::flows::LoadedFlow>,
    inherited: &HashSet<String>,
) -> Result<(), ZekError> {
    for &i in &plan.selected {
        let mut available = inherited.clone();
        if plan.mode == ExecutionMode::Dag {
            let mut pending = plan.dependencies[i].clone();
            let mut visited = HashSet::new();
            while let Some(j) = pending.pop() {
                if visited.insert(j) {
                    available.insert(flow.steps[j].name.clone());
                    add_child_outputs(&flow.steps[j], flows, &mut available, true)?;
                    pending.extend(&plan.dependencies[j]);
                }
            }
        } else {
            let group_start = if flow.steps[i].parallel {
                (0..i)
                    .rev()
                    .take_while(|&j| flow.steps[j].parallel)
                    .last()
                    .unwrap_or(i)
            } else {
                i
            };
            for &j in &plan.selected {
                if j < group_start {
                    available.insert(flow.steps[j].name.clone());
                    add_child_outputs(&flow.steps[j], flows, &mut available, false)?;
                }
            }
        }
        let step = &flow.steps[i];
        validate_step_inputs(step, commands, &available)?;
        validate_child(step, commands, flows, &available)?;
    }
    let mut available = inherited.clone();
    available.extend(plan.selected.iter().map(|&i| flow.steps[i].name.clone()));
    for &i in &plan.selected {
        add_child_outputs(
            &flow.steps[i],
            flows,
            &mut available,
            plan.mode == ExecutionMode::Dag,
        )?;
    }
    if let Some(fin) = &flow.finally {
        for step in &fin.steps {
            validate_step_inputs(step, commands, &available)?;
            validate_child(step, commands, flows, &available)?;
            available.insert(step.name.clone());
            add_child_outputs(step, flows, &mut available, false)?;
        }
    }
    Ok(())
}
fn validate_child(
    step: &Step,
    commands: &HashMap<String, crate::commands::LoadedCommand>,
    flows: &HashMap<String, crate::flows::LoadedFlow>,
    available: &HashSet<String>,
) -> Result<(), ZekError> {
    if step.step_type == crate::step::StepType::Flow {
        let name = step
            .flow
            .as_ref()
            .ok_or_else(|| invalid(crate::t!(val_flow_missing_name, step.name)))?;
        let child = flows
            .get(name)
            .ok_or_else(|| invalid(crate::t!(val_flow_not_found, step.name, name)))?;
        let plan = ExecutionPlan::build(&child.flow, &Selection::All)?;
        validate_nested_inputs(&child.flow, &plan, commands, flows, available)?;
    }
    Ok(())
}

fn add_child_outputs(
    step: &Step,
    flows: &HashMap<String, crate::flows::LoadedFlow>,
    available: &mut HashSet<String>,
    scoped: bool,
) -> Result<(), ZekError> {
    if step.step_type != crate::step::StepType::Flow {
        return Ok(());
    }
    let name = step
        .flow
        .as_ref()
        .ok_or_else(|| invalid(crate::t!(val_flow_missing_name, step.name)))?;
    let child = flows
        .get(name)
        .ok_or_else(|| invalid(crate::t!(val_flow_not_found, step.name, name)))?;
    let mut outputs = HashSet::new();
    for s in &child.flow.steps {
        outputs.insert(s.name.clone());
        add_child_outputs(
            s,
            flows,
            &mut outputs,
            child.flow.execution == ExecutionMode::Dag,
        )?;
    }
    if let Some(fin) = &child.flow.finally {
        for s in &fin.steps {
            outputs.insert(s.name.clone());
            add_child_outputs(s, flows, &mut outputs, false)?;
        }
    }
    available.extend(outputs.into_iter().map(|name| {
        if scoped {
            format!("{}::{name}", step.name)
        } else {
            name
        }
    }));
    Ok(())
}
