use std::fmt::Display;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Idioma de los mensajes de la interfaz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Es,
}

impl Language {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Es => "es",
        }
    }

    /// Parsea un código de idioma (`en`/`es`, sin distinción de mayúsculas).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "en" => Some(Self::En),
            "es" => Some(Self::Es),
            _ => None,
        }
    }
}

static LANGUAGE: Mutex<Language> = Mutex::new(Language::En);

/// Fija el idioma global de la aplicación.
pub fn set(lang: Language) {
    *LANGUAGE.lock().unwrap() = lang;
}

/// Idioma actual (por defecto `en`).
pub fn current() -> Language {
    *LANGUAGE.lock().unwrap()
}

/// Catálogo de mensajes traducidos (en/es).
pub struct Messages {
    // Errores de `error.rs`.
    pub err_no_home_dir: &'static str,
    pub err_config_not_found: &'static str,
    pub err_io: &'static str,
    pub err_yaml: &'static str,
    pub err_invalid_config: &'static str,
    pub err_template: &'static str,
    pub err_workdir_not_found: &'static str,
    pub err_workdir_not_dir: &'static str,
    pub err_missing_dir: &'static str,

    // Errores de `exec.rs`.
    pub exec_spawn_failed: &'static str,
    pub exec_wait_failed: &'static str,

    // Validación de flujos/commands/steps.
    pub val_step_duplicate: &'static str,
    pub val_finally_overlap: &'static str,
    pub val_finally_duplicate: &'static str,
    pub val_goto_self: &'static str,
    pub val_goto_missing: &'static str,
    pub val_goto_missing_short: &'static str,
    pub val_dead_step: &'static str,
    pub val_static_cycle: &'static str,
    pub val_flow_duplicate: &'static str,
    pub val_command_duplicate: &'static str,
    pub val_cmd_ref_missing: &'static str,
    pub val_on_error_invalid: &'static str,
    pub val_on_success_invalid: &'static str,
    pub val_goto_empty: &'static str,
    pub val_step_no_command: &'static str,

    // Razones de salida de ejecución.
    pub reason_infinite_loop: &'static str,

    // CLI: ayuda de clap (estáticas por defecto en inglés).
    pub about: &'static str,
    pub help_dry_run: &'static str,
    pub help_verbose: &'static str,
    pub help_debug: &'static str,
    pub help_timeout_global: &'static str,
    pub cmd_init: &'static str,
    pub cmd_init_dir: &'static str,
    pub cmd_config: &'static str,
    pub cmd_list: &'static str,
    pub cmd_commands: &'static str,
    pub cmd_commands_name: &'static str,
    pub cmd_ask: &'static str,
    pub cmd_ask_message: &'static str,
    pub cmd_completion: &'static str,
    pub cmd_completion_shell: &'static str,
    pub cmd_run: &'static str,
    pub cfg_show: &'static str,
    pub cfg_set_dir: &'static str,
    pub cfg_set_dir_path: &'static str,
    pub cfg_set_lang: &'static str,
    pub cfg_set_lang_lang: &'static str,
    pub err_missing_name: &'static str,

    // CLI: comandos.
    pub msg_config_saved: &'static str,
    pub msg_workdir: &'static str,
    pub prompt_workdir: &'static str,
    pub err_workdir_empty: &'static str,
    pub prompt_create_dir: &'static str,
    pub err_init_cancelled: &'static str,
    pub err_not_a_dir: &'static str,
    pub prompt_create_subdir: &'static str,
    pub label_config: &'static str,
    pub label_workdir: &'static str,
    pub status_ok: &'static str,
    pub status_missing: &'static str,
    pub msg_configured: &'static str,
    pub label_commands: &'static str,
    pub label_flows: &'static str,
    pub label_none: &'static str,
    pub warning_at: &'static str,
    pub label_command: &'static str,
    pub label_description: &'static str,
    pub label_run: &'static str,
    pub label_cwd: &'static str,
    pub label_timeout: &'static str,
    pub label_author: &'static str,
    pub label_source: &'static str,
    pub err_command_not_found: &'static str,
    pub err_claude_failed: &'static str,
    pub err_name_not_found: &'static str,
    pub confirm_step: &'static str,
    pub plan_label: &'static str,
    pub plan_finally: &'static str,
    pub plan_no_command: &'static str,
    pub plan_no_prompt: &'static str,
    pub sum_flow: &'static str,
    pub sum_status: &'static str,
    pub sum_exit_code: &'static str,
    pub sum_duration: &'static str,
    pub sum_steps: &'static str,
    pub sum_attempts: &'static str,
    pub sum_failed_steps: &'static str,
    pub sum_exit_reason: &'static str,
    pub sum_output_of: &'static str,
    pub err_command_failed: &'static str,
    pub err_timeout_exceeded: &'static str,
    pub msg_no_config: &'static str,
    pub err_config_after_init: &'static str,
    pub err_config_load: &'static str,
    pub err_workdir_invalid: &'static str,
    pub msg_workdir_updated: &'static str,
    pub msg_language_updated: &'static str,
    pub err_language_invalid: &'static str,
}

