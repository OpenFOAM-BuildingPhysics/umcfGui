//! Edits that touch only the entry they change.

use crate::cst::{Dict, Entry, Item, Node, Tok};
use crate::{Error, Result, unquote};

/// OpenFOAM's usual layout puts values at column 16 (`key            value;`).
const VALUE_COLUMN: usize = 16;
const STEP: &str = "    ";

/// Sets the entry at `path`. `inline`: a dictionary value keeps the layout it is given
/// (`{ role side; }` on the key's line), as a plain value does; otherwise it is laid out over
/// lines like its siblings.
pub fn set(dict: &mut Dict, path: &[&str], mut nodes: Vec<Node>, indent: &str, inline: bool) -> Result<()> {
    let (key, rest) = path.split_first().ok_or(Error::EmptyPath)?;
    if !rest.is_empty() {
        let child_indent = child_indent(dict, key, indent);
        if dict.get_mut(key).is_none() {
            let entry = new_entry(dict, key, vec![empty_dict()], indent, false);
            push(dict, entry);
        }
        let entry = dict.get_mut(key).ok_or_else(|| Error::NotFound(key.to_string()))?;
        let Some(Node::Dict { body, close, .. }) = entry.value.first_mut() else {
            return Err(Error::NotADict(key.to_string()));
        };
        let before = body.items.len();
        set(body, rest, nodes, &child_indent, inline)?;
        if body.items.len() > before {
            adopt_same_line(&mut close.lead, body);
        }
        return Ok(());
    }
    let dict_value = matches!(nodes.as_slice(), [Node::Dict { .. }]);
    let is_dict = dict_value && !inline;
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
                        // A value's own layout stays (on the next line, say); an inline dictionary
                        // over one laid out by hand goes on the key's line.
                        // A comment in front of it stays, with the line break that ends it.
                        Some(l)
                            if !l.is_empty()
                                && (was_value
                                    || (inline && (!l.contains('\n') || l.contains("//") || l.contains("/*")))) =>
                        {
                            l
                        }
                        _ => pad_hint,
                    };
                }
                if dict_value {
                    e.end = None;
                } else if e.end.is_none() {
                    e.end = Some(Tok::new("", ";"));
                }
            }
            e.value = nodes;
        }
        None => {
            let entry = new_entry(dict, key, nodes, indent, inline);
            push(dict, entry);
        }
    }
    Ok(())
}

/// Appends `node` to the list that is the entry at the end of `path`, on a line of its own.
pub fn append(dict: &mut Dict, path: &[&str], mut node: Node) -> Result<()> {
    let (key, rest) = path.split_first().ok_or(Error::EmptyPath)?;
    let entry = dict.get_mut(key).ok_or_else(|| Error::NotFound(key.to_string()))?;
    if !rest.is_empty() {
        let sub = entry.dict_mut().ok_or_else(|| Error::NotADict(key.to_string()))?;
        return append(sub, rest, node);
    }
    let key_indent = trailing_indent(&entry.key.lead).to_string();
    let [Node::List { items, close, .. }] = entry.value.as_mut_slice() else {
        return Err(Error::NotAList(key.to_string()));
    };
    // Like the items before, when they are on lines of their own; one step in otherwise.
    let indent = match items.last().map(first_lead) {
        Some(lead) if lead.contains('\n') => trailing_indent(lead).to_string(),
        _ => format!("{key_indent}{STEP}"),
    };
    match &mut node {
        Node::Dict { .. } => layout_dict(&mut node, &indent),
        other => other.first_tok_mut().lead = format!("\n{indent}"),
    }
    let kept = before_close(close, &key_indent);
    if !kept.is_empty() {
        let lead = &mut node.first_tok_mut().lead;
        *lead = format!("{kept}{lead}");
    }
    items.push(node);
    Ok(())
}

/// What is before a list's `)` that stays in front of an item added on a line of its own (`( a
/// /* note */ )`, `( a // note` and a new line): taken from the `)`'s lead, which keeps its own
/// line, or gets one. A comment on the `)`'s own line (`    /* one */ )`) goes too: the `)` gets
/// a line of its own.
fn before_close(close: &mut Tok, key_indent: &str) -> String {
    let (kept, rest) = match last_free_newline(&close.lead) {
        Some(i) => {
            let tail = close.lead.get(i..).unwrap_or("");
            if tail.trim().is_empty() {
                (close.lead.get(..i).unwrap_or("").to_string(), tail.to_string())
            } else {
                (close.lead.clone(), format!("\n{key_indent}"))
            }
        }
        None => (close.lead.clone(), format!("\n{key_indent}")),
    };
    close.lead = rest;
    kept
}

