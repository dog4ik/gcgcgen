//! Recursive-descent parser producing the [`Node`] AST.
//!
//! Grammar (see [`super`] for the prose version):
//!
//! ```text
//! expr     := coalesce
//! coalesce := pipeline ('??' pipeline)*
//! pipeline := primary ('|' call)*
//! call     := IDENT ('(' expr (',' expr)* ')')?
//! primary  := literal | path | array | object | '(' expr ')'
//! path     := IDENT ('.' IDENT | '[' NUMBER ']')*
//! array    := '[' (expr (',' expr)*)? ']'
//! object   := '{' ((STRING|IDENT) ':' expr (',' ...)*)? '}'
//! literal  := STRING | NUMBER | 'true' | 'false' | 'null'
//! ```

use serde_json::Value;

use super::lex::{lex, Tok, Token};
use super::{ExprError, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Seg {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub name: String,
    pub args: Vec<Node>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Literal(Value),
    Path {
        root: String,
        segs: Vec<Seg>,
        span: Span,
    },
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
    /// Two or more alternatives; the first present one wins.
    Coalesce(Vec<Node>),
    Pipe {
        input: Box<Node>,
        calls: Vec<Call>,
    },
}

impl Node {
    /// Collects every scope root the expression reads, for validation.
    pub fn roots(&self, out: &mut Vec<String>) {
        match self {
            Node::Literal(_) => {}
            Node::Path { root, .. } => {
                if !out.iter().any(|r| r == root) {
                    out.push(root.clone());
                }
            }
            Node::Array(items) | Node::Coalesce(items) => items.iter().for_each(|n| n.roots(out)),
            Node::Object(fields) => fields.iter().for_each(|(_, n)| n.roots(out)),
            Node::Pipe { input, calls } => {
                input.roots(out);
                calls
                    .iter()
                    .flat_map(|c| &c.args)
                    .for_each(|n| n.roots(out));
            }
        }
    }

    /// Every function name the expression invokes, for validation.
    pub fn calls(&self, out: &mut Vec<(String, usize, Span)>) {
        match self {
            Node::Literal(_) | Node::Path { .. } => {}
            Node::Array(items) | Node::Coalesce(items) => items.iter().for_each(|n| n.calls(out)),
            Node::Object(fields) => fields.iter().for_each(|(_, n)| n.calls(out)),
            Node::Pipe { input, calls } => {
                input.calls(out);
                for c in calls {
                    out.push((c.name.clone(), c.args.len(), c.span));
                    c.args.iter().for_each(|n| n.calls(out));
                }
            }
        }
    }
}

pub fn parse(src: &str) -> Result<Node, ExprError> {
    let tokens = lex(src)?;
    let mut p = Parser {
        tokens,
        pos: 0,
        src_len: src.len(),
    };
    let node = p.expr()?;
    p.expect_eof()?;
    Ok(node)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    src_len: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn bump(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, want: &Tok) -> bool {
        if self.peek() == want {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, want: &Tok) -> Result<Token, ExprError> {
        if self.peek() == want {
            Ok(self.bump())
        } else {
            Err(ExprError::new(
                format!(
                    "expected {}, found {}",
                    want.describe(),
                    self.peek().describe()
                ),
                self.span(),
            ))
        }
    }

    fn expect_eof(&self) -> Result<(), ExprError> {
        if matches!(self.peek(), Tok::Eof) {
            Ok(())
        } else {
            Err(ExprError::new(
                format!("unexpected trailing {}", self.peek().describe()),
                self.span(),
            ))
        }
    }

    fn expr(&mut self) -> Result<Node, ExprError> {
        self.coalesce()
    }

    fn coalesce(&mut self) -> Result<Node, ExprError> {
        let first = self.pipeline()?;
        if !matches!(self.peek(), Tok::Coalesce) {
            return Ok(first);
        }
        let mut alts = vec![first];
        while self.eat(&Tok::Coalesce) {
            alts.push(self.pipeline()?);
        }
        Ok(Node::Coalesce(alts))
    }

    fn pipeline(&mut self) -> Result<Node, ExprError> {
        let input = self.primary()?;
        if !matches!(self.peek(), Tok::Pipe) {
            return Ok(input);
        }
        let mut calls = Vec::new();
        while self.eat(&Tok::Pipe) {
            calls.push(self.call()?);
        }
        Ok(Node::Pipe {
            input: Box::new(input),
            calls,
        })
    }

    fn call(&mut self) -> Result<Call, ExprError> {
        let start = self.span();
        let name = match self.bump().tok {
            Tok::Ident(n) => n,
            other => {
                return Err(ExprError::new(
                    format!(
                        "expected a function name after `|`, found {}",
                        other.describe()
                    ),
                    start,
                ))
            }
        };

        let mut args = Vec::new();
        let mut end = start;
        if self.eat(&Tok::LParen) {
            if !matches!(self.peek(), Tok::RParen) {
                loop {
                    args.push(self.expr()?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
            }
            end = self.expect(&Tok::RParen)?.span;
        }
        Ok(Call {
            name,
            args,
            span: Span::new(start.start, end.end),
        })
    }

    fn primary(&mut self) -> Result<Node, ExprError> {
        let span = self.span();
        match self.peek().clone() {
            Tok::Str(s) => {
                self.bump();
                Ok(Node::Literal(Value::String(s)))
            }
            Tok::Num(n) => {
                self.bump();
                Ok(Node::Literal(Value::Number(n)))
            }
            Tok::Ident(id) => match id.as_str() {
                "true" => {
                    self.bump();
                    Ok(Node::Literal(Value::Bool(true)))
                }
                "false" => {
                    self.bump();
                    Ok(Node::Literal(Value::Bool(false)))
                }
                "null" => {
                    self.bump();
                    Ok(Node::Literal(Value::Null))
                }
                _ => self.path(),
            },
            Tok::LBracket => self.array(),
            Tok::LBrace => self.object(),
            Tok::LParen => {
                self.bump();
                let inner = self.expr()?;
                self.expect(&Tok::RParen)?;
                Ok(inner)
            }
            other => Err(ExprError::new(
                format!("expected a value, found {}", other.describe()),
                span,
            )),
        }
    }

    fn path(&mut self) -> Result<Node, ExprError> {
        let start = self.span();
        let root = match self.bump().tok {
            Tok::Ident(n) => n,
            other => {
                return Err(ExprError::new(
                    format!("expected a path, found {}", other.describe()),
                    start,
                ))
            }
        };

        let mut segs = Vec::new();
        let mut end = start;
        loop {
            if self.eat(&Tok::Dot) {
                let tok = self.bump();
                match tok.tok {
                    Tok::Ident(k) => {
                        segs.push(Seg::Key(k));
                        end = tok.span;
                    }
                    other => {
                        return Err(ExprError::new(
                            format!(
                                "expected a field name after `.`, found {}",
                                other.describe()
                            ),
                            tok.span,
                        ))
                    }
                }
            } else if matches!(self.peek(), Tok::LBracket) {
                self.bump();
                let tok = self.bump();
                match tok.tok {
                    Tok::Num(n) => match n.as_u64() {
                        Some(i) => segs.push(Seg::Index(i as usize)),
                        None => {
                            return Err(ExprError::new(
                                "array index must be a non-negative integer",
                                tok.span,
                            ))
                        }
                    },
                    other => {
                        return Err(ExprError::new(
                            format!("expected an array index, found {}", other.describe()),
                            tok.span,
                        ))
                    }
                }
                end = self.expect(&Tok::RBracket)?.span;
            } else {
                break;
            }
        }

        Ok(Node::Path {
            root,
            segs,
            span: Span::new(start.start, end.end),
        })
    }

    fn array(&mut self) -> Result<Node, ExprError> {
        self.expect(&Tok::LBracket)?;
        let mut items = Vec::new();
        if !matches!(self.peek(), Tok::RBracket) {
            loop {
                items.push(self.expr()?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(&Tok::RBracket)?;
        Ok(Node::Array(items))
    }

    fn object(&mut self) -> Result<Node, ExprError> {
        self.expect(&Tok::LBrace)?;
        let mut fields: Vec<(String, Node)> = Vec::new();
        if !matches!(self.peek(), Tok::RBrace) {
            loop {
                let tok = self.bump();
                let key = match tok.tok {
                    Tok::Str(s) => s,
                    Tok::Ident(s) => s,
                    other => {
                        return Err(ExprError::new(
                            format!("expected an object key, found {}", other.describe()),
                            tok.span,
                        ))
                    }
                };
                if fields.iter().any(|(k, _)| *k == key) {
                    return Err(ExprError::new(format!("duplicate key `{key}`"), tok.span));
                }
                self.expect(&Tok::Colon)?;
                fields.push((key, self.expr()?));
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(&Tok::RBrace)?;
        Ok(Node::Object(fields))
    }
}

impl Parser {
    #[allow(dead_code)]
    fn eof_span(&self) -> Span {
        Span::new(self.src_len, self.src_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(src: &str) -> Node {
        parse(src).unwrap_or_else(|e| panic!("{src}: {e}"))
    }

    #[test]
    fn parses_a_plain_path() {
        assert_eq!(
            p("payment.token"),
            Node::Path {
                root: "payment".into(),
                segs: vec![Seg::Key("token".into())],
                span: Span::new(0, 13),
            }
        );
    }

    #[test]
    fn parses_indexes() {
        let Node::Path { segs, .. } = p("params.items[2].sku") else {
            panic!("expected a path")
        };
        assert_eq!(
            segs,
            vec![
                Seg::Key("items".into()),
                Seg::Index(2),
                Seg::Key("sku".into())
            ]
        );
    }

    #[test]
    fn parses_pipeline_with_args() {
        let Node::Pipe { input, calls } = p("params.x | null_if('_blank_') | trim") else {
            panic!("expected a pipe")
        };
        assert!(matches!(*input, Node::Path { .. }));
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "null_if");
        assert_eq!(
            calls[0].args,
            vec![Node::Literal(Value::String("_blank_".into()))]
        );
        assert_eq!(calls[1].name, "trim");
        assert!(calls[1].args.is_empty());
    }

    #[test]
    fn coalesce_binds_looser_than_pipe() {
        let Node::Coalesce(alts) = p("a.b | trim ?? c.d ?? 'lit'") else {
            panic!("expected a coalesce")
        };
        assert_eq!(alts.len(), 3);
        assert!(matches!(alts[0], Node::Pipe { .. }));
        assert!(matches!(alts[1], Node::Path { .. }));
        assert_eq!(alts[2], Node::Literal(Value::String("lit".into())));
    }

    #[test]
    fn parses_array_and_object_literals() {
        assert_eq!(
            p("[a.b, 'x']"),
            Node::Array(vec![
                Node::Path {
                    root: "a".into(),
                    segs: vec![Seg::Key("b".into())],
                    span: Span::new(1, 4)
                },
                Node::Literal(Value::String("x".into())),
            ])
        );
        let Node::Object(fields) = p("{ Success: 'approved', 'Failed': 'declined' }") else {
            panic!("expected an object")
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].0, "Success");
        assert_eq!(fields[1].0, "Failed");
    }

    #[test]
    fn parens_group() {
        assert_eq!(p("(a.b ?? c.d) | trim").calls_len(), 1);
    }

    #[test]
    fn collects_roots_and_calls() {
        let n = p("payment.a ?? settings.b | join(' ') ?? steps.auth.c");
        let mut roots = Vec::new();
        n.roots(&mut roots);
        assert_eq!(roots, vec!["payment", "settings", "steps"]);
        let mut calls = Vec::new();
        n.calls(&mut calls);
        assert_eq!(
            calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
            vec!["join"]
        );
    }

    #[test]
    fn reports_useful_errors() {
        let e = parse("payment.").unwrap_err();
        assert!(e.message.contains("field name after `.`"), "{}", e.message);

        let e = parse("payment.token trailing").unwrap_err();
        assert!(e.message.contains("trailing"), "{}", e.message);

        let e = parse("a | ").unwrap_err();
        assert!(
            e.message.contains("function name after `|`"),
            "{}",
            e.message
        );

        let e = parse("{a: 1, a: 2}").unwrap_err();
        assert!(e.message.contains("duplicate key"), "{}", e.message);
    }

    impl Node {
        fn calls_len(&self) -> usize {
            let mut v = Vec::new();
            self.calls(&mut v);
            v.len()
        }
    }
}
