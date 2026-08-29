use serde_json::Value;

/// Extrae el bloque JSON de un output de claude, ya sea un fenced block
/// ```json ... ``` o un objeto `{ ... }` suelto.
pub fn extract_json_block(text: &str) -> Option<String> {
    const FENCE: &str = "```json";
    if let Some(start) = text.find(FENCE) {
        let after = &text[start + FENCE.len()..];
        if let Some(end) = after.find("```") {
            return Some(after[..end].trim().to_string());
        }
    }

    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            if end >= start {
                return Some(text[start..=end].to_string());
            }
        }
    }

    None
}

/// Parsea el output de claude como JSON, extrayendo el bloque JSON si es
/// necesario.
pub fn parse_claude_json(output: &str) -> serde_json::Result<Value> {
    let block = match extract_json_block(output) {
        Some(b) => b,
        None => output.to_string(),
    };
    serde_json::from_str(&block)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extrae_fenced_block_json() {
        let text = "Aquí está el resultado:\n```json\n{\"a\": 1}\n```\n\nChau";
        assert_eq!(extract_json_block(text).as_deref(), Some("{\"a\": 1}"));
    }

    #[test]
    fn extrae_json_suelto() {
        let text = "respuesta: {\"x\": true} y más texto";
        assert_eq!(extract_json_block(text).as_deref(), Some("{\"x\": true}"));
    }

    #[test]
    fn sin_json_devuelve_none() {
        assert_eq!(extract_json_block("sin json acá"), None);
    }

    #[test]
    fn parsea_json_valido() {
        let value = parse_claude_json("```json\n{\"session_id\": \"abc\"}\n```").unwrap();
        assert_eq!(value["session_id"], "abc");
    }

    #[test]
    fn parsea_json_crudo() {
        let value = parse_claude_json("{\"result\": \"hola\"}").unwrap();
        assert_eq!(value["result"], "hola");
    }
}