/// Adds the word `value` at the end of a list's `items`, laid out as the list is: on a line of
/// its own like the item before when the items are on lines of their own, else after a space
/// (`()` gets `(a)`, `( )` gets `( a )`). What is before the `)` on the last item's line (a
/// comment) stays after that item, in front of the new one.
fn push_word(items: &mut Vec<Node>, close: &mut Tok, value: &str) {
    let lead = match items.last().map(first_lead) {
        Some(l) if l.contains('\n') => format!("\n{}", trailing_indent(l)),
        Some(_) => " ".to_string(),
        None if close.lead.is_empty() => String::new(),
        None => " ".to_string(),
    };
    // On one line: what is there but the spaces before the `)`.
    let at = last_free_newline(&close.lead).unwrap_or_else(|| close.lead.trim_end().len());
    let kept = close.lead.get(..at).unwrap_or("").to_string();
    close.lead = close.lead.get(at..).unwrap_or("").to_string();
    // After a `// …` comment the word needs a line of its own, or the comment swallows it:
    // one step in from the `)`.
    let lead = if ends_in_line_comment(&kept) && !lead.starts_with('\n') {
        format!("\n{}{STEP}", trailing_indent(&close.lead))
    } else {
        lead
    };
    items.push(Node::Atom(Tok::new(format!("{kept}{lead}"), value)));
}

/// Adds the word `value` at the end of the list that is the entry at `path` (`solid ( street );`
/// in a dictionary), laid out as the list is, its comments kept where they are.
pub fn add_word(dict: &mut Dict, path: &[&str], value: &str) -> Result<()> {
    one_word(value)?;
    let (key, rest) = path.split_first().ok_or(Error::EmptyPath)?;
    let entry = dict.get_mut(key).ok_or_else(|| Error::NotFound(key.to_string()))?;
    if !rest.is_empty() {
        let sub = entry.dict_mut().ok_or_else(|| Error::NotADict(key.to_string()))?;
        return add_word(sub, rest, value);
    }
    let [Node::List { items, close, .. }] = entry.value.as_mut_slice() else {
        return Err(Error::NotAList(key.to_string()));
    };
    push_word(items, close, value);
    Ok(())
}

/// In the list that is the entry `key` of `dict`, made of words each followed by a list
/// (`regions ( fluid (air) solid (street) vegetation () );`), adds the word `value` at the end
/// of the list after `word`, on its line. Without `word`, adds `word (value)` at the end, on a
/// line of its own like the pairs before. Nothing else changes.
pub fn add_to_sublist(dict: &mut Dict, key: &str, word: &str, value: &str) -> Result<()> {
    one_word(word)?;
    one_word(value)?;
    let entry = dict.get_mut(key).ok_or_else(|| Error::NotFound(key.to_string()))?;
    let key_indent = trailing_indent(&entry.key.lead).to_string();
    let [Node::List { items, close, .. }] = entry.value.as_mut_slice() else {
        return Err(Error::NotAList(key.to_string()));
    };
    let is_word = |n: &Node| matches!(n, Node::Atom(t) if unquote(&t.text) == word);
    let at = items.iter().position(is_word);
    if let Some(i) = at {
        let Some(Node::List { items: inner, close: inner_close, .. }) = items.get_mut(i + 1) else {
            return Err(Error::NotAList(format!("{key}/{word}")));
        };
        push_word(inner, inner_close, value);
        return Ok(());
    }
    // A list on one line gets the pair on it: `( fluid (air) solid (x) )`; not after a `// …`
    // comment, which would swallow it.
    if items.first().is_some_and(|n| !first_lead(n).contains('\n')) && !ends_in_line_comment(close.lead.trim_end()) {
        let trailing = close.lead.len() - close.lead.trim_end().len();
        let at = close.lead.len() - trailing;
        let (kept, rest) =
            (close.lead.get(..at).unwrap_or("").to_string(), close.lead.get(at..).unwrap_or("").to_string());
        close.lead = rest;
        items.push(Node::Atom(Tok::new(format!("{kept} "), word)));
        items.push(Node::List {
            open: Tok::new(" ", "("),
            items: vec![Node::Atom(Tok::new("", value))],
            close: Tok::new("", ")"),
        });
        return Ok(());
    }
    // A new pair, laid out like the first one: its indent, its word's list at the same column.
    let (indent, column) = match items.first() {
        Some(Node::Atom(t)) if t.lead.contains('\n') => {
            let gap = match items.get(1) {
                Some(Node::List { open, .. }) => open.lead.len(),
                _ => 1,
            };
            (trailing_indent(&t.lead).to_string(), t.text.len() + gap)
        }
        _ => (format!("{key_indent}{STEP}"), word.len() + 1),
    };
    let mut kept = before_close(close, &key_indent);
    // No spaces left at the end of the line the pair's word leaves.
    kept.truncate(kept.trim_end_matches([' ', '\t']).len());
    let gap = " ".repeat(column.saturating_sub(word.len()).max(1));
    items.push(Node::Atom(Tok::new(format!("{kept}\n{indent}"), word)));
    items.push(Node::List {
        open: Tok::new(gap, "("),
        items: vec![Node::Atom(Tok::new("", value))],
        close: Tok::new("", ")"),
    });
    Ok(())
}

