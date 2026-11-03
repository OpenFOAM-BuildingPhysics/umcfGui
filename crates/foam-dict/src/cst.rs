//! The concrete syntax tree. Every node keeps the trivia (whitespace and comments) in front of
//! it, so writing the tree back gives the input byte for byte.

use std::fmt::{self, Write as _};

use crate::lex::{self, Kind, Token};
use crate::{Error, Limits};

/// A token with the trivia in front of it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tok {
    pub lead: String,
    pub text: String,
}

impl Tok {
    pub fn new(lead: impl Into<String>, text: impl Into<String>) -> Self {
        Tok { lead: lead.into(), text: text.into() }
    }
}

/// A value node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// A word, number, string, `$macro`, `#directive` or `#{ … #}` block.
    Atom(Tok),
    /// `( … )`
    List { open: Tok, items: Vec<Node>, close: Tok },
    /// `[ … ]` (dimensions)
    Dims { open: Tok, items: Vec<Node>, close: Tok },
    /// `{ … }`
    Dict { open: Tok, body: Dict, close: Tok },
}

/// The entries of a dictionary (or of a whole file).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Dict {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// `key value… ;` or `key { … }`. A dictionary entry has `value == [Node::Dict]` and no `end`.
    Entry(Entry),
    /// `#include "file"`, `#includeEtc`, `#remove`, `#inputMode`, `#includeFunc …`: the
    /// directive name, its arguments up to the end of the line, and an optional `;`.
    Directive { name: Tok, args: Vec<Node>, end: Option<Tok> },
    /// `$macro` (optionally followed by `;`) at entry level: merges another dictionary in.
    Macro { name: Tok, end: Option<Tok> },
    /// A value without a key: the data files OpenFOAM reads as a bare `List` (`constant/IDN`,
    /// `sunPosVector`, time tables such as `T_room.dat`, probe location lists).
    Value(Node),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: Tok,
    pub value: Vec<Node>,
    /// The closing `;`. `None` for dictionary entries, and for a value entry that ran into `}`
    /// or the end of the file without one (kept, but reported by [`Dict::problems`]).
    pub end: Option<Tok>,
}

/// A parsed file: the top-level entries and the trivia after the last one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document {
    pub root: Dict,
    pub tail: String,
}

// ---------------------------------------------------------------------------------------------
// Parsing

struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    pos: usize,
    depth: usize,
    limits: &'a Limits,
}

