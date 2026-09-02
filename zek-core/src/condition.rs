//! Evaluación de condiciones `when`.
//!
//! La condición se renderiza primero con Handlebars (rellenando `{{...}}`) y el
//! resultado se evalúa como expresión booleana: soporta comparaciones
//! (`==`, `!=`, `<`, `<=`, `>`, `>=`), lógica (`&&`, `||`, `!`) y paréntesis.
//! Los valores literales pueden ser números, cadenas entre comillas o palabras.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Bool(bool),
    Number(f64),
    Str(String),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Bool(b) => write!(f, "{b}"),
            Value::Number(n) => write!(f, "{n}"),
            Value::Str(s) => write!(f, "{s}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Str(String),
    Number(f64),
    Not,
    And,
    Or,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    LParen,
    RParen,
}

/// Evalúa `expression` y devuelve el valor de verdad.
pub fn evaluate(expression: &str) -> Result<bool, String> {
    let tokens = tokenize(expression)?;
    let mut parser = Parser::new(tokens);
    let value = parser.parse()?;
    Ok(truthy(&value))
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Number(n) => *n != 0.0,
        Value::Str(s) => {
            let lower = s.trim().to_ascii_lowercase();
            !lower.is_empty() && lower != "false" && lower != "no" && lower != "0"
        }
    }
}

fn word_value(word: &str) -> Value {
    match word.to_ascii_lowercase().as_str() {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => match word.parse::<f64>() {
            Ok(n) => Value::Number(n),
            Err(_) => Value::Str(word.to_string()),
        },
    }
}

fn compare(op: Op, left: &Value, right: &Value) -> Value {
    let result = match (left, right) {
        (Value::Number(a), Value::Number(b)) => match op {
            Op::Eq => a == b,
            Op::Ne => a != b,
            Op::Lt => a < b,
            Op::Le => a <= b,
            Op::Gt => a > b,
            Op::Ge => a >= b,
        },
        _ => {
            let a = left.to_string();
            let b = right.to_string();
            match op {
                Op::Eq => a == b,
                Op::Ne => a != b,
                Op::Lt => a < b,
                Op::Le => a <= b,
                Op::Gt => a > b,
                Op::Ge => a >= b,
            }
        }
    };
    Value::Bool(result)
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        match c {
            '(' => {
                chars.next();
                tokens.push(Token::LParen);
            }
            ')' => {
                chars.next();
                tokens.push(Token::RParen);
            }
            '&' => {
                chars.next();
                if chars.next() == Some('&') {
                    tokens.push(Token::And);
                } else {
                    return Err("expected '&&'".to_string());
                }
            }
            '|' => {
                chars.next();
                if chars.next() == Some('|') {
                    tokens.push(Token::Or);
                } else {
                    return Err("expected '||'".to_string());
                }
            }
            '!' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    tokens.push(Token::Ne);
                } else {
                    tokens.push(Token::Not);
                }
            }
            '=' => {
                chars.next();
                if chars.next() == Some('=') {
                    tokens.push(Token::Eq);
                } else {
                    return Err("expected '=='".to_string());
                }
            }
            '<' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    tokens.push(Token::Le);
                } else {
                    tokens.push(Token::Lt);
                }
            }
            '>' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    tokens.push(Token::Ge);
                } else {
                    tokens.push(Token::Gt);
                }
            }
            '"' | '\'' => {
                let quote = c;
                chars.next();
                let mut s = String::new();
                let mut closed = false;
                for ch in chars.by_ref() {
                    if ch == quote {
                        closed = true;
                        break;
                    }
                    s.push(ch);
                }
                if !closed {
                    return Err("unterminated string".to_string());
                }
                tokens.push(Token::Str(s));
            }
            _ if c.is_ascii_digit() => tokens.push(Token::Number(read_number(&mut chars))),
            _ => {
                let mut word = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_whitespace() || is_op_char(ch) {
                        break;
                    }
                    word.push(ch);
                    chars.next();
                }
                tokens.push(Token::Word(word));
            }
        }
    }

    Ok(tokens)
}