/// The last line break of trivia `t` that isn't inside a `/* … */` comment (the one ending a
/// `//` comment counts).
fn last_free_newline(t: &str) -> Option<usize> {
    scan_trivia(t).0
}

/// Whether trivia `t` ends inside a `// …` comment (one not ended by a line break).
fn ends_in_line_comment(t: &str) -> bool {
    scan_trivia(t).1
}

/// Trivia `t` read once: its last line break outside a `/* … */` comment, and whether it ends
/// inside a `// …` comment.
fn scan_trivia(t: &str) -> (Option<usize>, bool) {
    let b = t.as_bytes();
    let at = |i: usize| b.get(i).copied();
    let (mut i, mut last, mut block, mut line) = (0, None, false, false);
    while let Some(c) = at(i) {
        let next = at(i + 1);
        if block {
            if c == b'*' && next == Some(b'/') {
                block = false;
                i += 1;
            }
        } else if c == b'\n' {
            line = false;
            last = Some(i);
        } else if !line && c == b'/' && next == Some(b'*') {
            block = true;
            i += 1;
        } else if !line && c == b'/' && next == Some(b'/') {
            line = true;
            i += 1;
        }
        i += 1;
    }
    (last, line)
}

/// Whether `w` reads back as one token of a list: not empty, without white space, brackets,
/// `;`, quotes or the start of a comment (a `$macro` or `#directive` is one token too, read as
/// such).
fn is_one_word(w: &str) -> bool {
    !w.is_empty()
        && !w.contains("//")
        && !w.contains("/*")
        && !w
            .bytes()
            .any(|b| b.is_ascii_whitespace() || matches!(b, b'(' | b')' | b'{' | b'}' | b'[' | b']' | b';' | b'"'))
}

fn one_word(w: &str) -> Result<()> {
    if is_one_word(w) { Ok(()) } else { Err(Error::NotAWord(w.to_string())) }
}

/// Removes every entry with the key at the end of `path`. `Some(head)` tells the caller that the
/// last item of `dict` went: the trivia after the dictionary (its closing brace's lead or the
/// document tail) starts with that entry's same-line comment, which should go
/// ([`strip_same_line`]), and `head` (the same-line comment of the entry before it, which the
/// removed entry's lead held) goes in front of it ([`after_removed`]).
pub fn remove(dict: &mut Dict, path: &[&str]) -> Result<(bool, Option<String>)> {
    let (key, rest) = path.split_first().ok_or(Error::EmptyPath)?;
    if rest.is_empty() {
        let mut removed = false;
        let mut after = None;
        while let Some(i) = dict
            .items
            .iter()
            .rposition(|i| matches!(i, Item::Entry(e) if !e.key.text.is_empty() && unquote(&e.key.text) == *key))
        {
            let gone = dict.items.remove(i);
            let gone_lead = gone.key_tok().lead.clone();
            let head = same_line_head(&gone_lead);
            removed = true;
            match dict.items.get_mut(i) {
                Some(next) => {
                    let lead = &mut next.key_tok_mut().lead;
                    strip_same_line(lead);
                    if lead.contains('\n') {
                        after_removed(lead, &head);
                    } else {
                        // The next entry was on the removed one's line: it goes where that one
                        // began (its line and indent), after the comments of the one before.
                        let mut ws = trailing_ws(&gone_lead).to_string();
                        if ends_in_line_comment(&head) && !ws.contains('\n') {
                            ws = format!("\n{ws}");
                        }
                        // A CRLF's `\r` ends the head: the removed one's line break brings its own.
                        let head = if ws.contains('\n') { head.trim_end_matches('\r') } else { &head };
                        *lead = format!("{head}{ws}{}", lead.trim_start());
                    }
                }
                None => after = Some(head),
            }
        }
        return Ok((removed, after));
    }
    let Some(entry) = dict.get_mut(key) else { return Ok((false, None)) };
    let Some(Node::Dict { body, close, .. }) = entry.value.first_mut() else { return Ok((false, None)) };
    let (removed, after) = remove(body, rest)?;
    if let Some(head) = after {
        after_removed(&mut close.lead, &head);
    }
    Ok((removed, None))
}

