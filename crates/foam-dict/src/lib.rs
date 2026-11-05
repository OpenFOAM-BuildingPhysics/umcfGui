//! Lossless OpenFOAM dictionary parser and editor.
//!
//! [`Document::parse`] keeps every byte: comments, alignment, `#include`, `$macros`, `#calc`,
//! `#codeStream` and keys it doesn't understand. Writing the document back
//! (`doc.to_string()`) gives the input unchanged, and the edit operations ([`Document::set`],
//! [`Document::remove`]) change only the entry they touch.
//!
//! The crate knows OpenFOAM syntax only. It does not expand `#include` or `$macros` (that
//! needs the case on disk and is the model's job), and it works for OpenFOAM-8 and
//! OpenFOAM-12 files alike.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

pub mod cst;
mod edit;
pub mod lex;

pub use cst::{Dict, Document, Entry, Item, Node, Tok};

/// Bounds on hostile input. Big field files (`nonuniform List<scalar> …` with millions of
/// values) belong to `foam-field`, which streams them; the defaults stop them from filling
/// memory here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_tokens: usize,
    /// Nesting of `{ }`, `( )` and `[ ]`.
    pub max_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_bytes: 64 << 20, max_tokens: 4_000_000, max_depth: 128 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("unterminated {what} starting at line {}, column {}", at.0, at.1)]
    Unterminated { what: &'static str, at: (usize, usize) },
    #[error("`{open}` at line {}, column {} is never closed", at.0, at.1)]
    Unclosed { open: char, at: (usize, usize) },
    #[error("unexpected `{found}` at line {}, column {} (expected {expected})", at.0, at.1)]
    Unexpected { found: String, expected: &'static str, at: (usize, usize) },
    #[error("nesting deeper than {limit} levels at line {}, column {}", at.0, at.1)]
    TooDeep { limit: usize, at: (usize, usize) },
    #[error("{what} exceeds the limit of {limit}; large field data needs the field reader")]
    TooLarge { what: &'static str, limit: usize },
    #[error("unexpected end of input")]
    Eof,
    #[error("the file is not UTF-8 text (binary format?)")]
    NotText,
    #[error("no entry `{0}`")]
    NotFound(String),
    #[error("`{0}` is not a dictionary")]
    NotADict(String),
    #[error("`{0}` is not a list")]
    NotAList(String),
    #[error("empty key path")]
    EmptyPath,
}

pub type Result<T> = std::result::Result<T, Error>;

/// The entry's key without quotes (`"(U|k)"` → `(U|k)`).
pub fn unquote(s: &str) -> &str {
    s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(s)
}

impl Document {
    pub fn parse(src: &str) -> Result<Document> {
        cst::parse(src, &Limits::default())
    }

    pub fn parse_with(src: &str, limits: &Limits) -> Result<Document> {
        cst::parse(src, limits)
    }

    /// Parses bytes read from disk; binary (non-UTF-8) content is an error, not a panic.
    pub fn parse_bytes(bytes: &[u8]) -> Result<Document> {
        let src = std::str::from_utf8(bytes).map_err(|_| Error::NotText)?;
        Document::parse(src)
    }

    /// The entry at `path` (`["solvers", "p", "solver"]`). With duplicate keys the last one
    /// wins, as in OpenFOAM.
    pub fn get(&self, path: &[&str]) -> Option<&Entry> {
        self.root.get_path(path)
    }

    /// `get(path)` as compact text (`"Gauss linear"`), for value entries.
    pub fn value(&self, path: &[&str]) -> Option<String> {
        self.get(path).and_then(Entry::value_text)
    }

    /// The `FoamFile` header entry, e.g. `header("class")` → `volScalarField`.
    pub fn header(&self, key: &str) -> Option<String> {
        self.value(&["FoamFile", key]).map(|v| unquote(&v).to_string())
    }

    /// Sets the entry at `path` to `value` (OpenFOAM syntax without the `;`, e.g.
    /// `uniform (1 0 0)` or `{ type zeroGradient; }`). An existing entry keeps its key, its
    /// comments and its alignment; only the value changes. A missing entry (and any missing
    /// parent dictionaries) is appended to its parent, indented like its siblings.
    pub fn set(&mut self, path: &[&str], value: &str) -> Result<()> {
        let nodes = cst::parse_value(value, &Limits::default())?;
        edit::set(&mut self.root, path, nodes, "")
    }

    /// Appends `value` (one value: `{ name a; }`, `b`) to the list that is the entry at `path`
    /// (`patches ( … );`), on a line of its own, indented like the items before it. Nothing
    /// else in the file changes.
    pub fn append(&mut self, path: &[&str], value: &str) -> Result<()> {
        let mut nodes = cst::parse_value(value, &Limits::default())?;
        let node = match (nodes.pop(), nodes.is_empty()) {
            (Some(n), true) => n,
            _ => return Err(Error::Unexpected { found: value.to_string(), expected: "one value", at: (1, 1) }),
        };
        edit::append(&mut self.root, path, node)
    }

    /// Removes the entry at `path`, with the comments and blank lines in front of it.
    /// Returns whether there was one.
    pub fn remove(&mut self, path: &[&str]) -> Result<bool> {
        let (removed, strip_tail) = edit::remove(&mut self.root, path)?;
        if strip_tail {
            edit::strip_same_line(&mut self.tail);
        }
        Ok(removed)
    }
}

impl Dict {
    /// Sets the entry at `path` below this dictionary, as [`Document::set`] does from the root:
    /// an existing entry keeps its key, comments and alignment; a new one is laid out like its
    /// siblings.
    pub fn set(&mut self, path: &[&str], value: &str) -> Result<()> {
        let nodes = cst::parse_value(value, &Limits::default())?;
        edit::set(self, path, nodes, "")
    }

    /// Gives the entry `key` (the last one with that key) the key `to`, keeping its value,
    /// comments and layout. `false` when there is no such entry.
    pub fn rename(&mut self, key: &str, to: &str) -> bool {
        match self.get_mut(key) {
            Some(e) => {
                e.key.text = if edit::needs_quotes(to) { format!("\"{to}\"") } else { to.to_string() };
                true
            }
            None => false,
        }
    }

    /// The last entry with this key (keys compare without quotes). Directives and macros are
    /// not entries.
    pub fn get(&self, key: &str) -> Option<&Entry> {
        self.entries().filter(|e| !e.key.text.is_empty() && unquote(&e.key.text) == key).last()
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Entry> {
        self.items
            .iter_mut()
            .filter_map(|i| match i {
                Item::Entry(e) if !e.key.text.is_empty() && unquote(&e.key.text) == key => Some(e),
                _ => None,
            })
            .last()
    }

    pub fn get_path(&self, path: &[&str]) -> Option<&Entry> {
        let (first, rest) = path.split_first()?;
        let e = self.get(first)?;
        if rest.is_empty() { Some(e) } else { e.dict()?.get_path(rest) }
    }

    /// Value and dictionary entries, in file order.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.items.iter().filter_map(|i| match i {
            Item::Entry(e) if !e.key.text.is_empty() => Some(e),
            _ => None,
        })
    }

    /// Keyless values (the data of a bare-list file such as `constant/IDN`).
    pub fn values(&self) -> impl Iterator<Item = &Node> {
        self.items.iter().filter_map(|i| match i {
            Item::Value(n) => Some(n),
            _ => None,
        })
    }

    /// Keys in file order (unquoted, duplicates kept).
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries().map(|e| unquote(&e.key.text))
    }

    /// `#include`, `#includeEtc`, … directives with their first argument (unquoted).
    pub fn directives(&self) -> impl Iterator<Item = (&str, Option<String>)> {
        self.items.iter().filter_map(|i| match i {
            Item::Directive { name, args, .. } => {
                Some((name.text.as_str(), args.first().map(|a| unquote(&a.compact()).to_string())))
            }
            _ => None,
        })
    }

    /// Things the parser accepted to stay lossless but OpenFOAM would reject or warn about:
    /// value entries without a closing `;` and stray `;`. Paths are `/`-joined.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_problems("", &mut out);
        out
    }

    fn collect_problems(&self, prefix: &str, out: &mut Vec<String>) {
        for item in &self.items {
            if let Item::Value(Node::Atom(t)) = item {
                out.push(format!(
                    "{}stray `{}`",
                    if prefix.is_empty() { String::new() } else { format!("{prefix}: ") },
                    t.text
                ));
                continue;
            }
            let Item::Entry(e) = item else { continue };
            let key = unquote(&e.key.text);
            let path = if prefix.is_empty() { key.to_string() } else { format!("{prefix}/{key}") };
            if key.is_empty() {
                out.push(format!("{}stray `;`", if prefix.is_empty() { String::new() } else { format!("{prefix}: ") }));
            } else if let Some(d) = e.dict() {
                d.collect_problems(&path, out);
            } else if e.end.is_none() {
                out.push(format!("{path}: missing `;`"));
            }
        }
    }
}

