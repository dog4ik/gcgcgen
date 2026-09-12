//! Tokenizer for the mapping expression language.
//!
//! The language is deliberately tiny — see [`super`] for the grammar. Every
//! token carries a byte span so the editor can underline the offending slice.

use super::{ExprError, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Str(String),
    Num(serde_json::Number),
    Dot,
    LBracket,
    RBracket,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Colon,
    Comma,
    Pipe,
    /// `??`
    Coalesce,
    Eof,
}

impl Tok {
    /// Human-readable name used in "expected X, found Y" messages.
    pub fn describe(&self) -> String {
        match self {
            Tok::Ident(i) => format!("`{i}`"),
            Tok::Str(_) => "a string".into(),
            Tok::Num(_) => "a number".into(),
            Tok::Dot => "`.`".into(),
            Tok::LBracket => "`[`".into(),
            Tok::RBracket => "`]`".into(),
            Tok::LParen => "`(`".into(),
            Tok::RParen => "`)`".into(),
            Tok::LBrace => "`{`".into(),
            Tok::RBrace => "`}`".into(),
            Tok::Colon => "`:`".into(),
            Tok::Comma => "`,`".into(),
            Tok::Pipe => "`|`".into(),
            Tok::Coalesce => "`??`".into(),
            Tok::Eof => "end of expression".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

pub fn lex(src: &str) -> Result<Vec<Token>, ExprError> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        let start = i;
        let tok = match c {
            b'.' => {
                i += 1;
                Tok::Dot
            }
            b'[' => {
                i += 1;
                Tok::LBracket
            }
            b']' => {
                i += 1;
                Tok::RBracket
            }
            b'(' => {
                i += 1;
                Tok::LParen
            }
            b')' => {
                i += 1;
                Tok::RParen
            }
            b'{' => {
                i += 1;
                Tok::LBrace
            }
            b'}' => {
                i += 1;
                Tok::RBrace
            }
            b':' => {
                i += 1;
                Tok::Colon
            }
            b',' => {
                i += 1;
                Tok::Comma
            }
            b'|' => {
                i += 1;
                Tok::Pipe
            }
            b'?' => {
                if bytes.get(i + 1) == Some(&b'?') {
                    i += 2;
                    Tok::Coalesce
                } else {
                    return Err(ExprError::new(
                        "lone `?` — did you mean `??`",
                        Span::new(start, start + 1),
                    ));
                }
            }
            b'\'' | b'"' => {
                let (s, next) = lex_string(src, i)?;
                i = next;
                Tok::Str(s)
            }
            b'0'..=b'9' => {
                let (n, next) = lex_number(src, i)?;
                i = next;
                Tok::Num(n)
            }
            b'-' if bytes.get(i + 1).is_some_and(u8::is_ascii_digit) => {
                let (n, next) = lex_number(src, i)?;
                i = next;
                Tok::Num(n)
            }
            c if c == b'_' || c.is_ascii_alphabetic() => {
                let s = i;
                while i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                Tok::Ident(src[s..i].to_string())
            }
            _ => {
                // Advance by a whole char so the span lands on a boundary.
                let ch = src[i..].chars().next().expect("index is on a boundary");
                return Err(ExprError::new(
                    format!("unexpected character `{ch}`"),
                    Span::new(start, start + ch.len_utf8()),
                ));
            }
        };
        out.push(Token {
            tok,
            span: Span::new(start, i),
        });
    }

    out.push(Token {
        tok: Tok::Eof,
        span: Span::new(src.len(), src.len()),
    });
    Ok(out)
}

/// Reads a `'`- or `"`-delimited string starting at `start`. Supports the
/// escapes `\\`, `\'`, `\"`, `\n`, `\r`, `\t`.
fn lex_string(src: &str, start: usize) -> Result<(String, usize), ExprError> {
    let bytes = src.as_bytes();
    let quote = bytes[start];
    let mut out = String::new();
    let mut i = start + 1;

    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                let esc = bytes.get(i + 1).ok_or_else(|| {
                    ExprError::new("unterminated escape", Span::new(i, src.len()))
                })?;
                out.push(match esc {
                    b'n' => '\n',
                    b'r' => '\r',
                    b't' => '\t',
                    b'\\' => '\\',
                    b'\'' => '\'',
                    b'"' => '"',
                    other => {
                        return Err(ExprError::new(
                            format!("unknown escape `\\{}`", *other as char),
                            Span::new(i, i + 2),
                        ))
                    }
                });
                i += 2;
            }
            c if c == quote => return Ok((out, i + 1)),
            _ => {
                let ch = src[i..].chars().next().expect("index is on a boundary");
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }

    Err(ExprError::new(
        "unterminated string",
        Span::new(start, src.len()),
    ))
}

fn lex_number(src: &str, start: usize) -> Result<(serde_json::Number, usize), ExprError> {
    let bytes = src.as_bytes();
    let mut i = start;
    if bytes[i] == b'-' {
        i += 1;
    }
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    // A `.` only continues the number when a digit follows, so `1.foo` stays a
    // number followed by a path segment rather than becoming a bad float.
    if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    let text = &src[start..i];
    let n: serde_json::Number = text
        .parse()
        .map_err(|_| ExprError::new(format!("invalid number `{text}`"), Span::new(start, i)))?;
    Ok((n, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        lex(src).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn lexes_a_pipeline() {
        assert_eq!(
            toks("payment.gateway_amount | minor_to_major"),
            vec![
                Tok::Ident("payment".into()),
                Tok::Dot,
                Tok::Ident("gateway_amount".into()),
                Tok::Pipe,
                Tok::Ident("minor_to_major".into()),
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn lexes_strings_and_escapes() {
        assert_eq!(toks("'a b'"), vec![Tok::Str("a b".into()), Tok::Eof]);
        assert_eq!(toks(r"'it\'s'"), vec![Tok::Str("it's".into()), Tok::Eof]);
        assert_eq!(toks(r#""x\ty""#), vec![Tok::Str("x\ty".into()), Tok::Eof]);
    }

    #[test]
    fn lexes_numbers() {
        assert_eq!(toks("42"), vec![Tok::Num(42.into()), Tok::Eof]);
        assert_eq!(toks("-7"), vec![Tok::Num((-7).into()), Tok::Eof]);
        match &toks("1.5")[0] {
            Tok::Num(n) => assert_eq!(n.as_f64(), Some(1.5)),
            other => panic!("expected number, got {other:?}"),
        }
    }

    #[test]
    fn lexes_coalesce_but_rejects_lone_question() {
        assert_eq!(
            toks("a ?? b"),
            vec![
                Tok::Ident("a".into()),
                Tok::Coalesce,
                Tok::Ident("b".into()),
                Tok::Eof
            ]
        );
        let err = lex("a ? b").unwrap_err();
        assert!(err.message.contains("lone `?`"), "{}", err.message);
        assert_eq!(err.span, Span::new(2, 3));
    }

    #[test]
    fn reports_span_of_unterminated_string() {
        let err = lex("'abc").unwrap_err();
        assert_eq!(err.message, "unterminated string");
        assert_eq!(err.span, Span::new(0, 4));
    }

    #[test]
    fn dot_after_integer_is_not_a_float() {
        assert_eq!(
            toks("a[1].b"),
            vec![
                Tok::Ident("a".into()),
                Tok::LBracket,
                Tok::Num(1.into()),
                Tok::RBracket,
                Tok::Dot,
                Tok::Ident("b".into()),
                Tok::Eof,
            ]
        );
    }
}
