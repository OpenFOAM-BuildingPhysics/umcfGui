//! Reading command parameters, and turning JSON values into OpenFOAM text.

use serde_json::{Map, Value};
use umcf_foam_dict::Document;

use crate::Error;

/// The most nested arrays a value may have (`((0 0 0) (1 1 1))` is two).
const MAX_NESTING: usize = 32;

/// A command's parameters: a JSON object, or `null` for none.
pub(crate) struct Params<'a> {
    id: &'static str,
    map: Option<&'a Map<String, Value>>,
}

impl<'a> Params<'a> {
    /// The parameters of command `id`, which takes the names in `allowed`.
    pub(crate) fn new(id: &'static str, value: &'a Value, allowed: &[&str]) -> Result<Self, Error> {
        let p = Params::any(id, value)?;
        if let Some(bad) = p.map.into_iter().flat_map(Map::keys).find(|k| !allowed.contains(&k.as_str())) {
            let known = if allowed.is_empty() { "none".to_string() } else { allowed.join(", ") };
            return Err(Error::Params(format!("{id}: unknown parameter `{bad}` (it takes: {known})")));
        }
        Ok(p)
    }

    /// Parameters with any names (a command whose names are the entries it sets).
    pub(crate) fn any(id: &'static str, value: &'a Value) -> Result<Self, Error> {
        match value {
            Value::Null => Ok(Params { id, map: None }),
            Value::Object(m) => Ok(Params { id, map: Some(m) }),
            _ => Err(Error::Params(format!("{id}: parameters are a JSON object"))),
        }
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = (&'a String, &'a Value)> {
        self.map.into_iter().flat_map(Map::iter)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.map.is_none_or(Map::is_empty)
    }

    fn get(&self, key: &str) -> Option<&'a Value> {
        self.map.and_then(|m| m.get(key))
    }

    fn missing(&self, key: &str) -> Error {
        Error::Params(format!("{}: `{key}` is required", self.id))
    }

    pub(crate) fn str(&self, key: &str) -> Result<Option<&'a str>, Error> {
        match self.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(Error::Params(format!("{}: `{key}` is a string", self.id))),
        }
    }

    pub(crate) fn required_str(&self, key: &str) -> Result<&'a str, Error> {
        self.str(key)?.ok_or_else(|| self.missing(key))
    }

    pub(crate) fn bool(&self, key: &str) -> Result<Option<bool>, Error> {
        match self.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(Error::Params(format!("{}: `{key}` is true or false", self.id))),
        }
    }

    /// A finite number.
    pub(crate) fn number(&self, key: &str) -> Result<Option<f64>, Error> {
        match self.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(n)) => match n.as_f64() {
                Some(x) if x.is_finite() => Ok(Some(x)),
                _ => Err(Error::Params(format!("{}: `{key}` is a finite number", self.id))),
            },
            Some(_) => Err(Error::Params(format!("{}: `{key}` is a number", self.id))),
        }
    }

    pub(crate) fn required(&self, key: &str) -> Result<&'a Value, Error> {
        match self.get(key) {
            None | Some(Value::Null) => Err(self.missing(key)),
            Some(v) => Ok(v),
        }
    }

    /// An entry path: `"a/b/c"` or `["a", "b", "c"]`; each key checked by [`key`].
    pub(crate) fn path(&self, key: &str) -> Result<Vec<&'a str>, Error> {
        let parts: Vec<&str> = match self.required(key)? {
            Value::String(s) => s.split('/').collect(),
            Value::Array(a) => a
                .iter()
                .map(|v| v.as_str().ok_or_else(|| Error::Params(format!("{}: `{key}` lists strings", self.id))))
                .collect::<Result<_, _>>()?,
            _ => return Err(Error::Params(format!("{}: `{key}` is \"a/b\" or [\"a\", \"b\"]", self.id))),
        };
        for p in &parts {
            self::key(self.id, p)?;
        }
        Ok(parts)
    }
}

/// A dictionary key: a plain OpenFOAM word (`endTime`, `div(phi,U)`), never a directive
/// (`#include`) or a macro (`$x`). It is checked by writing an entry with it and reading that
/// back: the key must come back as the one entry, unchanged, so it can't end the entry, open
/// another one or leave a bracket open.
pub(crate) fn key<'k>(id: &str, k: &'k str) -> Result<&'k str, Error> {
    let bad = || Error::Params(format!("{id}: `{k}` is not a dictionary key"));
    if k.is_empty()
        || k.starts_with(['#', '$'])
        || k.chars().any(|c| c.is_whitespace() || c.is_control() || "\"';{}[]/\\".contains(c))
    {
        return Err(bad());
    }
    // Brackets balance (OpenFOAM reads `a(b` with only a warning).
    let mut depth = 0usize;
    for c in k.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.checked_sub(1).ok_or_else(bad)?,
            _ => {}
        }
    }
    if depth != 0 {
        return Err(bad());
    }
    let mut doc = Document::parse("").map_err(|_| bad())?;
    doc.set(&[k], "1").map_err(|_| bad())?;
    let back = Document::parse(&doc.to_string()).map_err(|_| bad())?;
    let one = back.root.entries().count() == 1 && back.root.problems().is_empty();
    if one && back.value(&[k]).as_deref() == Some("1") { Ok(k) } else { Err(bad()) }
}

/// OpenFOAM text for a JSON value: a number as written, a string as OpenFOAM syntax
/// (`"Gauss linear"`, `uniform 300`), `true`/`false`, and an array as a list `(a b c)`.
pub(crate) fn foam_text(id: &str, v: &Value) -> Result<String, Error> {
    fn go(v: &Value, depth: usize, out: &mut String) -> Result<(), String> {
        match v {
            Value::Number(n) => out.push_str(&n.to_string()),
            Value::String(s) if s.trim().is_empty() => return Err("an empty value".to_string()),
            Value::String(s) => out.push_str(s),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Array(_) if depth >= MAX_NESTING => {
                return Err(format!("lists nested more than {MAX_NESTING} deep"));
            }
            Value::Array(a) => {
                out.push('(');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    go(x, depth + 1, out)?;
                }
                out.push(')');
            }
            Value::Null => return Err("null (to remove an entry, use dict.remove)".to_string()),
            Value::Object(_) => {
                return Err("an object (give a dictionary as OpenFOAM text: \"{ type fixedValue; }\")".to_string());
            }
        }
        Ok(())
    }
    let mut out = String::new();
    go(v, 0, &mut out).map_err(|e| Error::Params(format!("{id}: a value can't be {e}")))?;
    Ok(out)
}
