//! Minimal expression evaluator for edge conditions.
//!
//! Deliberately minimal — highest scope-creep risk in the system.
//!
//! **MVP scope** (hard boundary):
//! - Field access: dot notation (`result.confidence`, `finish_reason`)
//! - Comparisons: `==`, `!=`, `>`, `<`, `>=`, `<=`
//! - Logical: `&&`, `||`, `!`
//! - Literals: string (single or double quoted), number, bool, null
//! - Numeric comparison uses f64 coercion — `1` and `1.0` are equal
//!
//! **Explicitly NOT in MVP**: array indexing, null coalescing, string
//! contains, regex, function calls, ternary.

use serde_json::Value;
use thiserror::Error;

/// Errors from expression evaluation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExpressionError {
    #[error("parse error: {message}")]
    Parse { message: String },
    #[error("evaluation error: {message}")]
    #[allow(dead_code)]
    Eval { message: String },
}

/// Evaluate an expression against a data context (the node's output value).
///
/// Returns `true` or `false`. Missing fields in comparisons evaluate to
/// `false` (not an error) so conditional edges can safely coexist.
pub fn evaluate(expression: &str, data: &Value) -> Result<bool, ExpressionError> {
    let tokens = tokenize(expression)?;
    if tokens.is_empty() {
        return Err(ExpressionError::Parse {
            message: "empty expression".into(),
        });
    }
    let (val, rest) = parse_or(&tokens, data)?;
    if !rest.is_empty() {
        return Err(ExpressionError::Parse {
            message: format!("unexpected token: {:?}", rest[0]),
        });
    }
    Ok(val.as_bool())
}

// ---------------------------------------------------------------------------
// Token
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String), // field name or dotted path
    Str(String),   // quoted string literal
    Num(f64),      // numeric literal
    Bool(bool),    // true / false
    Null,          // null
    Eq,            // ==
    Ne,            // !=
    Gt,            // >
    Lt,            // <
    Ge,            // >=
    Le,            // <=
    And,           // &&
    Or,            // ||
    Not,           // !
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

fn tokenize(input: &str) -> Result<Vec<Token>, ExpressionError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            ' ' | '\t' | '\r' | '\n' => i += 1,
            '=' if peek(&chars, i + 1) == Some('=') => {
                tokens.push(Token::Eq);
                i += 2;
            }
            '!' if peek(&chars, i + 1) == Some('=') => {
                tokens.push(Token::Ne);
                i += 2;
            }
            '!' => {
                tokens.push(Token::Not);
                i += 1;
            }
            '>' if peek(&chars, i + 1) == Some('=') => {
                tokens.push(Token::Ge);
                i += 2;
            }
            '>' => {
                tokens.push(Token::Gt);
                i += 1;
            }
            '<' if peek(&chars, i + 1) == Some('=') => {
                tokens.push(Token::Le);
                i += 2;
            }
            '<' => {
                tokens.push(Token::Lt);
                i += 1;
            }
            '&' if peek(&chars, i + 1) == Some('&') => {
                tokens.push(Token::And);
                i += 2;
            }
            '|' if peek(&chars, i + 1) == Some('|') => {
                tokens.push(Token::Or);
                i += 2;
            }
            '"' | '\'' => {
                let quote = chars[i];
                i += 1;
                let start = i;
                while i < chars.len() && chars[i] != quote {
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(ExpressionError::Parse {
                        message: "unterminated string literal".into(),
                    });
                }
                let s: String = chars[start..i].iter().collect();
                tokens.push(Token::Str(s));
                i += 1; // closing quote
            }
            c if c.is_ascii_digit()
                || (c == '-' && peek(&chars, i + 1).is_some_and(|n| n.is_ascii_digit())) =>
            {
                let start = i;
                if c == '-' {
                    i += 1;
                }
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let num_str: String = chars[start..i].iter().collect();
                let num: f64 = num_str.parse().map_err(|_| ExpressionError::Parse {
                    message: format!("invalid number: {num_str}"),
                })?;
                tokens.push(Token::Num(num));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '.')
                {
                    i += 1;
                }
                let ident: String = chars[start..i].iter().collect();
                match ident.as_str() {
                    "true" => tokens.push(Token::Bool(true)),
                    "false" => tokens.push(Token::Bool(false)),
                    "null" => tokens.push(Token::Null),
                    _ => tokens.push(Token::Ident(ident)),
                }
            }
            other => {
                return Err(ExpressionError::Parse {
                    message: format!("unexpected character: {other}"),
                });
            }
        }
    }
    Ok(tokens)
}

fn peek(chars: &[char], idx: usize) -> Option<char> {
    chars.get(idx).copied()
}

// ---------------------------------------------------------------------------
// Evaluated value (internal)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum EvalValue {
    Bool(bool),
    Num(f64),
    Str(String),
    Null,
    Json(Value),
}

impl EvalValue {
    fn as_bool(&self) -> bool {
        match self {
            Self::Bool(b) => *b,
            Self::Null => false,
            Self::Num(n) => *n != 0.0,
            Self::Str(s) => !s.is_empty(),
            Self::Json(Value::Bool(b)) => *b,
            Self::Json(Value::Null) => false,
            Self::Json(_) => true,
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Num(n) => Some(*n),
            Self::Json(Value::Number(n)) => n.as_f64(),
            _ => None,
        }
    }

