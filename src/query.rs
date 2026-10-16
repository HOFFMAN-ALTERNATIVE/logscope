//! A small filter language.
//!
//! ```text
//! expr    := or
//! or      := and (("or" | "||") and)*
//! and     := unary (("and" | "&&") unary)*
//! unary   := ("not" | "!") unary | "(" expr ")" | cmp
//! cmp     := FIELD [OP VALUE]            -- bare FIELD means "exists and not null"
//! OP      := == | = | != | > | >= | < | <= | ~ (regex) | !~ | contains
//! ```
//!
//! Comparisons are numeric when both sides parse as numbers, otherwise they
//! compare strings (so ISO-8601 timestamps order correctly).

use crate::record::{Record, Value};
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Match,
    NotMatch,
    Contains,
}

#[derive(Debug)]
enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Exists(String),
    Cmp {
        field: String,
        op: Op,
        value: String,
        num: Option<f64>,
        re: Option<Regex>,
    },
}

/// A compiled filter expression.
#[derive(Debug)]
pub struct Query(Expr);

impl Query {
    /// Compile a filter expression.
    pub fn parse(src: &str) -> Result<Query> {
        let tokens = tokenize(src)?;
        let mut p = Parser { tokens, pos: 0 };
        let e = p.or()?;
        if let Some(t) = p.tokens.get(p.pos) {
            bail!("unexpected token {t:?} in query {src:?}");
        }
        Ok(Query(e))
    }

    /// Evaluate against a record.
    pub fn matches(&self, r: &Record) -> bool {
        eval(&self.0, r)
    }
}

fn eval(e: &Expr, r: &Record) -> bool {
    match e {
        Expr::And(a, b) => eval(a, r) && eval(b, r),
        Expr::Or(a, b) => eval(a, r) || eval(b, r),
        Expr::Not(a) => !eval(a, r),
        Expr::Exists(f) => matches!(r.get(f), Some(v) if *v != Value::Null),
        Expr::Cmp {
            field,
            op,
            value,
            num,
            re,
        } => {
            let Some(v) = r.get(field) else {
                return matches!(op, Op::Ne | Op::NotMatch);
            };
            let text = v.to_string();
            match op {
                Op::Match => re.as_ref().is_some_and(|re| re.is_match(&text)),
                Op::NotMatch => !re.as_ref().is_some_and(|re| re.is_match(&text)),
                Op::Contains => text.contains(value.as_str()),
                _ => {
                    let ord = match (v.as_f64(), num) {
                        (Some(a), Some(b)) => a.partial_cmp(b),
                        _ => Some(text.as_str().cmp(value.as_str())),
                    };
                    let Some(ord) = ord else { return false };
                    match op {
                        Op::Eq => ord == Ordering::Equal,
                        Op::Ne => ord != Ordering::Equal,
                        Op::Gt => ord == Ordering::Greater,
                        Op::Ge => ord != Ordering::Less,
                        Op::Lt => ord == Ordering::Less,
                        Op::Le => ord != Ordering::Greater,
                        Op::Match | Op::NotMatch | Op::Contains => unreachable!(),
                    }
                }
            }
        }
    }
}

// --------------------------------------------------------------- lexer

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Quoted(String),
    Op(Op),
    LParen,
    RParen,
    And,
    Or,
    Not,
}

fn tokenize(src: &str) -> Result<Vec<Tok>> {
    let mut out = Vec::new();
    let mut it = src.chars().peekable();
    while let Some(&c) = it.peek() {
        match c {
            c if c.is_whitespace() => {
                it.next();
            }
            '(' => {
                it.next();
                out.push(Tok::LParen);
            }
            ')' => {
                it.next();
                out.push(Tok::RParen);
            }
            '"' | '\'' => {
                let quote = c;
                it.next();
                let mut s = String::new();
                loop {
                    match it.next() {
                        // Only the quote and backslash are escapes, so regex
                        // classes like \d survive unchanged.
                        Some('\\') => match it.next() {
                            Some(e) if e == quote || e == '\\' => s.push(e),
                            Some(e) => {
                                s.push('\\');
                                s.push(e);
                            }
                            None => bail!("unterminated escape in query"),
                        },
                        Some(ch) if ch == quote => break,
                        Some(ch) => s.push(ch),
                        None => bail!("unterminated string in query"),
                    }
                }
                out.push(Tok::Quoted(s));
            }
            '=' | '!' | '<' | '>' | '~' | '&' | '|' => {
                it.next();
                let next = it.peek().copied();
                let (tok, consume) = match (c, next) {
                    ('=', Some('=')) => (Tok::Op(Op::Eq), true),
                    ('=', _) => (Tok::Op(Op::Eq), false),
                    ('!', Some('=')) => (Tok::Op(Op::Ne), true),
                    ('!', Some('~')) => (Tok::Op(Op::NotMatch), true),
                    ('!', _) => (Tok::Not, false),
                    ('<', Some('=')) => (Tok::Op(Op::Le), true),
                    ('<', _) => (Tok::Op(Op::Lt), false),
                    ('>', Some('=')) => (Tok::Op(Op::Ge), true),
                    ('>', _) => (Tok::Op(Op::Gt), false),
                    ('~', _) => (Tok::Op(Op::Match), false),
                    ('&', Some('&')) => (Tok::And, true),
                    ('|', Some('|')) => (Tok::Or, true),
                    _ => bail!("unexpected character {c:?} in query"),
                };
                if consume {
                    it.next();
                }
                out.push(tok);
            }
            _ => {
                let mut w = String::new();
                while let Some(&ch) = it.peek() {
                    if ch.is_whitespace() || "()=!<>~&|\"'".contains(ch) {
                        break;
                    }
                    w.push(ch);
                    it.next();
                }
                out.push(match w.as_str() {
                    "and" | "AND" => Tok::And,
                    "or" | "OR" => Tok::Or,
                    "not" | "NOT" => Tok::Not,
                    "contains" => Tok::Op(Op::Contains),
                    _ => Tok::Word(w),
                });
            }
        }
    }
    Ok(out)
}