impl Entry {
    /// The key without quotes.
    pub fn key(&self) -> &str {
        unquote(&self.key.text)
    }

    /// The sub-dictionary of a dictionary entry (`key { … }`).
    pub fn dict(&self) -> Option<&Dict> {
        match self.value.as_slice() {
            [Node::Dict { body, .. }] if self.end.is_none() => Some(body),
            _ => None,
        }
    }

    pub fn dict_mut(&mut self) -> Option<&mut Dict> {
        let end_none = self.end.is_none();
        match self.value.as_mut_slice() {
            [Node::Dict { body, .. }] if end_none => Some(body),
            _ => None,
        }
    }

    /// The value as compact text (`Gauss linear`, `uniform (1 0 0)`); `None` for dictionaries.
    pub fn value_text(&self) -> Option<String> {
        if self.dict().is_some() {
            return None;
        }
        Some(self.value.iter().map(Node::compact).collect::<Vec<_>>().join(" "))
    }

    /// The value's atoms when it is a flat sequence of words/numbers/strings.
    pub fn words(&self) -> Option<Vec<&str>> {
        self.value
            .iter()
            .map(|n| match n {
                Node::Atom(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The value as one number (`0.5`, `1e-5`).
    pub fn as_f64(&self) -> Option<f64> {
        match self.words()?.as_slice() {
            [w] => w.parse().ok().filter(|v: &f64| v.is_finite()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
