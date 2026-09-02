use std::path::{Path, PathBuf};

use crate::error::ZekError;

/// Lista los archivos `.yaml`/`.yml` de un directorio, ordenados.
pub fn yaml_files(dir: &Path) -> Result<Vec<PathBuf>, ZekError> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("yaml") | Some("yml")
            )
        })
        .collect();
    files.sort();
    Ok(files)
}

/// Expande referencias a variables de entorno del proceso (`$VAR` o `${VAR}`).
/// `$$` escapa a un `$` literal; las variables no definidas se resuelven a cadena vacía.
pub fn expand_env_vars(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }

        match chars.peek() {
            Some('$') => {
                chars.next();
                out.push('$');
            }
            Some('{') => {
                chars.next();
                let mut name = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    name.push(c);
                }
                if closed {
                    out.push_str(&std::env::var(&name).unwrap_or_default());
                } else {
                    out.push_str("${");
                    out.push_str(&name);
                }
            }
            Some(&c2) if is_var_char(c2) => {
                let mut name = String::new();
                while let Some(&c2) = chars.peek() {
                    if is_var_char(c2) {
                        name.push(c2);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push_str(&std::env::var(&name).unwrap_or_default());
            }
            _ => out.push('$'),
        }
    }

    out
}

fn is_var_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expande_var_de_entorno() {
        std::env::set_var("ZEK_TEST_VAR", "hola");
        assert_eq!(expand_env_vars("$ZEK_TEST_VAR"), "hola");
        assert_eq!(expand_env_vars("${ZEK_TEST_VAR}"), "hola");
        assert_eq!(expand_env_vars("pre-${ZEK_TEST_VAR}-post"), "pre-hola-post");
    }

    #[test]
    fn variable_no_definida_se_resuelve_a_vacio() {
        std::env::remove_var("ZEK_TEST_MISSING");
        assert_eq!(expand_env_vars("${ZEK_TEST_MISSING}"), "");
    }

    #[test]
    fn escapa_dolar_doble() {
        assert_eq!(expand_env_vars("$$HOME"), "$HOME");
    }

    #[test]
    fn texto_sin_variables_no_cambia() {
        assert_eq!(expand_env_vars("sin variables"), "sin variables");
    }
}