// -------------------------------------------------------------- parser

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn or(&mut self) -> Result<Expr> {
        let mut left = self.and()?;
        while self.peek() == Some(&Tok::Or) {
            self.pos += 1;
            left = Expr::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr> {
        let mut left = self.unary()?;
        while self.peek() == Some(&Tok::And) {
            self.pos += 1;
            left = Expr::And(Box::new(left), Box::new(self.unary()?));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr> {
        match self.next() {
            Some(Tok::Not) => Ok(Expr::Not(Box::new(self.unary()?))),
            Some(Tok::LParen) => {
                let e = self.or()?;
                match self.next() {
                    Some(Tok::RParen) => Ok(e),
                    _ => bail!("missing closing parenthesis"),
                }
            }
            Some(Tok::Word(field)) | Some(Tok::Quoted(field)) => self.cmp(field),
            Some(t) => bail!("expected a field name, found {t:?}"),
            None => bail!("unexpected end of query"),
        }
    }

    fn cmp(&mut self, field: String) -> Result<Expr> {
        let Some(Tok::Op(op)) = self.peek().cloned() else {
            return Ok(Expr::Exists(field));
        };
        self.pos += 1;
        let value = match self.next() {
            Some(Tok::Word(v)) | Some(Tok::Quoted(v)) => v,
            other => return Err(anyhow!("expected a value after operator, found {other:?}")),
        };
        let re = match op {
            Op::Match | Op::NotMatch => {
                Some(Regex::new(&value).map_err(|e| anyhow!("invalid regex {value:?}: {e}"))?)
            }
            _ => None,
        };
        let num = value.parse::<f64>().ok();
        Ok(Expr::Cmp {
            field,
            op,
            value,
            num,
            re,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_logfmt;

    fn m(q: &str, line: &str) -> bool {
        Query::parse(q).unwrap().matches(&parse_logfmt(line))
    }

    #[test]
    fn comparisons_numeric_and_string() {
        let l = "status=503 path=/api/users level=error ts=2026-10-16T10:00:00Z";
        assert!(m("status >= 500", l));
        assert!(m("status == 503", l));
        assert!(m("status = 503.0", l)); // numeric equality
        assert!(!m("status < 500", l));
        assert!(m("level != info", l));
        assert!(m("ts > 2026-10-16T09:59:59Z", l)); // lexicographic timestamps
        assert!(m("path contains users", l));
    }

    #[test]
    fn boolean_logic_and_precedence() {
        let l = "status=200 method=GET";
        assert!(m("status=500 or method=GET", l));
        assert!(!m("status=500 or method=GET and status=404", l)); // and binds tighter
        assert!(m("(status=500 or method=GET) and not status=404", l));
        assert!(m("!(status = 500) && method == GET", l));
        assert!(m("status=500 || method=GET", l));
    }

    #[test]
    fn regex_exists_and_missing_fields() {
        let l = r#"msg="user 42 logged in" user=42 empty="#;
        assert!(m(r#"msg ~ "user \d+ logged""#, l));
        assert!(m("msg !~ ^timeout", l));
        assert!(m("user", l));
        assert!(!m("empty", l)); // null counts as absent
        assert!(!m("missing", l));
        assert!(!m("missing = 1", l));
        assert!(m("missing != 1", l));
    }

    #[test]
    fn quoted_values_and_fields() {
        let l = r#"msg="hello world" http.status=201"#;
        assert!(m(r#"msg == "hello world""#, l));
        assert!(m("msg == 'hello world'", l));
        assert!(m("http.status >= 200 and http.status < 300", l));
    }

    #[test]
    fn parse_errors() {
        for q in [
            "",
            "status >=",
            "(status = 1",
            "a = 1 )",
            "msg ~ \"[\"",
            "a = \"x",
            "a & b",
        ] {
            assert!(Query::parse(q).is_err(), "query {q:?} should fail");
        }
    }
}
