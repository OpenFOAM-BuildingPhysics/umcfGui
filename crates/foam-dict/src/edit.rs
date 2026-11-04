//! Edits that touch only the entry they change.

use crate::cst::{Dict, Entry, Item, Node, Tok};
use crate::{Error, Result, unquote};

/// OpenFOAM's usual layout puts values at column 16 (`key            value;`).
const VALUE_COLUMN: usize = 16;
const STEP: &str = "    ";

pub fn set(dict: &mut Dict, path: &[&str], mut nodes: Vec<Node>, indent: &str) -> Result<()> {
    let (key, rest) = path.split_first().ok_or(Error::EmptyPath)?;
    if !rest.is_empty() {
        let child_indent = child_indent(dict, key, indent);
        if dict.get_mut(key).is_none() {
            let entry = new_entry(dict, key, vec![empty_dict()], indent);
            push(dict, entry);
        }
        let entry = dict.get_mut(key).ok_or_else(|| Error::NotFound(key.to_string()))?;
        let sub = entry.dict_mut().ok_or_else(|| Error::NotADict(key.to_string()))?;
        return set(sub, rest, nodes, &child_indent);
    }
    let is_dict = matches!(nodes.as_slice(), [Node::Dict { .. }]);
    let pad_hint = value_pad(dict, key);
    match dict.get_mut(key) {
        Some(e) => {
            let was_value = e.dict().is_none();
            let old_lead = e.value.first().map(|n| first_lead(n).to_string());
            let key_indent = trailing_indent(&e.key.lead).to_string();
            if is_dict {
                if let Some(n) = nodes.first_mut() {
                    layout_dict(n, &key_indent);
                }
                e.end = None;
            } else {
                if let Some(n) = nodes.first_mut() {
                    n.first_tok_mut().lead = match old_lead {
                        Some(l) if was_value && !l.is_empty() => l,
                        _ => pad_hint,
                    };
                }
                if e.end.is_none() {
                    e.end = Some(Tok::new("", ";"));
                }
            }
            e.value = nodes;
        }
        None => {
            let entry = new_entry(dict, key, nodes, indent);
            push(dict, entry);
        }
    }
    Ok(())
}

/// Removes every entry with the key at the end of `path`. The second flag tells the caller that
/// the last item of `dict` went, so the trivia after the dictionary (its closing brace's lead or
/// the document tail) starts with that entry's same-line comment, which should go too.
pub fn remove(dict: &mut Dict, path: &[&str]) -> Result<(bool, bool)> {
    let (key, rest) = path.split_first().ok_or(Error::EmptyPath)?;
    if rest.is_empty() {
        let mut removed = false;
        let mut strip_after = false;
        while let Some(i) = dict
            .items
            .iter()
            .rposition(|i| matches!(i, Item::Entry(e) if !e.key.text.is_empty() && unquote(&e.key.text) == *key))
        {
            dict.items.remove(i);
            removed = true;
            match dict.items.get_mut(i) {
                Some(next) => strip_same_line(&mut next.key_tok_mut().lead),
                None => strip_after = true,
            }
        }
        return Ok((removed, strip_after));
    }
    let Some(entry) = dict.get_mut(key) else { return Ok((false, false)) };
    let Some(Node::Dict { body, close, .. }) = entry.value.first_mut() else { return Ok((false, false)) };
    let (removed, strip) = remove(body, rest)?;
    if strip {
        strip_same_line(&mut close.lead);
    }
    Ok((removed, false))
}

/// Drops a same-line comment at the start of `lead` (the trivia after a removed entry's `;`),
/// keeping the line break and what follows.
pub fn strip_same_line(lead: &mut String) {
    let first_line = lead.find('\n').unwrap_or(lead.len());
    let head = lead.get(..first_line).unwrap_or("");
    if head.trim_start().starts_with("//") || head.trim().is_empty() {
        *lead = lead.get(first_line..).unwrap_or("").to_string();
    }
}

fn empty_dict() -> Node {
    Node::Dict { open: Tok::new("", "{"), body: Dict::default(), close: Tok::new("", "}") }
}