fn read_number(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> f64 {
    let mut s = String::new();
    let mut seen_dot = false;
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            s.push(c);
            chars.next();
        } else if c == '.' && !seen_dot {
            seen_dot = true;
            s.push(c);
            chars.next();
        } else {
            break;
        }
    }
    s.parse().unwrap_or(0.0)
}

fn is_op_char(c: char) -> bool {
    matches!(
        c,
        '(' | ')' | '<' | '>' | '=' | '!' | '&' | '|' | '"' | '\''
    )
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    fn parse(&mut self) -> Result<Value, String> {
        if self.tokens.is_empty() {
            return Err("empty condition".to_string());
        }
        let value = self.parse_or()?;
        if self.pos != self.tokens.len() {
            return Err("unexpected token at the end of the condition".to_string());
        }
        Ok(value)
    }

    fn parse_or(&mut self) -> Result<Value, String> {
        let mut left = self.parse_and()?;
        while self.peek() == Some(&Token::Or) {
            self.next();
            let right = self.parse_and()?;
            left = Value::Bool(truthy(&left) || truthy(&right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Value, String> {
        let mut left = self.parse_not()?;
        while self.peek() == Some(&Token::And) {
            self.next();
            let right = self.parse_not()?;
            left = Value::Bool(truthy(&left) && truthy(&right));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Value, String> {
        if self.peek() == Some(&Token::Not) {
            self.next();
            let value = self.parse_not()?;
            return Ok(Value::Bool(!truthy(&value)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Value, String> {
        if self.peek() == Some(&Token::LParen) {
            self.next();
            let value = self.parse_or()?;
            if self.next() != Some(Token::RParen) {
                return Err("expected ')'".to_string());
            }
            return Ok(value);
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Value, String> {
        let left = self.parse_operand()?;
        let op = match self.peek() {
            Some(Token::Eq) => Some(Op::Eq),
            Some(Token::Ne) => Some(Op::Ne),
            Some(Token::Lt) => Some(Op::Lt),
            Some(Token::Le) => Some(Op::Le),
            Some(Token::Gt) => Some(Op::Gt),
            Some(Token::Ge) => Some(Op::Ge),
            _ => None,
        };
        match op {
            Some(op) => {
                self.next();
                let right = self.parse_operand()?;
                Ok(compare(op, &left, &right))
            }
            None => Ok(left),
        }
    }

    fn parse_operand(&mut self) -> Result<Value, String> {
        match self.next() {
            Some(Token::Number(n)) => Ok(Value::Number(n)),
            Some(Token::Str(s)) => Ok(Value::Str(s)),
            Some(Token::Word(w)) => Ok(word_value(&w)),
            _ => Err("expected a value".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(expr: &str) -> bool {
        evaluate(expr).unwrap()
    }

    #[test]
    fn comparacion_numerica() {
        assert!(ok("0 == 0"));
        assert!(!ok("0 != 0"));
        assert!(ok("2 > 1"));
        assert!(!ok("1 >= 2"));
    }

    #[test]
    fn comparacion_de_cadenas() {
        assert!(ok("success == success"));
        assert!(!ok("success == failed"));
        assert!(ok("'hola' == 'hola'"));
    }

    #[test]
    fn logica_y_parentesis() {
        assert!(ok("true && true"));
        assert!(!ok("true && false"));
        assert!(ok("false || true"));
        assert!(ok("!false"));
        assert!(ok("!(1 == 2)"));
        assert!(ok("(1 == 1) && (2 == 2)"));
    }

    #[test]
    fn valores_de_verdad() {
        assert!(ok("true"));
        assert!(!ok("false"));
        assert!(ok("1"));
        assert!(!ok("0"));
        assert!(ok("hola"));
    }

    #[test]
    fn numeros_negativos() {
        assert!(ok("-1 == -1"));
        assert!(ok("-1 != 0"));
    }

    #[test]
    fn errores_de_sintaxis() {
        assert!(evaluate("==").is_err());
        assert!(evaluate("1 &&").is_err());
        assert!(evaluate("(1 == 1").is_err());
    }
}
