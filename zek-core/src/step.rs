use serde::de::{self, Deserializer};
use serde::Deserialize;

pub const DEFAULT_TIMEOUT: u32 = 300;

/// Tipo de step: ejecuta un comando o habla con Claude/OpenCode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepType {
    Command,
    Claude,
    Opencode,
}

/// Qué hacer si un step falla.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnErrorAction {
    Stop,
    Continue,
    Goto(String),
}

/// Qué hacer si un step tiene éxito.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnSuccessAction {
    Continue,
    End,
    Goto(String),
}

impl<'de> Deserialize<'de> for OnErrorAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "stop" => Ok(OnErrorAction::Stop),
            "continue" => Ok(OnErrorAction::Continue),
            _ => match s.strip_prefix("goto:") {
                Some(target) if !target.trim().is_empty() => {
                    Ok(OnErrorAction::Goto(target.trim().to_string()))
                }
                Some(_) => Err(de::Error::custom(
                    "goto: requiere un nombre de step no vacío",
                )),
                None => Err(de::Error::custom(format!(
                    "on_error inválido: '{s}' (esperado: stop | continue | goto:<step>)"
                ))),
            },
        }
    }
}

impl<'de> Deserialize<'de> for OnSuccessAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "continue" => Ok(OnSuccessAction::Continue),
            "end" => Ok(OnSuccessAction::End),
            _ => match s.strip_prefix("goto:") {
                Some(target) if !target.trim().is_empty() => {
                    Ok(OnSuccessAction::Goto(target.trim().to_string()))
                }
                Some(_) => Err(de::Error::custom(
                    "goto: requiere un nombre de step no vacío",
                )),
                None => Err(de::Error::custom(format!(
                    "on_success inválido: '{s}' (esperado: continue | end | goto:<step>)"
                ))),
            },
        }
    }
}

fn default_timeout() -> u32 {
    DEFAULT_TIMEOUT
}

/// Un paso de un flujo. Los campos `on_error`/`on_success` son `Option` porque
/// su default depende del scope: en `steps` el default de `on_error` es `stop`,
/// mientras que en `finally` es `continue`. Resolver con [`Step::effective_on_error`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Step {
    /// Nombre único dentro del flujo (obligatorio).
    pub name: String,

    #[serde(rename = "type")]
    pub step_type: StepType,

    #[serde(default)]
    pub retries: u32,

    #[serde(default)]
    pub retry_delay: u32,

    #[serde(default)]
    pub on_error: Option<OnErrorAction>,

    #[serde(default)]
    pub on_success: Option<OnSuccessAction>,

    #[serde(default)]
    pub entry_only_via_goto: bool,

    /// Solo `type: command`: comando a ejecutar (nombre de un command o comando crudo).
    #[serde(default)]
    pub command: Option<String>,

    /// Solo `type: command`.
    #[serde(default)]
    pub cwd: Option<String>,

    /// Solo `type: command`.
    #[serde(default = "default_timeout")]
    pub timeout: u32,

    /// Solo `type: claude`/`type: opencode`: prompt con placeholders.
    #[serde(default)]
    pub prompt: Option<String>,

    /// Solo `type: claude`/`type: opencode`: formato de salida (`json` para parseo automático).
    #[serde(default)]
    pub output_format: Option<String>,

    /// Solo `type: claude`/`type: opencode`: ID de sesión previa para `--resume`/`--session`.
    #[serde(default)]
    pub session_id: Option<String>,

    /// Solo `type: claude`/`type: opencode`: continuar la sesión del paso anterior.
    #[serde(default)]
    pub continue_session: bool,

    /// Solo `type: opencode`: modelo a usar (formato `provider/model`).
    #[serde(default)]
    pub model: Option<String>,

    /// Solo `type: opencode`: agente a usar.
    #[serde(default)]
    pub agent: Option<String>,

    /// Pedir confirmación antes de ejecutar.
    #[serde(default)]
    pub confirm: bool,
}

impl Step {
    pub fn effective_on_error(&self, in_finally: bool) -> OnErrorAction {
        self.on_error.clone().unwrap_or(if in_finally {
            OnErrorAction::Continue
        } else {
            OnErrorAction::Stop
        })
    }

