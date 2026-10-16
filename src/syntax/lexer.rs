//! Tokenizer for `.wx` sources.
//!
//! Identifiers are permissive so that real pin and net names need no quoting: `+3V3`, `-12V`,
//! `~RESET`, `D+`, `D-`, `VBUS_IN`. A `-` continues an identifier unless another `-` follows
//! (so `a--b` is still `a -- b`). Tokens starting with a digit are *words* (`10uF`, `4k7`,
//! `0805`, pin number `1`).

use crate::diag::{Diagnostic, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Word(String),
    Str(String),
    LBrace,
    RBrace,
    LParen,
    RParen,
    Comma,
    Colon,
    ColonColon,
    Semi,
    Dot,
    Eq,
    At,
    Link, // --
    Star,
    Eof,
}

impl Tok {
    pub fn describe(&self) -> String {
        match self {
            Tok::Ident(s) => format!("name `{s}`"),
            Tok::Word(s) => format!("`{s}`"),
            Tok::Str(s) => format!("string \"{s}\""),
            Tok::LBrace => "`{`".into(),
            Tok::RBrace => "`}`".into(),
            Tok::LParen => "`(`".into(),
            Tok::RParen => "`)`".into(),
            Tok::Comma => "`,`".into(),
            Tok::Colon => "`:`".into(),
            Tok::ColonColon => "`::`".into(),
            Tok::Semi => "`;`".into(),
            Tok::Dot => "`.`".into(),
            Tok::Eq => "`=`".into(),
            Tok::At => "`@`".into(),
            Tok::Link => "`--`".into(),
            Tok::Star => "`*`".into(),
            Tok::Eof => "end of file".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

fn ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '~'
}

fn ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '+' || c == '~' || c == '#'
}

pub fn lex(file: usize, src: &str) -> Result<Vec<Token>, Diagnostic> {
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    let at = |i: usize| chars.get(i).map(|c| c.1);
    let off = |i: usize| chars.get(i).map(|c| c.0).unwrap_or(src.len());

    while i < chars.len() {
        let (start, c) = chars[i];
        // Whitespace and comments.
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && at(i + 1) == Some('/') {
            while i < chars.len() && chars[i].1 != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && at(i + 1) == Some('*') {
            i += 2;
            while i < chars.len() && !(chars[i].1 == '*' && at(i + 1) == Some('/')) {
                i += 1;
            }
            if i >= chars.len() {
                return Err(Diagnostic::error("unterminated_comment", "block comment is never closed")
                    .at(Span::new(file, start, start + 2)));
            }
            i += 2;
            continue;
        }

        let single = |t: Tok| Token { tok: t, span: Span::new(file, start, start + c.len_utf8()) };
        match c {
            '{' => { toks.push(single(Tok::LBrace)); i += 1; continue; }
            '}' => { toks.push(single(Tok::RBrace)); i += 1; continue; }
            '(' => { toks.push(single(Tok::LParen)); i += 1; continue; }
            ')' => { toks.push(single(Tok::RParen)); i += 1; continue; }
            ',' => { toks.push(single(Tok::Comma)); i += 1; continue; }
            ';' => { toks.push(single(Tok::Semi)); i += 1; continue; }
            '.' => { toks.push(single(Tok::Dot)); i += 1; continue; }
            '=' => { toks.push(single(Tok::Eq)); i += 1; continue; }
            '@' => { toks.push(single(Tok::At)); i += 1; continue; }
            '*' => { toks.push(single(Tok::Star)); i += 1; continue; }
            ':' => {
                if at(i + 1) == Some(':') {
                    toks.push(Token { tok: Tok::ColonColon, span: Span::new(file, start, start + 2) });
                    i += 2;
                } else {
                    toks.push(single(Tok::Colon));
                    i += 1;
                }
                continue;
            }
            '"' => {
                let mut s = String::new();
                i += 1;
                loop {
                    match at(i) {
                        None | Some('\n') => {
                            return Err(Diagnostic::error("unterminated_string", "string is never closed")
                                .at(Span::new(file, start, off(i))));
                        }
                        Some('"') => break,
                        Some('\\') => {
                            match at(i + 1) {
                                Some('n') => s.push('\n'),
                                Some(o) => s.push(o),
                                None => {}
                            }
                            i += 2;
                        }
                        Some(o) => {
                            s.push(o);
                            i += 1;
                        }
                    }
                }
                i += 1;
                toks.push(Token { tok: Tok::Str(s), span: Span::new(file, start, off(i)) });
                continue;
            }
            '-' if at(i + 1) == Some('-') => {
                toks.push(Token { tok: Tok::Link, span: Span::new(file, start, start + 2) });
                i += 2;
                continue;
            }
            _ => {}
        }

        // Rail names like +3V3 / -12V, identifiers and words.
        let is_rail = (c == '+' || c == '-') && at(i + 1).is_some_and(|n| n.is_ascii_alphanumeric());
        if is_rail || ident_start(c) || c.is_ascii_digit() {
            let word = c.is_ascii_digit();
            let mut j = i + 1;
            while let Some(n) = at(j) {
                let cont = if word {
                    n.is_ascii_alphanumeric() || n == '_' || (n == '.' && at(j + 1).is_some_and(|d| d.is_ascii_digit()))
                } else {
                    ident_continue(n) || (n == '-' && at(j + 1) != Some('-'))
                };
                if !cont {
                    break;
                }
                j += 1;
            }
            let text = src[start..off(j)].to_string();
            let tok = if word { Tok::Word(text) } else { Tok::Ident(text) };
            toks.push(Token { tok, span: Span::new(file, start, off(j)) });
            i = j;
            continue;
        }

        return Err(Diagnostic::error("unexpected_char", format!("unexpected character `{c}`"))
            .at(Span::new(file, start, start + c.len_utf8())));
    }
    toks.push(Token { tok: Tok::Eof, span: Span::new(file, src.len(), src.len()) });
    Ok(toks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<Tok> {
        lex(0, src).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn rails_pins_and_links() {
        let t = kinds("+3V3 -- u.VIN, c.1 // hi\n u.D- -- J1.D+");
        assert_eq!(
            t,
            vec![
                Tok::Ident("+3V3".into()),
                Tok::Link,
                Tok::Ident("u".into()),
                Tok::Dot,
                Tok::Ident("VIN".into()),
                Tok::Comma,
                Tok::Ident("c".into()),
                Tok::Dot,
                Tok::Word("1".into()),
                Tok::Ident("u".into()),
                Tok::Dot,
                Tok::Ident("D-".into()),
                Tok::Link,
                Tok::Ident("J1".into()),
                Tok::Dot,
                Tok::Ident("D+".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn values() {
        assert_eq!(kinds("C(2.2uF, 0805)")[2], Tok::Word("2.2uF".into()));
        assert_eq!(kinds("a--b"), vec![Tok::Ident("a".into()), Tok::Link, Tok::Ident("b".into()), Tok::Eof]);
    }
}