/// A new entry formatted like the last sibling (same blank-line spacing and indentation).
fn new_entry(dict: &Dict, key: &str, mut nodes: Vec<Node>, indent: &str) -> Entry {
    let (newlines, indent) = match dict.items.iter().rev().find(|i| !i.key_tok().text.is_empty()) {
        Some(last) => {
            let ws = trailing_ws(&last.key_tok().lead);
            let nl = ws.matches('\n').count().clamp(1, 2);
            (nl, trailing_indent(ws).to_string())
        }
        None => (1, indent.to_string()),
    };
    let key_text = if needs_quotes(key) { format!("\"{key}\"") } else { key.to_string() };
    let is_dict = matches!(nodes.as_slice(), [Node::Dict { .. }]);
    if is_dict {
        if let Some(n) = nodes.first_mut() {
            layout_dict(n, &indent);
        }
    } else if let Some(n) = nodes.first_mut() {
        n.first_tok_mut().lead = pad(key_text.len(), VALUE_COLUMN);
    }
    let lead = format!("{}{indent}", "\n".repeat(newlines));
    Entry { key: Tok::new(lead, key_text), value: nodes, end: (!is_dict).then(|| Tok::new("", ";")) }
}

fn push(dict: &mut Dict, entry: Entry) {
    dict.items.push(Item::Entry(entry));
}

/// Keys that read back as something else unless quoted: white space and punctuation, and a
/// leading `(`, which would start a list (a pattern key such as `"(U|k)"`).
pub(crate) fn needs_quotes(key: &str) -> bool {
    key.is_empty()
        || key.starts_with('(')
        || key.bytes().any(|b| b.is_ascii_whitespace() || matches!(b, b'{' | b'}' | b';' | b'"' | b'[' | b']'))
}

fn first_lead(n: &Node) -> &str {
    match n {
        Node::Atom(t) => &t.lead,
        Node::List { open, .. } | Node::Dims { open, .. } | Node::Dict { open, .. } => &open.lead,
    }
}

fn trailing_ws(lead: &str) -> &str {
    let start =
        lead.rfind(|c: char| !c.is_whitespace()).map_or(0, |i| i + lead[i..].chars().next().map_or(1, char::len_utf8));
    lead.get(start..).unwrap_or("")
}

fn trailing_indent(lead: &str) -> &str {
    let ws = trailing_ws(lead);
    ws.rfind('\n').map_or(ws, |i| ws.get(i + 1..).unwrap_or(""))
}

fn pad(key_len: usize, column: usize) -> String {
    " ".repeat(column.saturating_sub(key_len).max(1))
}

/// The padding a new value in this dictionary should get: aligned at column 16 when the
/// siblings are aligned, a single space when they are not.
fn value_pad(dict: &Dict, key: &str) -> String {
    let aligned = dict.entries().filter(|e| e.dict().is_none()).last().map(|e| {
        let lead = e.value.first().map_or("", first_lead);
        lead.bytes().all(|b| b == b' ') && e.key.text.len() + lead.len() >= 12
    });
    match aligned {
        Some(false) => " ".into(),
        _ => pad(key.len(), VALUE_COLUMN),
    }
}

/// The indentation of the entries inside `key`'s dictionary.
fn child_indent(dict: &Dict, key: &str, indent: &str) -> String {
    if let Some(sub) = dict.get(key).and_then(Entry::dict)
        && let Some(first) = sub.entries().next()
    {
        return trailing_indent(&first.key.lead).to_string();
    }
    let key_indent = dict.get(key).map_or(indent.to_string(), |e| trailing_indent(&e.key.lead).to_string());
    format!("{key_indent}{STEP}")
}

/// OpenFOAM layout for a dictionary written from scratch: braces on their own lines, entries
/// indented one step, values at column 16.
fn layout_dict(node: &mut Node, indent: &str) {
    let Node::Dict { open, body, close } = node else { return };
    let inner = format!("{indent}{STEP}");
    open.lead = format!("\n{indent}");
    close.lead = format!("\n{indent}");
    for item in &mut body.items {
        let key_len = item.key_tok().text.len();
        item.key_tok_mut().lead = format!("\n{inner}");
        if let Item::Entry(e) = item {
            match e.value.as_mut_slice() {
                [n @ Node::Dict { .. }] => layout_dict(n, &inner),
                [first, ..] => first.first_tok_mut().lead = pad(key_len, VALUE_COLUMN),
                [] => {}
            }
            if let Some(end) = &mut e.end {
                end.lead.clear();
            }
        }
    }
}