pub const EN: Messages = Messages {
    err_no_home_dir: "could not determine the user's home directory",
    err_config_not_found: "configuration not found: {}",
    err_io: "I/O error: {}",
    err_yaml: "YAML error in {}:{}: {}",
    err_invalid_config: "invalid configuration: {}",
    err_template: "template error: {}",
    err_workdir_not_found: "working directory does not exist: {}",
    err_workdir_not_dir: "working directory is not a directory: {}",
    err_missing_dir: "missing '{}' folder in the working directory: {}",

    exec_spawn_failed: "failed to launch the process: {}",
    exec_wait_failed: "error waiting for the process: {}",

    val_step_duplicate: "duplicate step name: '{}'",
    val_finally_overlap: "the '{}' step in `finally` overlaps with a step in the main flow",
    val_finally_duplicate: "duplicate step name in `finally`: '{}'",
    val_goto_self: "step '{}' cannot goto itself",
    val_goto_missing: "goto to nonexistent step: '{}' (from '{}')",
    val_goto_missing_short: "goto to nonexistent step: '{}'",
    val_dead_step: "dead step '{}': entry_only_via_goto but no step references it with goto",
    val_static_cycle: "static cycle detected: {}",
    val_flow_duplicate: "duplicate flow name: '{}'",
    val_command_duplicate: "duplicate command name: '{}'",
    val_cmd_ref_missing: "step references command '{}' which does not exist (raw command or typo?)",
    val_on_error_invalid: "invalid on_error: '{}' (expected: stop | continue | goto:<step>)",
    val_on_success_invalid: "invalid on_success: '{}' (expected: continue | end | goto:<step>)",
    val_goto_empty: "goto: requires a non-empty step name",
    val_step_no_command: "step '{}' of type command has no command defined",

    reason_infinite_loop: "infinite_loop: exceeded {} jumps",

    about: "Terminal workflow orchestrator with Claude and OpenCode",
    help_dry_run: "Preview the execution plan without running it",
    help_verbose: "Verbose logs",
    help_debug: "Extra debug (paths, timestamps)",
    help_timeout_global: "Global timeout in seconds for the whole flow",
    cmd_init: "Set up zek for the first time (or reconfigure)",
    cmd_init_dir: "Working directory (if omitted, prompts interactively)",
    cmd_config: "Show or modify the configuration",
    cmd_list: "List available commands and flows",
    cmd_commands: "Show a specific command",
    cmd_commands_name: "Command name",
    cmd_ask: "Ask Claude something outside of flows",
    cmd_ask_message: "Message to send to claude -p",
    cmd_completion: "Generate the completion script for a shell",
    cmd_completion_shell: "Target shell",
    cmd_run: "Run a flow or command by name",
    cfg_show: "Show the current configuration",
    cfg_set_dir: "Change the working directory",
    cfg_set_dir_path: "New working directory (must contain commands/ and flows/)",
    cfg_set_lang: "Change the language (en/es)",
    cfg_set_lang_lang: "Language code (en or es)",
    err_missing_name: "missing the flow or command name",

    msg_config_saved: "Configuration saved to {}",
    msg_workdir: "workdir: {}",
    prompt_workdir: "Working directory (will contain commands/ and flows/)",
    err_workdir_empty: "the working directory cannot be empty",
    prompt_create_dir: "The directory {} does not exist. Create it?",
    err_init_cancelled: "init cancelled by the user",
    err_not_a_dir: "{} is not a directory",
    prompt_create_subdir: "Create the {} folder?",
    label_config: "Config : {}",
    label_workdir: "Workdir: {}",
    status_ok: "ok",
    status_missing: "missing",
    msg_configured: "zek is configured. Run `zek --help` to see the commands.",
    label_commands: "Commands ({}):",
    label_flows: "Flows ({}):",
    label_none: "(none)",
    warning_at: "warning: {} (in {})",
    label_command: "Command: {}",
    label_description: "  Description: {}",
    label_run: "  Run: {}",
    label_cwd: "  Cwd: {}",
    label_timeout: "  Timeout: {}s",
    label_author: "  Author: {}",
    label_source: "  Source: {}",
    err_command_not_found: "command not found: {}",
    err_claude_failed: "claude terminated with an error (exit code {})",
    err_name_not_found: "no flow or command named: {}",
    confirm_step: "Run step '{}'?",
    plan_label: "Plan: {}",
    plan_finally: "  finally:",
    plan_no_command: "(no command)",
    plan_no_prompt: "(no prompt)",
    sum_flow: "══ Flow: {} ══",
    sum_status: "Status   : {}",
    sum_exit_code: "Exit code: {}",
    sum_duration: "Duration : {}",
    sum_steps: "Steps:",
    sum_attempts: "{} attempt(s)",
    sum_failed_steps: "Failed steps: {}",
    sum_exit_reason: "Exit reason: {}",
    sum_output_of: "── output of {} ──",
    err_command_failed: "command '{}' failed (exit code {})",
    err_timeout_exceeded: "global timeout exceeded ({}s)",
    msg_no_config: "No configuration found. Starting setup wizard...",
    err_config_after_init: "could not load the configuration after init",
    err_config_load: "error loading the configuration",
    err_workdir_invalid:
        "the new working directory is invalid (it must exist and contain commands/ and flows/)",
    msg_workdir_updated: "Workdir updated: {}",
    msg_language_updated: "Language updated: {}",
    err_language_invalid: "invalid language '{}' (expected: en | es)",
};

