//! Recursive-descent parser. The grammar needs no statement separators: every statement starts
//! with a keyword or a name followed by `=` or a connection chain. `;` is accepted and ignored.

use super::ast::*;
use super::lexer::{Tok, Token};
use crate::diag::{Diagnostic, Span};

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

type PResult<T> = Result<T, Diagnostic>;

const META_KEYS: &[&str] = &["title", "author", "revision", "company", "date", "comment"];
const PART_FLAGS: &[&str] = &["open_pins_ok"];

impl Parser {
    pub fn new(toks: Vec<Token>) -> Self {
        Parser { toks, pos: 0 }
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }
    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }
    fn span(&self) -> Span {
        self.toks[self.pos].span
    }
    fn prev_span(&self) -> Span {
        self.toks[self.pos.saturating_sub(1)].span
    }
    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == t {
            self.bump();
            true
        } else {
            false
        }
    }
    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == kw)
    }
    fn skip_semis(&mut self) {
        while self.eat(&Tok::Semi) {}
    }

    fn err_expected(&self, what: &str) -> Diagnostic {
        Diagnostic::error("syntax", format!("expected {what}, found {}", self.peek().describe())).at(self.span())
    }

    fn expect(&mut self, t: Tok, what: &str) -> PResult<Span> {
        if self.peek() == &t {
            Ok(self.bump().span)
        } else {
            Err(self.err_expected(what))
        }
    }

    fn ident(&mut self, what: &str) -> PResult<Name> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                let sp = self.bump().span;
                Ok(Name { text: s, span: sp })
            }
            _ => Err(self.err_expected(what)),
        }
    }

    /// An identifier, word or string: anything that can stand for a name or value.
    fn atom(&mut self, what: &str) -> PResult<Name> {
        match self.peek().clone() {
            Tok::Ident(s) | Tok::Word(s) | Tok::Str(s) => {
                let sp = self.bump().span;
                Ok(Name { text: s, span: sp })
            }
            _ => Err(self.err_expected(what)),
        }
    }

    fn string_or_ident(&mut self, what: &str) -> PResult<Name> {
        match self.peek().clone() {
            Tok::Ident(s) | Tok::Str(s) => {
                let sp = self.bump().span;
                Ok(Name { text: s, span: sp })
            }
            _ => Err(self.err_expected(what)),
        }
    }

    pub fn parse_file(&mut self) -> PResult<File> {
        let mut items = Vec::new();
        loop {
            self.skip_semis();
            if self.peek() == &Tok::Eof {
                break;
            }
            items.extend(self.item()?);
        }
        Ok(File { items })
    }

    fn item(&mut self) -> PResult<Vec<Item>> {
        let Tok::Ident(kw) = self.peek().clone() else {
            return Err(self.err_expected("`part`, `module`, `sheet`, `use`, `rail` or a statement"));
        };
        // A keyword only counts when it is not used as a name in a statement (`part = …`, `part -- …`).
        let used_as_name = matches!(self.peek_at(1), Tok::Eq | Tok::Link | Tok::Dot | Tok::Comma);
        if !used_as_name {
            match kw.as_str() {
                "use" => return Ok(vec![Item::Use(self.use_decl()?)]),
                "part" => return Ok(vec![Item::Part(self.part()?)]),
                "rail" => return Ok(self.rails()?.into_iter().map(Item::Rail).collect()),
                "module" => return Ok(vec![Item::Module(self.module()?)]),
                "sheet" => return Ok(vec![Item::Sheet(self.sheet()?)]),
                k if META_KEYS.contains(&k) && matches!(self.peek_at(1), Tok::Str(_)) => {
                    let key = self.ident("key")?;
                    let value = self.atom("value")?;
                    return Ok(vec![Item::Meta { key, value }]);
                }
                _ => {}
            }
        }
        Ok(vec![Item::Stmt(self.stmt()?)])
    }

    fn use_decl(&mut self) -> PResult<Use> {
        let start = self.bump().span;
        if let Tok::Str(s) = self.peek().clone() {
            let sp = self.bump().span;
            return Ok(Use { file: Some(Name { text: s, span: sp }), path: vec![], names: None, span: start.to(sp) });
        }
        let mut path = vec![self.ident("library name")?];
        let mut names = None;
        let mut star = false;
        while self.eat(&Tok::ColonColon) {
            if self.eat(&Tok::LBrace) {
                let mut list = Vec::new();
                while self.peek() != &Tok::RBrace {
                    list.push(self.ident("part or rail name")?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(Tok::RBrace, "`}`")?;
                names = Some(list);
                break;
            } else if self.eat(&Tok::Star) {
                star = true;
                break;
            } else {
                path.push(self.ident("name")?);
            }
        }
        if names.is_none() && !star && path.len() > 1 {
            let last = path.pop().unwrap();
            names = Some(vec![last]);
        }
        Ok(Use { file: None, path, names, span: start.to(self.prev_span()) })
    }

    fn rails(&mut self) -> PResult<Vec<RailDecl>> {
        let start = self.bump().span;
        let mut names = vec![self.ident("rail name")?];
        while self.eat(&Tok::Comma) {
            names.push(self.ident("rail name")?);
        }
        self.eat(&Tok::Colon);
        let kind = self.ident("rail kind (`ground`, `supply` or `negative`)")?;
        let span = start.to(kind.span);
        Ok(names.into_iter().map(|name| RailDecl { name, kind: kind.clone(), span }).collect())
    }

    fn part(&mut self) -> PResult<PartDecl> {
        let start = self.bump().span;
        let name = self.ident("part name")?;
        self.expect(Tok::LBrace, "`{` to start the part body")?;
        let mut props = Vec::new();
        let mut fields = Vec::new();
        let mut pins = Vec::new();
        loop {
            self.skip_semis();
            if self.eat(&Tok::RBrace) {
                break;
            }
            let key = self.ident("part property (prefix, footprint, value, symbol, pins, …)")?;
            match key.text.as_str() {
                "pins" => {
                    self.expect(Tok::LBrace, "`{` to start the pin list")?;
                    loop {
                        self.skip_semis();
                        if self.eat(&Tok::RBrace) {
                            break;
                        }
                        pins.push(self.pin_decl()?);
                    }
                }
                "field" => {
                    let k = self.atom("field name")?;
                    self.eat(&Tok::Eq);
                    let v = self.atom("field value")?;
                    fields.push((k, v));
                }
                k if PART_FLAGS.contains(&k) => props.push((key, None)),
                _ => {
                    let v = self.atom("property value")?;
                    props.push((key, Some(v)));
                }
            }
        }
        Ok(PartDecl { name, props, fields, pins, span: start.to(self.prev_span()) })
    }

    fn pin_decl(&mut self) -> PResult<PinDecl> {
        let start = self.span();
        let mut numbers = vec![self.atom("pin number")?];
        while self.eat(&Tok::Comma) {
            numbers.push(self.atom("pin number")?);
        }
        let name = self.atom("pin name")?;
        let mut ptype = None;
        let mut side = None;
        if self.eat(&Tok::Colon) {
            ptype = Some(self.ident("pin type (input, output, bidirectional, passive, power_in, power_out, …)")?);
        }
        if self.eat(&Tok::At) {
            side = Some(self.ident("side (left, right, top, bottom)")?);
        }
        let mut hidden = false;
        if self.is_kw("hidden") {
            self.bump();
            hidden = true;
        }
        Ok(PinDecl { numbers, name, ptype, side, hidden, span: start.to(self.prev_span()) })
    }

    fn module(&mut self) -> PResult<ModuleDecl> {
        let start = self.bump().span;
        let name = self.ident("module name")?;
        let mut params = Vec::new();
        if self.eat(&Tok::LParen) {
            while self.peek() != &Tok::RParen {
                params.push(self.ident("port name")?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(Tok::RParen, "`)`")?;
        }
        let body = self.block("module body")?;
        Ok(ModuleDecl { name, params, body, span: start.to(self.prev_span()) })
    }

    fn sheet(&mut self) -> PResult<SheetDecl> {
        let start = self.bump().span;
        let name = self.string_or_ident("sheet name")?;
        let body = self.block("sheet body")?;
        Ok(SheetDecl { name, body, span: start.to(self.prev_span()) })
    }

    fn block(&mut self, what: &str) -> PResult<Vec<Stmt>> {
        self.expect(Tok::LBrace, &format!("`{{` to start the {what}"))?;
        let mut body = Vec::new();
        loop {
            self.skip_semis();
            if self.eat(&Tok::RBrace) {
                break;
            }
            if self.peek() == &Tok::Eof {
                return Err(self.err_expected("`}`"));
            }
            body.push(self.stmt()?);
        }
        Ok(body)
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        let start = self.span();
        let next = self.peek_at(1).clone();
        if self.is_kw("layout") && next == Tok::LBrace {
            self.bump();
            return self.layout(start);
        }
        if self.is_kw("nc") && matches!(next, Tok::Ident(_)) {
            self.bump();
            let mut pins = vec![self.endpoint()?];
            while self.eat(&Tok::Comma) {
                pins.push(self.endpoint()?);
            }
            return Ok(Stmt::Nc { pins, span: start.to(self.prev_span()) });
        }
        if self.is_kw("power_flag") && matches!(next, Tok::Ident(_)) {
            self.bump();
            let mut nets = vec![self.ident("net name")?];
            while self.eat(&Tok::Comma) {
                nets.push(self.ident("net name")?);
            }
            return Ok(Stmt::PowerFlag { nets, span: start.to(self.prev_span()) });
        }
        if matches!(self.peek(), Tok::Ident(_)) && next == Tok::Eq {
            let name = self.ident("instance name")?;
            self.bump();
            let ty = self.ident("part or module name")?;
            let mut args = Vec::new();
            if self.eat(&Tok::LParen) {
                while self.peek() != &Tok::RParen {
                    args.push(self.arg()?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(Tok::RParen, "`)` to close the argument list")?;
            }
            return Ok(Stmt::Inst { name, ty, args, span: start.to(self.prev_span()) });
        }
        // Connection chain.
        let mut groups = vec![self.endpoint_list()?];
        if self.peek() != &Tok::Link {
            return Err(self.err_expected("`--` (connection), or `=` (new instance)"));
        }
        while self.eat(&Tok::Link) {
            groups.push(self.endpoint_list()?);
        }
        Ok(Stmt::Connect { groups, span: start.to(self.prev_span()) })
    }

    fn endpoint_list(&mut self) -> PResult<Vec<Endpoint>> {
        let mut list = vec![self.endpoint()?];
        while self.eat(&Tok::Comma) {
            list.push(self.endpoint()?);
        }
        Ok(list)
    }

    fn endpoint(&mut self) -> PResult<Endpoint> {
        if let Tok::Word(w) = self.peek().clone() {
            return Err(Diagnostic::error("syntax", format!("`{w}` is not a net or pin; net names cannot start with a digit"))
                .at(self.span())
                .hint("write pins as `inst.pin` (e.g. `r1.2`) and rails with a sign (e.g. `+5V`)"));
        }
        let first = self.ident("net, port or `inst.pin`")?;
        if first.text == "nc" && self.peek() != &Tok::Dot {
            return Ok(Endpoint::Nc(first.span));
        }
        if self.eat(&Tok::Dot) {
            let pin = self.atom("pin name or number after `.`")?;
            return Ok(Endpoint::Pin { inst: first, pin });
        }
        Ok(Endpoint::Net(first))
    }

    fn arg(&mut self) -> PResult<Arg> {
        let key = if matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Colon {
            let k = self.ident("argument name")?;
            self.bump();
            Some(k)
        } else {
            None
        };
        let v = self.atom("argument value")?;
        if self.peek() == &Tok::Dot {
            self.bump();
            let pin = self.atom("pin name")?;
            return Ok(Arg { key, value: ArgValue::Pin(Endpoint::Pin { inst: v, pin }) });
        }
        Ok(Arg { key, value: ArgValue::Lit(v) })
    }

    fn layout(&mut self, start: Span) -> PResult<Stmt> {
        self.expect(Tok::LBrace, "`{`")?;
        let mut hints = Vec::new();
        loop {
            self.skip_semis();
            if self.eat(&Tok::RBrace) {
                break;
            }
            if self.is_kw("flow") {
                self.bump();
                hints.push(Hint::Flow(self.ident("direction (right, down, left, up)")?));
                continue;
            }
            let inst = self.ident("layout hint (`flow …` or `inst near target`)")?;
            if !self.is_kw("near") {
                return Err(self.err_expected("`near`"));
            }
            self.bump();
            let target = self.endpoint()?;
            hints.push(Hint::Near { inst, target });
        }
        Ok(Stmt::Layout { hints, span: start.to(self.prev_span()) })
    }
}

pub fn parse(file: usize, src: &str) -> Result<File, Diagnostic> {
    let toks = super::lexer::lex(file, src)?;
    Parser::new(toks).parse_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LDO: &str = r#"
use std::{R, C, GND, +3V3}

part AMS1117_3V3 {
  prefix "U"
  footprint "Package_TO_SOT_SMD:SOT-223-3_TabPin2"
  pins {
    1 GND  : power_in  @bottom
    2 VOUT : power_out @right
    3 VIN  : power_in  @left
  }
}

module Ldo(vin, vout) {
  u    = AMS1117_3V3
  cin  = C(10uF, 0805)
  cout = C(22uF, 0805)
  vin  -- u.VIN  -- cin.1
  vout -- u.VOUT -- cout.1
  GND  -- u.GND, cin.2, cout.2
  layout { flow right; cin near u.VIN }
}

sheet "Power" {
  reg = Ldo(vin: VBUS, vout: +3V3)
}
"#;

    #[test]
    fn parses_ldo() {
        let f = parse(0, LDO).unwrap();
        assert_eq!(f.items.len(), 4);
        let Item::Use(u) = &f.items[0] else { panic!() };
        assert_eq!(u.path[0].text, "std");
        assert_eq!(u.names.as_ref().unwrap().len(), 4);
        let Item::Part(p) = &f.items[1] else { panic!() };
        assert_eq!(p.pins.len(), 3);
        assert_eq!(p.pins[1].side.as_ref().unwrap().text, "right");
        let Item::Module(m) = &f.items[2] else { panic!() };
        assert_eq!(m.params.len(), 2);
        assert_eq!(m.body.len(), 7);
        let Stmt::Connect { groups, .. } = &m.body[5] else { panic!() };
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].len(), 3);
    }

    #[test]
    fn reports_position() {
        let e = parse(0, "sheet \"A\" { r = R(1k) r.1 }").unwrap_err();
        assert!(e.message.contains("expected `--`"), "{}", e.message);
    }
}