impl<'a> Parser<'a> {
    fn text(&self, t: Token) -> &'a str {
        self.src.get(t.start..t.end).unwrap_or("")
    }

    /// Collects trivia, then returns the next significant token without consuming it.
    fn peek(&mut self) -> (String, Option<Token>) {
        let mut lead = String::new();
        let mut p = self.pos;
        while let Some(t) = self.toks.get(p).copied() {
            if t.kind != Kind::Trivia {
                return (lead, Some(t));
            }
            lead.push_str(self.text(t));
            p += 1;
        }
        (lead, None)
    }

    /// Consumes trivia and the next significant token.
    fn bump(&mut self) -> Option<Tok> {
        let mut lead = String::new();
        while let Some(t) = self.toks.get(self.pos).copied() {
            self.pos += 1;
            if t.kind != Kind::Trivia {
                return Some(Tok { lead, text: self.text(t).to_string() });
            }
            lead.push_str(self.text(t));
        }
        None
    }

    fn at(&self, t: Token) -> (usize, usize) {
        lex::line_col(self.src, t.start)
    }

    fn enter(&mut self, t: Token) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > self.limits.max_depth {
            return Err(Error::TooDeep { limit: self.limits.max_depth, at: self.at(t) });
        }
        Ok(())
    }

    /// Entries until `}` (not consumed) or the end of input.
    fn dict(&mut self) -> Result<Dict, Error> {
        let mut items = Vec::new();
        loop {
            let (_, next) = self.peek();
            let Some(t) = next else { break };
            match t.kind {
                Kind::Punct(b'}') => break,
                Kind::Punct(b';') => {
                    // A stray `;` between entries: keep it as an empty entry's end.
                    let end = self.bump();
                    items.push(Item::Entry(Entry { key: Tok::default(), value: Vec::new(), end }));
                }
                Kind::Punct(b'(' | b'[') => items.push(Item::Value(self.node(t)?)),
                Kind::Punct(_) => {
                    let text = self.text(t).to_string();
                    return Err(Error::Unexpected { found: text, expected: "a keyword", at: self.at(t) });
                }
                _ => items.push(self.item(t)?),
            }
        }
        Ok(Dict { items })
    }

    fn item(&mut self, first: Token) -> Result<Item, Error> {
        let text = self.text(first);
        let key = self.bump().ok_or(Error::Eof)?;
        if first.kind == Kind::Word && text.starts_with('#') {
            let args = self.directive_args();
            let end = self.semicolon();
            return Ok(Item::Directive { name: key, args, end });
        }
        if first.kind == Kind::Word && text.starts_with('$') {
            let end = self.semicolon();
            return Ok(Item::Macro { name: key, end });
        }
        let mut value = Vec::new();
        loop {
            let (_, next) = self.peek();
            let Some(t) = next else { break };
            match t.kind {
                Kind::Punct(b';') => {
                    let end = self.bump();
                    return Ok(Item::Entry(Entry { key, value, end }));
                }
                Kind::Punct(b'}') => break,
                Kind::Punct(b'{') if value.is_empty() => {
                    value.push(self.node(t)?);
                    return Ok(Item::Entry(Entry { key, value, end: None }));
                }
                _ => value.push(self.node(t)?),
            }
        }
        Ok(Item::Entry(Entry { key, value, end: None }))
    }

    /// A directive's arguments, kept as flat atoms: up to the end of the line, or further while
    /// parentheses are open. `#includeFunc`-style function calls may put their `(` on the next
    /// line and spread their arguments over several lines, with words like `end=(1 0 0),` whose
    /// brackets don't nest the way values do.
    fn directive_args(&mut self) -> Vec<Node> {
        let mut args = Vec::new();
        let mut depth = 0usize;
        loop {
            let (lead, next) = self.peek();
            let Some(t) = next else { break };
            // `#includeFunc name` followed by `(` on the next line: the call continues.
            let call_on_next_line = t.kind == Kind::Punct(b'(')
                && match args.as_slice() {
                    [] => true,
                    [Node::Atom(name)] => !name.text.contains('(') && !name.text.starts_with('"'),
                    _ => false,
                };
            if depth == 0 && !call_on_next_line && (lead.contains('\n') || matches!(t.kind, Kind::Punct(b'}' | b';'))) {
                break;
            }
            if t.kind != Kind::Str {
                let text = self.text(t);
                let opens = text.bytes().filter(|b| *b == b'(').count();
                let closes = text.bytes().filter(|b| *b == b')').count();
                depth = (depth + opens).saturating_sub(closes);
            }
            match self.bump() {
                Some(tok) => args.push(Node::Atom(tok)),
                None => break,
            }
        }
        args
    }

    fn semicolon(&mut self) -> Option<Tok> {
        match self.peek() {
            (_, Some(t)) if t.kind == Kind::Punct(b';') => self.bump(),
            _ => None,
        }
    }

    fn node(&mut self, t: Token) -> Result<Node, Error> {
        match t.kind {
            Kind::Punct(b'(') => {
                self.enter(t)?;
                let open = self.bump().ok_or(Error::Eof)?;
                let items = self.nodes_until(b')', t)?;
                let close = self.close(b')', t)?;
                self.depth -= 1;
                Ok(Node::List { open, items, close })
            }
            Kind::Punct(b'[') => {
                self.enter(t)?;
                let open = self.bump().ok_or(Error::Eof)?;
                let items = self.nodes_until(b']', t)?;
                let close = self.close(b']', t)?;
                self.depth -= 1;
                Ok(Node::Dims { open, items, close })
            }
            Kind::Punct(b'{') => {
                self.enter(t)?;
                let open = self.bump().ok_or(Error::Eof)?;
                let body = self.dict()?;
                let close = self.close(b'}', t)?;
                self.depth -= 1;
                Ok(Node::Dict { open, body, close })
            }
            Kind::Punct(_) => {
                let text = self.text(t).to_string();
                Err(Error::Unexpected { found: text, expected: "a value", at: self.at(t) })
            }
            _ => Ok(Node::Atom(self.bump().ok_or(Error::Eof)?)),
        }
    }

    /// Nodes inside `( )` or `[ ]`. A `;` there is kept as an atom.
    fn nodes_until(&mut self, close: u8, opened: Token) -> Result<Vec<Node>, Error> {
        let mut items = Vec::new();
        loop {
            let (_, next) = self.peek();
            let Some(t) = next else {
                return Err(Error::Unclosed { open: char::from(opened_byte(close)), at: self.at(opened) });
            };
            match t.kind {
                Kind::Punct(b) if b == close => return Ok(items),
                Kind::Punct(b';') => items.push(Node::Atom(self.bump().ok_or(Error::Eof)?)),
                Kind::Punct(b')' | b']' | b'}') => {
                    let text = self.text(t).to_string();
                    return Err(Error::Unexpected {
                        found: text,
                        expected: "a value or closing bracket",
                        at: self.at(t),
                    });
                }
                _ => items.push(self.node(t)?),
            }
        }
    }

    fn close(&mut self, close: u8, opened: Token) -> Result<Tok, Error> {
        match self.peek() {
            (_, Some(t)) if t.kind == Kind::Punct(close) => self.bump().ok_or(Error::Eof),
            _ => Err(Error::Unclosed { open: char::from(opened_byte(close)), at: self.at(opened) }),
        }
    }
}

fn opened_byte(close: u8) -> u8 {
    match close {
        b')' => b'(',
        b']' => b'[',
        _ => b'{',
    }
}