/// The comment at the start of a lead, on the line of what came before it (the same-line comment
/// of the entry before): kept when the entry the lead belongs to is removed.
fn same_line_head(lead: &str) -> String {
    match first_free_newline(lead) {
        Some(n) if !lead[..n].trim().is_empty() => lead[..n].to_string(),
        Some(_) => String::new(),
        // On one line (`a 1; /* ca */ b 2;`): the comments are the entry's before it.
        None => lead.trim_end().to_string(),
    }
}

/// The first line break of trivia `t` that isn't inside a `/* … */` comment.
fn first_free_newline(t: &str) -> Option<usize> {
    let b = t.as_bytes();
    let (mut i, mut block, mut line) = (0, false, false);
    while let Some(&c) = b.get(i) {
        let next = b.get(i + 1).copied();
        if block {
            if c == b'*' && next == Some(b'/') {
                block = false;
                i += 1;
            }
        } else if c == b'\n' {
            return Some(i);
        } else if !line && c == b'/' && next == Some(b'*') {
            block = true;
            i += 1;
        } else if !line && c == b'/' && next == Some(b'/') {
            line = true;
            i += 1;
        }
        i += 1;
    }
    None
}

/// The trivia after a removed entry (the next entry's lead, a closing brace's, the document
/// tail): the removed entry's same-line comment taken out, the one before it (`head`) put back;
/// what follows a `// …` comment of it goes on a line of its own.
pub fn after_removed(trivia: &mut String, head: &str) {
    strip_same_line(trivia);
    let rest = trivia.trim_start_matches([' ', '\t']);
    *trivia = if ends_in_line_comment(head) && !rest.starts_with('\n') {
        format!("{head}\n{rest}")
    } else {
        format!("{head}{trivia}")
    };
}

/// Drops a same-line comment at the start of `lead` (the trivia after a removed entry's `;`),
/// keeping the line break and what follows.
pub fn strip_same_line(lead: &mut String) {
    // Up to the first line break outside a `/* … */` comment, which goes whole.
    let first_line = first_free_newline(lead).unwrap_or(lead.len());
    let head = lead.get(..first_line).unwrap_or("");
    // On one line (`{ a 1; b 2; }`), the space before what follows stays; the removed entry's
    // comments on it (`/* cb */`) go, a space kept.
    if first_free_newline(lead).is_none() && !lead.contains("//") {
        if lead.contains("/*") {
            *lead = " ".to_string();
        }
        return;
    }
    let t = head.trim_start();
    if t.starts_with("//") || t.starts_with("/*") || head.trim().is_empty() {
        *lead = lead.get(first_line..).unwrap_or("").to_string();
    }
}