pub const ES: Messages = Messages {
    err_no_home_dir: "no se pudo determinar el directorio home del usuario",
    err_config_not_found: "no se encontró la configuración: {}",
    err_io: "error de I/O: {}",
    err_yaml: "error de YAML en {}:{}: {}",
    err_invalid_config: "configuración inválida: {}",
    err_template: "error de template: {}",
    err_workdir_not_found: "el directorio de trabajo no existe: {}",
    err_workdir_not_dir: "el directorio de trabajo no es un directorio: {}",
    err_missing_dir: "falta la carpeta '{}' en el workdir: {}",

    exec_spawn_failed: "no se pudo lanzar el proceso: {}",
    exec_wait_failed: "error esperando el proceso: {}",

    val_step_duplicate: "nombre de step duplicado: '{}'",
    val_finally_overlap: "el step '{}' de finally solapa con un step del flujo principal",
    val_finally_duplicate: "nombre de step duplicado en finally: '{}'",
    val_goto_self: "el step '{}' no puede hacer goto a sí mismo",
    val_goto_missing: "goto a step inexistente: '{}' (desde '{}')",
    val_goto_missing_short: "goto a step inexistente: '{}'",
    val_dead_step: "step muerto '{}': entry_only_via_goto pero ningún step lo referencia con goto",
    val_static_cycle: "ciclo estático detectado: {}",
    val_flow_duplicate: "nombre de flujo duplicado: '{}'",
    val_command_duplicate: "nombre de comando duplicado: '{}'",
    val_cmd_ref_missing:
        "el step referencia el comando '{}' que no existe (¿comando crudo o typo?)",
    val_on_error_invalid: "on_error inválido: '{}' (esperado: stop | continue | goto:<step>)",
    val_on_success_invalid: "on_success inválido: '{}' (esperado: continue | end | goto:<step>)",
    val_goto_empty: "goto: requiere un nombre de step no vacío",
    val_step_no_command: "el step '{}' de tipo command no tiene comando definido",

    reason_infinite_loop: "infinite_loop: se superaron {} saltos",

    about: "Orquestador de flujos de trabajo para la terminal con Claude y OpenCode",
    help_dry_run: "Previsualizar el plan de ejecución sin correrlo",
    help_verbose: "Logs detallados",
    help_debug: "Debug extra (rutas, timestamps)",
    help_timeout_global: "Timeout global en segundos para todo el flujo",
    cmd_init: "Configura zek por primera vez (o re-configura)",
    cmd_init_dir: "Carpeta de trabajo (si se omite, se pregunta interactivamente)",
    cmd_config: "Muestra o modifica la configuración",
    cmd_list: "Lista los comandos y flujos disponibles",
    cmd_commands: "Muestra un comando específico",
    cmd_commands_name: "Nombre del comando",
    cmd_ask: "Pregunta algo a Claude fuera de flujos",
    cmd_ask_message: "Mensaje a enviar a claude -p",
    cmd_completion: "Genera el script de completions para un shell",
    cmd_completion_shell: "Shell objetivo",
    cmd_run: "Ejecuta un flujo o comando por nombre",
    cfg_show: "Muestra la configuración actual",
    cfg_set_dir: "Cambia la carpeta de trabajo",
    cfg_set_dir_path: "Nueva carpeta de trabajo (debe contener commands/ y flows/)",
    cfg_set_lang: "Cambia el idioma (en/es)",
    cfg_set_lang_lang: "Código de idioma (en o es)",
    err_missing_name: "falta el nombre del flujo o comando",

    msg_config_saved: "Configuración guardada en {}",
    msg_workdir: "  workdir: {}",
    prompt_workdir: "Carpeta de trabajo (contendrá commands/ y flows/)",
    err_workdir_empty: "la carpeta de trabajo no puede estar vacía",
    prompt_create_dir: "La carpeta {} no existe. ¿Crearla?",
    err_init_cancelled: "init cancelado por el usuario",
    err_not_a_dir: "{} no es un directorio",
    prompt_create_subdir: "Crear la carpeta {}?",
    label_config: "Config : {}",
    label_workdir: "Workdir: {}",
    status_ok: "ok",
    status_missing: "missing",
    msg_configured: "zek está configurado. Usá `zek --help` para ver los comandos.",
    label_commands: "Comandos ({}):",
    label_flows: "Flujos ({}):",
    label_none: "(ninguno)",
    warning_at: "warning: {} (en {})",
    label_command: "Comando: {}",
    label_description: "  Descripción: {}",
    label_run: "  Run: {}",
    label_cwd: "  Cwd: {}",
    label_timeout: "  Timeout: {}s",
    label_author: "  Autor: {}",
    label_source: "  Fuente: {}",
    err_command_not_found: "comando no encontrado: {}",
    err_claude_failed: "claude terminó con error (exit code {})",
    err_name_not_found: "no existe el flujo ni el comando: {}",
    confirm_step: "¿Ejecutar el step '{}'?",
    plan_label: "Plan: {}",
    plan_finally: "  finally:",
    plan_no_command: "(sin comando)",
    plan_no_prompt: "(sin prompt)",
    sum_flow: "══ Flow: {} ══",
    sum_status: "Status   : {}",
    sum_exit_code: "Exit code: {}",
    sum_duration: "Duration : {}",
    sum_steps: "Steps:",
    sum_attempts: "{} intento(s)",
    sum_failed_steps: "Failed steps: {}",
    sum_exit_reason: "Exit reason: {}",
    sum_output_of: "── output de {} ──",
    err_command_failed: "comando '{}' falló (exit code {})",
    err_timeout_exceeded: "timeout global excedido ({}s)",
    msg_no_config: "No se encontró configuración. Iniciando wizard de setup...",
    err_config_after_init: "no se pudo cargar la config tras el init",
    err_config_load: "error cargando la configuración",
    err_workdir_invalid:
        "el nuevo workdir no es válido (debe existir y contener commands/ y flows/)",
    msg_workdir_updated: "Workdir actualizado: {}",
    msg_language_updated: "Idioma actualizado: {}",
    err_language_invalid: "idioma inválido '{}' (esperado: en | es)",
};