pub fn parse(src: &str, limits: &Limits) -> Result<Document, Error> {
    let toks = lex::tokenize(src, limits)?;
    let mut p = Parser { src, toks, pos: 0, depth: 0, limits };
    let mut root = p.dict()?;
    // Only a stray `}` stops the top level early. OpenFOAM tolerates one at the end of a file
    // (some tutorials have it), so keep it and let `problems()` report it.
    while let (_, Some(_)) = p.peek() {
        if let Some(tok) = p.bump() {
            root.items.push(Item::Value(Node::Atom(tok)));
        }
        let more = p.dict()?;
        root.items.extend(more.items);
    }
    let (tail, _) = p.peek();
    Ok(Document { root, tail })
}

/// Parses a value as it would appear after a keyword (`uniform (1 0 0)`, `{ a 1; }`).
pub fn parse_value(src: &str, limits: &Limits) -> Result<Vec<Node>, Error> {
    let toks = lex::tokenize(src, limits)?;
    let mut p = Parser { src, toks, pos: 0, depth: 0, limits };
    let mut out = Vec::new();
    while let (_, Some(t)) = p.peek() {
        if t.kind == Kind::Punct(b';') {
            return Err(Error::Unexpected { found: ";".into(), expected: "a value (without `;`)", at: p.at(t) });
        }
        out.push(p.node(t)?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Writing

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.lead)?;
        f.write_str(&self.text)
    }
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Node::Atom(t) => t.fmt(f),
            Node::List { open, items, close } | Node::Dims { open, items, close } => {
                open.fmt(f)?;
                items.iter().try_for_each(|n| n.fmt(f))?;
                close.fmt(f)
            }
            Node::Dict { open, body, close } => {
                open.fmt(f)?;
                body.fmt(f)?;
                close.fmt(f)
            }
        }
    }
}

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Item::Entry(e) => {
                e.key.fmt(f)?;
                e.value.iter().try_for_each(|n| n.fmt(f))?;
                e.end.iter().try_for_each(|t| t.fmt(f))
            }
            Item::Directive { name, args, end } => {
                name.fmt(f)?;
                args.iter().try_for_each(|n| n.fmt(f))?;
                end.iter().try_for_each(|t| t.fmt(f))
            }
            Item::Macro { name, end } => {
                name.fmt(f)?;
                end.iter().try_for_each(|t| t.fmt(f))
            }
            Item::Value(n) => n.fmt(f),
        }
    }
}

impl fmt::Display for Dict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.items.iter().try_for_each(|i| i.fmt(f))
    }
}

impl fmt::Display for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.root.fmt(f)?;
        f.write_str(&self.tail)
    }
}

impl Node {
    /// The node without comments, whitespace collapsed to single spaces
    /// (`uniform (1 0 0)`, `[0 1 -1 0 0 0 0]`).
    pub fn compact(&self) -> String {
        let mut s = String::new();
        self.write_compact(&mut s);
        s
    }

    fn write_compact(&self, s: &mut String) {
        match self {
            Node::Atom(t) => s.push_str(&t.text),
            Node::List { open, items, close } | Node::Dims { open, items, close } => {
                s.push_str(&open.text);
                for (i, n) in items.iter().enumerate() {
                    if i > 0 {
                        s.push(' ');
                    }
                    n.write_compact(s);
                }
                s.push_str(&close.text);
            }
            Node::Dict { body, .. } => {
                s.push('{');
                for item in &body.items {
                    s.push(' ');
                    item.write_compact(s);
                }
                s.push_str(" }");
            }
        }
    }

    pub fn first_tok(&self) -> &Tok {
        match self {
            Node::Atom(t) => t,
            Node::List { open, .. } | Node::Dims { open, .. } | Node::Dict { open, .. } => open,
        }
    }

    /// The first token of the node, which carries its leading trivia.
    pub fn first_tok_mut(&mut self) -> &mut Tok {
        match self {
            Node::Atom(t) => t,
            Node::List { open, .. } | Node::Dims { open, .. } | Node::Dict { open, .. } => open,
        }
    }
}

impl Item {
    fn write_compact(&self, s: &mut String) {
        if let Item::Value(n) = self {
            n.write_compact(s);
            return;
        }
        let _ = write!(s, "{}", self.key_tok().text);
        match self {
            Item::Entry(e) => {
                for n in &e.value {
                    s.push(' ');
                    n.write_compact(s);
                }
                if e.end.is_some() {
                    s.push(';');
                }
            }
            Item::Directive { args, .. } => {
                for n in args {
                    s.push(' ');
                    n.write_compact(s);
                }
            }
            Item::Macro { end, .. } => {
                if end.is_some() {
                    s.push(';');
                }
            }
            Item::Value(_) => {}
        }
    }

    /// The first token: the key, or for a keyless value its opening bracket. It carries the
    /// item's leading trivia.
    pub fn key_tok(&self) -> &Tok {
        match self {
            Item::Entry(e) => &e.key,
            Item::Directive { name, .. } | Item::Macro { name, .. } => name,
            Item::Value(n) => n.first_tok(),
        }
    }

    pub fn key_tok_mut(&mut self) -> &mut Tok {
        match self {
            Item::Entry(e) => &mut e.key,
            Item::Directive { name, .. } | Item::Macro { name, .. } => name,
            Item::Value(n) => n.first_tok_mut(),
        }
    }
}