    fn as_str_value(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            Self::Json(Value::String(s)) => Some(s),
            _ => None,
        }
    }

    fn is_null(&self) -> bool {
        matches!(self, Self::Null | Self::Json(Value::Null))
    }
}

// ---------------------------------------------------------------------------
// Recursive descent parser — precedence: ! > comparison > && > ||
// ---------------------------------------------------------------------------

type ParseResult<'a> = Result<(EvalValue, &'a [Token]), ExpressionError>;

/// or_expr = and_expr ( "||" and_expr )*
fn parse_or<'a>(tokens: &'a [Token], data: &Value) -> ParseResult<'a> {
    let (mut left, mut rest) = parse_and(tokens, data)?;
    while rest.first() == Some(&Token::Or) {
        let (right, r) = parse_and(&rest[1..], data)?;
        left = EvalValue::Bool(left.as_bool() || right.as_bool());
        rest = r;
    }
    Ok((left, rest))
}

/// and_expr = not_expr ( "&&" not_expr )*
fn parse_and<'a>(tokens: &'a [Token], data: &Value) -> ParseResult<'a> {
    let (mut left, mut rest) = parse_not(tokens, data)?;
    while rest.first() == Some(&Token::And) {
        let (right, r) = parse_not(&rest[1..], data)?;
        left = EvalValue::Bool(left.as_bool() && right.as_bool());
        rest = r;
    }
    Ok((left, rest))
}

/// not_expr = "!" not_expr | comparison
fn parse_not<'a>(tokens: &'a [Token], data: &Value) -> ParseResult<'a> {
    if tokens.first() == Some(&Token::Not) {
        let (val, rest) = parse_not(&tokens[1..], data)?;
        return Ok((EvalValue::Bool(!val.as_bool()), rest));
    }
    parse_comparison(tokens, data)
}

/// comparison = primary ( ("==" | "!=" | ">" | "<" | ">=" | "<=") primary )?
fn parse_comparison<'a>(tokens: &'a [Token], data: &Value) -> ParseResult<'a> {
    let (left, rest) = parse_primary(tokens, data)?;
    if rest.is_empty() {
        return Ok((left, rest));
    }
    let op = match &rest[0] {
        Token::Eq => CompOp::Eq,
        Token::Ne => CompOp::Ne,
        Token::Gt => CompOp::Gt,
        Token::Lt => CompOp::Lt,
        Token::Ge => CompOp::Ge,
        Token::Le => CompOp::Le,
        _ => return Ok((left, rest)),
    };
    let (right, rest) = parse_primary(&rest[1..], data)?;
    Ok((EvalValue::Bool(compare(&left, &right, op)), rest))
}

enum CompOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
}

fn compare(left: &EvalValue, right: &EvalValue, op: CompOp) -> bool {
    // Null comparisons
    if left.is_null() || right.is_null() {
        let both_null = left.is_null() && right.is_null();
        return match op {
            CompOp::Eq => both_null,
            CompOp::Ne => !both_null,
            _ => false,
        };
    }

    // Numeric comparison with f64 coercion
    if let (Some(l), Some(r)) = (left.as_f64(), right.as_f64()) {
        return match op {
            CompOp::Eq => (l - r).abs() < f64::EPSILON,
            CompOp::Ne => (l - r).abs() >= f64::EPSILON,
            CompOp::Gt => l > r,
            CompOp::Lt => l < r,
            CompOp::Ge => l >= r || (l - r).abs() < f64::EPSILON,
            CompOp::Le => l <= r || (l - r).abs() < f64::EPSILON,
        };
    }

    // String comparison
    if let (Some(l), Some(r)) = (left.as_str_value(), right.as_str_value()) {
        return match op {
            CompOp::Eq => l == r,
            CompOp::Ne => l != r,
            CompOp::Gt => l > r,
            CompOp::Lt => l < r,
            CompOp::Ge => l >= r,
            CompOp::Le => l <= r,
        };
    }

    // Bool equality
    if let (EvalValue::Bool(l), EvalValue::Bool(r)) = (left, right) {
        return match op {
            CompOp::Eq => l == r,
            CompOp::Ne => l != r,
            _ => false,
        };
    }

    // Type mismatch → false for all comparisons
    false
}

/// primary = Str | Num | Bool | Null | Ident (resolved against data)
fn parse_primary<'a>(tokens: &'a [Token], data: &Value) -> ParseResult<'a> {
    if tokens.is_empty() {
        return Err(ExpressionError::Parse {
            message: "unexpected end of expression".into(),
        });
    }
    match &tokens[0] {
        Token::Str(s) => Ok((EvalValue::Str(s.clone()), &tokens[1..])),
        Token::Num(n) => Ok((EvalValue::Num(*n), &tokens[1..])),
        Token::Bool(b) => Ok((EvalValue::Bool(*b), &tokens[1..])),
        Token::Null => Ok((EvalValue::Null, &tokens[1..])),
        Token::Ident(path) => {
            let val = resolve_path(data, path);
            Ok((val, &tokens[1..]))
        }
        other => Err(ExpressionError::Parse {
            message: format!("expected value, got {other:?}"),
        }),
    }
}