    pub fn effective_on_success(&self) -> OnSuccessAction {
        self.on_success.clone().unwrap_or(OnSuccessAction::Continue)
    }

    /// Todos los destinos de `goto` declarados (en `on_error` y `on_success`).
    pub fn goto_targets(&self) -> Vec<String> {
        let mut targets = Vec::new();
        if let Some(OnErrorAction::Goto(t)) = &self.on_error {
            targets.push(t.clone());
        }
        if let Some(OnSuccessAction::Goto(t)) = &self.on_success {
            targets.push(t.clone());
        }
        targets
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_step(yaml: &str) -> Step {
        serde_yaml::from_str(yaml).expect("deserializar step")
    }

    #[test]
    fn parsea_step_command_completo() {
        let step = parse_step(
            "name: build\ntype: command\ncommand: build\nretries: 2\nretry_delay: 5\non_error: stop\non_success: continue\n",
        );
        assert_eq!(step.name, "build");
        assert_eq!(step.step_type, StepType::Command);
        assert_eq!(step.retries, 2);
        assert_eq!(step.retry_delay, 5);
        assert_eq!(step.on_error, Some(OnErrorAction::Stop));
        assert_eq!(step.on_success, Some(OnSuccessAction::Continue));
        assert_eq!(step.command.as_deref(), Some("build"));
    }

    #[test]
    fn parsea_step_claude() {
        let step = parse_step("name: resumen\ntype: claude\nprompt: hola\n");
        assert_eq!(step.step_type, StepType::Claude);
        assert_eq!(step.prompt.as_deref(), Some("hola"));
    }

    #[test]
    fn parsea_step_opencode() {
        let step = parse_step(
            "name: review\ntype: opencode\nprompt: revisa esto\nmodel: anthropic/claude-sonnet-4\nagent: build\n",
        );
        assert_eq!(step.step_type, StepType::Opencode);
        assert_eq!(step.prompt.as_deref(), Some("revisa esto"));
        assert_eq!(step.model.as_deref(), Some("anthropic/claude-sonnet-4"));
        assert_eq!(step.agent.as_deref(), Some("build"));
    }

    #[test]
    fn defaults_son_cero_y_sin_acciones() {
        let step = parse_step("name: x\ntype: command\ncommand: x\n");
        assert_eq!(step.retries, 0);
        assert_eq!(step.retry_delay, 0);
        assert_eq!(step.on_error, None);
        assert_eq!(step.on_success, None);
        assert!(!step.entry_only_via_goto);
        assert_eq!(step.timeout, DEFAULT_TIMEOUT);
    }

    #[test]
    fn on_error_goto() {
        let step = parse_step("name: x\ntype: command\ncommand: x\non_error: goto:handler\n");
        assert_eq!(step.on_error, Some(OnErrorAction::Goto("handler".into())));
        assert_eq!(step.goto_targets(), vec!["handler".to_string()]);
    }

    #[test]
    fn on_success_end() {
        let step = parse_step("name: x\ntype: command\ncommand: x\non_success: end\n");
        assert_eq!(step.on_success, Some(OnSuccessAction::End));
    }

    #[test]
    fn on_error_invalido_da_error() {
        let err = serde_yaml::from_str::<Step>(
            "name: x\ntype: command\ncommand: x\non_error: explotar\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("on_error inválido"));
    }

    #[test]
    fn goto_vacio_da_error() {
        let err = serde_yaml::from_str::<Step>(
            "name: x\ntype: command\ncommand: x\non_error: \"goto:\"\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("no vacío"));
    }

    #[test]
    fn effective_on_error_segun_scope() {
        let step = parse_step("name: x\ntype: command\ncommand: x\n");
        assert_eq!(step.effective_on_error(false), OnErrorAction::Stop);
        assert_eq!(step.effective_on_error(true), OnErrorAction::Continue);
        assert_eq!(step.effective_on_success(), OnSuccessAction::Continue);
    }

    #[test]
    fn effective_respeta_valor_explicito() {
        let step = parse_step("name: x\ntype: command\ncommand: x\non_error: continue\n");
        assert_eq!(step.effective_on_error(true), OnErrorAction::Continue);
    }
}