/// Devuelve el catálogo de mensajes para el idioma actual.
pub fn messages() -> &'static Messages {
    match current() {
        Language::En => &EN,
        Language::Es => &ES,
    }
}

/// Formatea un mensaje reemplazando los placeholders `{}` de forma posicional.
pub fn fmt(template: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    for arg in args {
        if let Some(idx) = rest.find("{}") {
            out.push_str(&rest[..idx]);
            out.push_str(&arg.to_string());
            rest = &rest[idx + 2..];
        } else {
            break;
        }
    }
    out.push_str(rest);
    out
}

/// Coerción de ayuda para `t!`: convierte `&T` en `&dyn Display`.
#[doc(hidden)]
pub fn disp<T: Display>(value: &T) -> &dyn Display {
    value
}

/// Atajo para `fmt(messages().<key>, &[<args>...])`.
#[macro_export]
macro_rules! t {
    ($key:ident $(, $arg:expr)* $(,)?) => {
        $crate::lang::fmt(
            $crate::lang::messages().$key,
            &[$($crate::lang::disp(&$arg)),*],
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lenguaje_por_defecto_es_en() {
        assert_eq!(Language::default(), Language::En);
        assert_eq!(current(), Language::En);
    }

    #[test]
    fn parsea_codigos_de_idioma() {
        assert_eq!(Language::parse("en"), Some(Language::En));
        assert_eq!(Language::parse("ES"), Some(Language::Es));
        assert_eq!(Language::parse("fr"), None);
    }

    #[test]
    fn as_str_es_estable() {
        assert_eq!(Language::En.as_str(), "en");
        assert_eq!(Language::Es.as_str(), "es");
    }
}