/// Resolve a dotted field path against a JSON value.
fn resolve_path(data: &Value, path: &str) -> EvalValue {
    let mut current = data;
    for segment in path.split('.') {
        match current.get(segment) {
            Some(v) => current = v,
            None => return EvalValue::Null,
        }
    }
    match current {
        Value::Null => EvalValue::Null,
        Value::Bool(b) => EvalValue::Bool(*b),
        Value::Number(n) => EvalValue::Num(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => EvalValue::Str(s.clone()),
        other => EvalValue::Json(other.clone()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_equality() {
        assert!(evaluate(
            r#"finish_reason == "stop""#,
            &json!({"finish_reason": "stop"})
        )
        .unwrap());
    }

    #[test]
    fn test_inequality() {
        assert!(!evaluate(
            r#"finish_reason != "stop""#,
            &json!({"finish_reason": "stop"})
        )
        .unwrap());
    }

    #[test]
    fn test_string_comparison_tool_calls() {
        assert!(evaluate(
            r#"finish_reason == "tool_calls""#,
            &json!({"finish_reason": "tool_calls"}),
        )
        .unwrap());
    }

    #[test]
    fn test_numeric_comparison_gt() {
        assert!(evaluate("score > 0.5", &json!({"score": 0.8})).unwrap());
        assert!(!evaluate("score > 0.5", &json!({"score": 0.3})).unwrap());
    }

    #[test]
    fn test_numeric_comparison_lt() {
        assert!(evaluate("score < 0.5", &json!({"score": 0.3})).unwrap());
        assert!(!evaluate("score < 0.5", &json!({"score": 0.8})).unwrap());
    }

    #[test]
    fn test_numeric_equality_coercion() {
        // 1 (integer in JSON) and 1.0 (float literal) should be equal
        assert!(evaluate("count == 1", &json!({"count": 1.0})).unwrap());
        assert!(evaluate("count == 1.0", &json!({"count": 1})).unwrap());
    }

    #[test]
    fn test_boolean_literal() {
        assert!(evaluate("enabled == true", &json!({"enabled": true})).unwrap());
        assert!(!evaluate("enabled == true", &json!({"enabled": false})).unwrap());
    }

    #[test]
    fn test_null_comparison() {
        assert!(evaluate("value == null", &json!({"value": null})).unwrap());
        assert!(!evaluate("value == null", &json!({"value": "something"})).unwrap());
    }

    #[test]
    fn test_logical_and() {
        assert!(evaluate("a == 1 && b == 2", &json!({"a": 1, "b": 2})).unwrap());
        assert!(!evaluate("a == 1 && b == 99", &json!({"a": 1, "b": 2})).unwrap());
    }

    #[test]
    fn test_logical_or() {
        assert!(evaluate("a == 1 || b == 99", &json!({"a": 1, "b": 2})).unwrap());
        assert!(!evaluate("a == 99 || b == 99", &json!({"a": 1, "b": 2})).unwrap());
    }

    #[test]
    fn test_logical_not() {
        assert!(evaluate("!done", &json!({"done": false})).unwrap());
        assert!(!evaluate("!done", &json!({"done": true})).unwrap());
    }

    #[test]
    fn test_nested_field() {
        assert!(evaluate(
            "result.confidence > 0.9",
            &json!({"result": {"confidence": 0.95}})
        )
        .unwrap());
    }

    #[test]
    fn test_deeply_nested() {
        assert!(evaluate(r#"a.b.c == "deep""#, &json!({"a": {"b": {"c": "deep"}}})).unwrap());
    }

    #[test]
    fn test_missing_field() {
        // Missing fields evaluate comparisons to false (not error)
        assert!(!evaluate(r#"missing == "x""#, &json!({"other": 1})).unwrap());
    }

    #[test]
    fn test_invalid_expression() {
        assert!(evaluate("==", &json!({})).is_err());
    }

    #[test]
    fn test_greater_equal() {
        assert!(evaluate("score >= 0.5", &json!({"score": 0.5})).unwrap());
        assert!(evaluate("score >= 0.5", &json!({"score": 0.8})).unwrap());
        assert!(!evaluate("score >= 0.5", &json!({"score": 0.3})).unwrap());
    }

    #[test]
    fn test_less_equal() {
        assert!(evaluate("score <= 0.5", &json!({"score": 0.5})).unwrap());
        assert!(evaluate("score <= 0.5", &json!({"score": 0.3})).unwrap());
        assert!(!evaluate("score <= 0.5", &json!({"score": 0.8})).unwrap());
    }

    #[test]
    fn test_single_quoted_string() {
        assert!(evaluate("finish_reason == 'stop'", &json!({"finish_reason": "stop"})).unwrap());
    }

    #[test]
    fn test_empty_expression() {
        assert!(evaluate("", &json!({})).is_err());
    }
}