/// Gives the entry just appended to `dict` the comments that followed the entry before it on its
/// line (at the start of `trivia`, what comes after the dictionary's last item: its closing
/// brace's lead, or the document's tail), so they stay with that entry: a `//` comment, and
/// `/* … */` comments whole, one going on over lines too.
pub fn adopt_same_line(trivia: &mut String, dict: &mut Dict) {
    let mut end = 0;
    loop {
        let rest = trivia.get(end..).unwrap_or("");
        let skipped = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        let after = rest.get(skipped..).unwrap_or("");
        if after.starts_with("//") {
            end += skipped + after.find('\n').unwrap_or(after.len());
            break;
        }
        match after.strip_prefix("/*").and_then(|b| b.find("*/")) {
            Some(close) => end += skipped + 2 + close + 2,
            None => break,
        }
    }
    if end == 0 {
        return;
    }
    let Some(last) = dict.items.last_mut() else { return };
    let head = trivia.get(..end).unwrap_or("").to_string();
    let lead = &mut last.key_tok_mut().lead;
    // After a `// …` comment the entry goes on a line of its own, never into the comment.
    *lead = if ends_in_line_comment(&head) && !lead.contains('\n') {
        format!("{head}\n{}", lead.trim_start())
    } else {
        format!("{head}{lead}")
    };
    *trivia = trivia.get(end..).unwrap_or("").to_string();
}

fn empty_dict() -> Node {
    Node::Dict { open: Tok::new("", "{"), body: Dict::default(), close: Tok::new("", "}") }
}

/// A new entry formatted like the last sibling (same blank-line spacing and indentation).
fn new_entry(dict: &Dict, key: &str, mut nodes: Vec<Node>, indent: &str, inline: bool) -> Entry {
    let last = dict.items.iter().rev().find(|i| !i.key_tok().text.is_empty());
    // A dictionary written on one line (`{ name soil; rho 1600; }`) gets it on that line.
    // (A file's first entry has no lead at all: not that.)
    let on_line = |i: &Item| {
        let lead = &i.key_tok().lead;
        !lead.is_empty() && !lead.contains('\n') && !lead.contains("//")
    };
    let one_line = last.is_some() && dict.items.iter().filter(|i| !i.key_tok().text.is_empty()).all(on_line);
    // Indented like the last entry that starts a line (not one after another on its line).
    let line_start = dict.items.iter().rev().find(|i| !i.key_tok().text.is_empty() && i.key_tok().lead.contains('\n'));
    let (newlines, indent) = match line_start.or(last) {
        Some(last) => {
            let ws = trailing_ws(&last.key_tok().lead);
            let nl = ws.matches('\n').count().clamp(1, 2);
            (nl, trailing_indent(ws).to_string())
        }
        None => (1, indent.to_string()),
    };
    let key_text = if needs_quotes(key) { format!("\"{key}\"") } else { key.to_string() };
    let dict_value = matches!(nodes.as_slice(), [Node::Dict { .. }]);
    let is_dict = dict_value && !inline;
    if is_dict {
        if let Some(n) = nodes.first_mut() {
            layout_dict(n, &indent);
        }
    } else if let Some(n) = nodes.first_mut() {
        n.first_tok_mut().lead = if one_line { " ".to_string() } else { sibling_pad(dict, key_text.len()) };
    }
    let lead = if one_line { " ".to_string() } else { format!("{}{indent}", "\n".repeat(newlines)) };
    Entry { key: Tok::new(lead, key_text), value: nodes, end: (!dict_value).then(|| Tok::new("", ";")) }
}

/// The space before a new value: to the column the values of the entries before it are at
/// (`rho     1600;`), else OpenFOAM's usual 16.
fn sibling_pad(dict: &Dict, key_len: usize) -> String {
    let last = dict.entries().filter(|e| e.dict().is_none()).last();
    match last.map(|e| (e.key.text.len(), e.value.first().map_or("", first_lead))) {
        // One space (`rho 1;`) is no column: one space.
        Some((_, " ")) => " ".to_string(),
        Some((k, lead)) if !lead.is_empty() && lead.bytes().all(|b| b == b' ') => pad(key_len, k + lead.len()),
        _ => pad(key_len, VALUE_COLUMN),
    }
}

fn push(dict: &mut Dict, entry: Entry) {
    dict.items.push(Item::Entry(entry));
}

/// Keys that read back as something else unless quoted: white space and punctuation, a
/// leading `(`, which would start a list, and the signs of a pattern (`|`, `*`): only a quoted
/// key is a pattern to OpenFOAM (`"(U|k)"`, `"k.*"`, `"h|e"`). A scheme's key (`div(phi,U)`)
/// stays as it is.
pub(crate) fn needs_quotes(key: &str) -> bool {
    key.is_empty()
        || key.starts_with('(')
        || key
            .bytes()
            .any(|b| b.is_ascii_whitespace() || matches!(b, b'{' | b'}' | b';' | b'"' | b'[' | b']' | b'|' | b'*'))
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
