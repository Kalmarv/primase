//! The text of a MojoShader GLSL program as tokens: the preprocessor lines the dialect uses read, the rest tokenized.
//!
//! MojoShader writes `#ifdef GL_ES` blocks of precision statements (skipped: `GL_ES` is not defined), `#extension`
//! lines (noted), and `#define NAME tokens` aliases for registers, which are expanded here. No other directive is in the
//! dialect, and one is refused by name.

use std::collections::BTreeMap;

use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Token {
    Ident(String),
    Float(f32),
    Int(i64),
    /// A punctuation mark or operator.
    Punct(&'static str),
}

impl Token {
    pub(crate) fn is(&self, punct: &str) -> bool {
        matches!(self, Token::Punct(p) if *p == punct)
    }

    pub(crate) fn text(&self) -> String {
        match self {
            Token::Ident(s) => s.clone(),
            Token::Float(f) => format!("{f:?}"),
            Token::Int(i) => i.to_string(),
            Token::Punct(p) => (*p).to_owned(),
        }
    }
}

/// A token and the line it is on.
#[derive(Clone, Debug)]
pub(crate) struct Spanned {
    pub token: Token,
    pub line: usize,
}

pub(crate) struct Lexed {
    pub tokens: Vec<Spanned>,
    /// The extensions the program enables.
    pub extensions: Vec<String>,
}

const PUNCTS: [&str; 32] = [
    "+=", "-=", "*=", "/=", "==", "!=", "<=", ">=", "&&", "||", "++", "--", "(", ")", "[", "]", "{", "}",
    ";", ",", ".", "+", "-", "*", "/", "<", ">", "=", "?", ":", "!", "%",
];

fn error(line: usize, detail: impl Into<String>) -> Error {
    Error {
        line,
        detail: detail.into(),
    }
}

/// Tokenize `source`, expanding the `#define` aliases.
pub(crate) fn lex(source: &str) -> Result<Lexed> {
    let mut macros: BTreeMap<String, Vec<Spanned>> = BTreeMap::new();
    let mut extensions = Vec::new();
    let mut out: Vec<Spanned> = Vec::new();
    // inside `#ifdef GL_ES`: skipped to its `#endif`
    let mut skipping = false;
    for (n, raw) in source.lines().enumerate() {
        let line = n.saturating_add(1);
        let text = raw.trim();
        if let Some(directive) = text.strip_prefix('#') {
            let directive = directive.trim();
            let (word, rest) = directive
                .split_once(char::is_whitespace)
                .unwrap_or((directive, ""));
            match word {
                "ifdef" if rest.trim() == "GL_ES" && !skipping => skipping = true,
                "endif" if skipping => skipping = false,
                _ if skipping => {}
                "extension" => extensions.push(rest.trim().to_owned()),
                "define" => {
                    let rest = rest.trim();
                    let (name, body) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                    if name.contains('(') {
                        return Err(error(line, "a function-like macro"));
                    }
                    let mut tokens = tokenize(body, line)?;
                    // an alias of an alias
                    tokens = expand(tokens, &macros, line)?;
                    macros.insert(name.to_owned(), tokens);
                }
                other => return Err(error(line, format!("the preprocessor directive #{other}"))),
            }
            continue;
        }
        if skipping {
            continue;
        }
        out.extend(expand(tokenize(text, line)?, &macros, line)?);
    }
    if skipping {
        return Err(error(
            source.lines().count(),
            "an #ifdef GL_ES without its #endif",
        ));
    }
    Ok(Lexed {
        tokens: out,
        extensions,
    })
}

/// Replace the aliases among `tokens` by their bodies.
fn expand(
    tokens: Vec<Spanned>,
    macros: &BTreeMap<String, Vec<Spanned>>,
    line: usize,
) -> Result<Vec<Spanned>> {
    let mut out = Vec::new();
    for t in tokens {
        match &t.token {
            Token::Ident(name) => match macros.get(name) {
                // the bodies are expanded when they are defined, so one pass is the whole expansion
                Some(body) => out.extend(body.iter().map(|b| Spanned {
                    token: b.token.clone(),
                    line,
                })),
                None => out.push(t),
            },
            _ => out.push(t),
        }
    }
    Ok(out)
}

fn tokenize(text: &str, line: usize) -> Result<Vec<Spanned>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(&c) = bytes.get(i) {
        if c.is_ascii_whitespace() {
            i = i.saturating_add(1);
            continue;
        }
        // a comment to the end of the line
        if text.get(i..).is_some_and(|r| r.starts_with("//")) {
            break;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while bytes
                .get(i)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
            {
                i = i.saturating_add(1);
            }
            out.push(Spanned {
                token: Token::Ident(text.get(start..i).unwrap_or("").to_owned()),
                line,
            });
            continue;
        }
        let next_is_digit = bytes.get(i.saturating_add(1)).is_some_and(u8::is_ascii_digit);
        if c.is_ascii_digit() || (c == b'.' && next_is_digit) {
            let start = i;
            let mut float = false;
            while let Some(&d) = bytes.get(i) {
                if d.is_ascii_digit() {
                    i = i.saturating_add(1);
                } else if d == b'.' {
                    float = true;
                    i = i.saturating_add(1);
                } else if d == b'e' || d == b'E' {
                    float = true;
                    i = i.saturating_add(1);
                    if matches!(bytes.get(i), Some(b'+' | b'-')) {
                        i = i.saturating_add(1);
                    }
                } else {
                    break;
                }
            }
            let literal = text.get(start..i).unwrap_or("");
            // a float's suffix
            if matches!(bytes.get(i), Some(b'f' | b'F')) {
                float = true;
                i = i.saturating_add(1);
            }
            let token = if float {
                let value = literal
                    .parse::<f32>()
                    .map_err(|_| error(line, format!("the number {literal}")))?;
                if !value.is_finite() {
                    return Err(error(line, format!("the number {literal}, which is not finite")));
                }
                Token::Float(value)
            } else {
                Token::Int(
                    literal
                        .parse::<i64>()
                        .map_err(|_| error(line, format!("the number {literal}")))?,
                )
            };
            out.push(Spanned { token, line });
            continue;
        }
        let rest = text.get(i..).unwrap_or("");
        match PUNCTS.iter().find(|p| rest.starts_with(**p)) {
            Some(p) => {
                out.push(Spanned {
                    token: Token::Punct(p),
                    line,
                });
                i = i.saturating_add(p.len());
            }
            None => {
                return Err(error(
                    line,
                    format!("the character {:?}", rest.chars().next().unwrap_or('?')),
                ));
            }
        }
    }
    Ok(out)
}
