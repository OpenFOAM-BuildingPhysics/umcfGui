//! `cargo xtask catalog [--check]`: the solver catalog, read from the `solver/` submodule.
//!
//! The scanner reads two revisions straight from git, without checking anything out: the pinned
//! commit (the OpenFOAM-12 port) and the tag `of-org_v8.0`. In each it finds:
//!
//! - **families**: base classes with run-time selection, their selector keyword (what their
//!   `New` function, or the solver, looks up) and the dictionary file it is read from;
//! - **types**: everything registered with `addToRunTimeSelectionTable` (directly or through a
//!   helper macro such as `addToGrassRunTimeSelectionTables`), `makePatchTypeField` (boundary
//!   conditions) or `makeRASModel`, by the name users write (`TypeName("…")`);
//! - **keywords**: every `lookup`, `lookupOrDefault` (with its default), `found`,
//!   `readIfPresent` and `subDict` with a literal key, per type, per family and per application.
//!
//! It writes `crates/schema/src/catalog/generated.rs` and `docs/solver-coverage.md`; `--check`
//! fails if either differs from what the pinned solver gives.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use regex::Regex;

pub const GENERATED_RS: &str = "crates/schema/src/catalog/generated.rs";
pub const COVERAGE_MD: &str = "docs/solver-coverage.md";
/// The OpenFOAM-8 revision of the solver.
pub const V8_REV: &str = "of-org_v8.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ver {
    V8,
    V12,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    Required,
    Default(String),
    Optional,
    SubDict,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Kw {
    pub name: String,
    pub access: Access,
    /// What the key is looked up in (`dict`, `coeffs_`, `controlDict()`), as written.
    pub receiver: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Model,
    ScalarBc,
    VectorBc,
    Ras,
}

/// Where a field is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Instance {
    /// The time the run starts from (`runTime.name()`, `timeName()`).
    Time,
    /// `0`, whatever time the run starts from.
    Zero,
    /// `constant/` (of the region).
    Constant,
    /// The mesh's directory (`facesInstance()`) or anything else: written by the utilities.
    Other,
}

/// Who reads a field.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Owner {
    /// The solver for every region of a kind: `fluid`, `solid`, `vegetation`.
    Region(String),
    /// A family's base class, for every type.
    Family(String),
    /// One type: `(family, name)`.
    Type(String, String),
    /// A utility or a library file no type owns.
    Other(String),
}

/// A file read through an `IOobject` with `MUST_READ` or `READ_IF_PRESENT`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FieldRead {
    pub owner: Owner,
    pub name: String,
    pub instance: Instance,
    pub required: bool,
    pub file: String,
}

#[derive(Debug, Clone, Default)]
struct FamilyScan {
    source: String,
    selector: Option<String>,
    dict_file: Option<String>,
    keywords: BTreeSet<Kw>,
}

#[derive(Debug, Clone)]
struct TypeScan {
    class: String,
    kind: Kind,
    source: String,
    keywords: BTreeSet<Kw>,
}

#[derive(Debug, Default)]
pub struct Scan {
    families: BTreeMap<String, FamilyScan>,
    /// (family, type name) → type.
    types: BTreeMap<(String, String), TypeScan>,
    /// (application, file) → keywords.
    apps: BTreeMap<(String, String), BTreeSet<Kw>>,
    fields: BTreeSet<FieldRead>,
    /// (region kind, class) for each `<class>::New(…)` the solver calls for a region kind.
    builds: BTreeSet<(String, String)>,
    /// (family, type, keyword, receiver): required keywords a type reads whenever it is built.
    startup: BTreeSet<(String, String, String, String)>,
}

// ---------------------------------------------------------------------------------------------
// Reading the source

/// C/C++ source without comments and line continuations; string and character literals kept.
pub fn strip(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], b.get(i + 1).copied()) {
            (b'/', Some(b'/')) => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            (b'/', Some(b'*')) => {
                i += 2;
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i += 2;
                out.push(' ');
            }
            (b'\\', Some(b'\n')) => i += 2,
            (q @ (b'"' | b'\''), _) => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != q && b[i] != b'\n' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
                out.push_str(src.get(start..i.min(b.len())).unwrap_or(""));
            }
            _ => {
                let start = i;
                i += 1;
                while i < b.len() && !matches!(b[i], b'/' | b'\\' | b'"' | b'\'') {
                    i += 1;
                }
                out.push_str(src.get(start..i).unwrap_or(""));
            }
        }
    }
    out
}

pub fn git(solver: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(solver).args(args).output().map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("git {}: {e}", args.join(" ")))
}

/// Every `.C`/`.H` file of `rev`, comments stripped, through one `git cat-file --batch`.
pub fn load(solver: &Path, rev: &str) -> Result<BTreeMap<String, String>, String> {
    let paths: Vec<String> = git(solver, &["ls-tree", "-r", "--name-only", rev])?
        .lines()
        .filter(|p| p.ends_with(".C") || p.ends_with(".H"))
        .map(str::to_string)
        .collect();
    let mut child = Command::new("git")
        .arg("-C")
        .arg(solver)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git cat-file: {e}"))?;
    let requests: String = paths.iter().map(|p| format!("{rev}:{p}\n")).collect();
    let mut stdin = child.stdin.take().ok_or("git cat-file: no stdin")?;
    let writer = std::thread::spawn(move || stdin.write_all(requests.as_bytes()));
    let out = child.wait_with_output().map_err(|e| format!("git cat-file: {e}"))?;
    writer.join().map_err(|_| "git cat-file: writer panicked")?.map_err(|e| format!("git cat-file: {e}"))?;
    let mut files = BTreeMap::new();
    let mut rest = out.stdout.as_slice();
    for p in &paths {
        let nl = rest.iter().position(|b| *b == b'\n').ok_or("git cat-file: truncated output")?;
        let header = String::from_utf8_lossy(&rest[..nl]).to_string();
        let size: usize = header
            .split_whitespace()
            .nth(2)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("git cat-file: unexpected header `{header}` for {p}"))?;
        let body = rest.get(nl + 1..nl + 1 + size).ok_or("git cat-file: truncated output")?;
        files.insert(p.clone(), strip(&String::from_utf8_lossy(body)));
        rest = rest.get(nl + 2 + size..).unwrap_or(&[]);
    }
    Ok(files)
}

// ---------------------------------------------------------------------------------------------
// Scanning

/// Every pattern the scanner uses, compiled once.
pub struct Patterns {
    call: Regex,
    class: Regex,
    typename: Regex,
    define: Regex,
    add: Regex,
    patch: Regex,
    ras: Regex,
    props: Regex,
    radiation: Regex,
    ioobject: Regex,
    new_call: Regex,
    cstr_call: Regex,
}

impl Patterns {
    pub fn new() -> Result<Patterns, String> {
        let re = |p: &str| Regex::new(p).map_err(|e| format!("catalog pattern: {e}"));
        Ok(Patterns {
            call: re(&format!(r"(\w+(?:\s*\(\s*\w*\s*\))?)?\s*(?:\.|->)\s*({METHODS})\b\s*(?:<[^;(){{}}]*>)?\s*\("))?,
            class: re(r"\bclass\s+(\w+)\s*(?::[^;{]*)?\{")?,
            typename: re(r#"\bTypeName\s*\(\s*"([^"]+)"\s*\)"#)?,
            define: re(r"#define\s+(\w+)\s*\(\s*(\w+)\s*\)([^\n]*)")?,
            add: re(r"addToRunTimeSelectionTable\s*\(\s*(\w+)\s*,\s*(\w+)\s*,\s*\w+\s*\)")?,
            patch: re(r"makePatchTypeField\s*\(\s*(\w+)\s*,\s*(\w+)\s*\)")?,
            ras: re(r"makeRASModel\s*\(\s*(\w+)\s*\)")?,
            props: re(r#""(\w+Properties)""#)?,
            // OpenFOAM's macro for radiation models: a family whose base class is OpenFOAM's.
            radiation: re(r"addToRadiationRunTimeSelectionTables\s*\(\s*(\w+)\s*\)")?,
            ioobject: re(r"\bIOobject\s*\(")?,
            new_call: re(r"\b(\w+)\s*::\s*New\s*\(")?,
            cstr_call: re(r"\bcstrIter\s*(?:\(\s*\)|->\s*second)\s*\(")?,
        })
    }
}

const METHODS: &str =
    r"lookup|lookupOrDefault|lookupOrAddDefault|subDict|subOrEmptyDict|optionalSubDict|found|readIfPresent|isDict";

/// The argument text of a call whose `(` is at `open`: everything up to the matching `)`.
fn call_args(text: &str, open: usize) -> Option<&str> {
    let mut depth = 0usize;
    for (k, c) in text.get(open..)?.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return text.get(open + 1..open + k);
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits call arguments at top-level commas.
fn split_args(args: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth, mut in_str) = (Vec::new(), String::new(), 0i32, false);
    let mut prev = ' ';
    for c in args.chars() {
        let after = std::mem::replace(&mut prev, c);
        match c {
            '"' => in_str = !in_str,
            '(' | '<' if !in_str => depth += 1,
            // `this->x`: an arrow, not a closing template bracket.
            '>' if !in_str && after == '-' => {}
            ')' | '>' if !in_str => depth -= 1,
            ',' if !in_str && depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Keywords read with a literal key in `text`.
pub fn keywords(pat: &Patterns, text: &str) -> BTreeSet<Kw> {
    keywords_at(pat, text).into_iter().map(|(_, k)| k).collect()
}

/// [`keywords`] with the offset of each lookup, in source order.
fn keywords_at(pat: &Patterns, text: &str) -> Vec<(usize, Kw)> {
    let mut out = Vec::new();
    for m in pat.call.captures_iter(text) {
        let (Some(all), Some(method)) = (m.get(0), m.get(2)) else { continue };
        let Some(args) = call_args(text, all.end() - 1) else { continue };
        let args = split_args(args);
        let Some(key) = args.first().and_then(|a| a.strip_prefix('"')).and_then(|a| a.strip_suffix('"')) else {
            continue;
        };
        let access = match method.as_str() {
            "lookup" => Access::Required,
            "lookupOrDefault" | "lookupOrAddDefault" => Access::Default(
                args.get(1).map(|d| d.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default(),
            ),
            "subDict" => Access::SubDict,
            _ => Access::Optional,
        };
        let receiver = m.get(1).map(|r| r.as_str().split_whitespace().collect::<String>()).unwrap_or_default();
        out.push((all.start(), Kw { name: key.to_string(), access, receiver }));
    }
    out
}

/// The files `text` reads through an `IOobject(…)` with a literal name (`"U"`, or
/// `groupName("k")`): name, where from, and whether it must be there. Dictionaries
/// (`IOdictionary`, `…Properties`, `…Dict`) are keywords' business, not fields. A named
/// `IOobject io(…)` is not read here: it is the form used to test for a file first
/// (`io.headerOk()`), so what it names is optional.
pub fn field_reads(pat: &Patterns, text: &str) -> Vec<(String, Instance, bool)> {
    field_reads_at(pat, text).into_iter().map(|(_, r)| r).collect()
}

/// [`field_reads`] with the offset of each `IOobject`.
fn field_reads_at(pat: &Patterns, text: &str) -> Vec<(usize, (String, Instance, bool))> {
    let mut out = Vec::new();
    for m in pat.ioobject.find_iter(text) {
        let Some(raw) = call_args(text, m.end() - 1) else { continue };
        let args = split_args(raw);
        let required = if args.iter().any(|a| a.contains("MUST_READ")) {
            true
        } else if args.iter().any(|a| a.contains("READ_IF_PRESENT")) {
            false
        } else {
            continue;
        };
        // The declaration this `IOobject` is in: back to the previous `;`, `{`, `}` or `,` (the
        // member before it in an initialiser list).
        let before = text.get(m.start().saturating_sub(120)..m.start()).unwrap_or("");
        let before = before.rfind([';', '{', '}', ',']).map_or(before, |k| before.get(k + 1..).unwrap_or(""));
        if before.contains("IOdictionary") || before.contains("typeIOobject") {
            continue;
        }
        let Some(first) = args.first() else { continue };
        let literal =
            if first.starts_with('"') || first.contains("groupName") { first.split('"').nth(1) } else { None };
        let Some(name) = literal.filter(|n| !n.is_empty()) else { continue };
        if name.ends_with("Properties") || name.ends_with("Dict") {
            continue;
        }
        let inst = args.get(1).map(String::as_str).unwrap_or("");
        // `IOobject(name, instance, local, registry, …)`: under a subdirectory of the instance.
        let local = args.get(2).is_some_and(|a| a.starts_with('"'));
        let instance = if local {
            Instance::Other
        } else if inst == "\"0\"" {
            Instance::Zero
        } else if inst.contains("caseConstant") {
            Instance::Other
        } else if inst.contains("constant()") {
            Instance::Constant
        } else if inst.contains("name()") || inst.contains("timeName()") {
            Instance::Time
        } else {
            Instance::Other
        };
        out.push((m.start(), (name.to_string(), instance, required)));
    }
    out
}

/// For a match ending at the `(` of `…::name(`: the number of parameters, when this is a
/// definition: its parameter list is followed by the body `{`, an initialiser list `:` or
/// `const`, never by what follows a call (`;`, `.y`, `+ g`).
fn definition_params(text: &str, open: usize) -> Option<usize> {
    let args = call_args(text, open)?;
    let after = text.get(open + args.len() + 2..)?.trim_start();
    let body = after.starts_with('{')
        || (after.starts_with(':') && !after.starts_with("::"))
        || Regex::new(r"^const\b").is_ok_and(|r| r.is_match(after));
    body.then(|| split_args(args).len())
}

/// A constructor of a class defined in a file: its byte span (`Foam::…::class::class(`, or
/// `class<Base>::class(`, up to the `}` at the start of a line that ends the body) and its
/// number of parameters.
struct Ctor {
    span: std::ops::Range<usize>,
    params: usize,
}

fn constructors(text: &str, class: &str) -> Vec<Ctor> {
    let c = regex::escape(class);
    // At the start of a line (after the return type and scope), never inside a string.
    let Ok(re) = Regex::new(&format!(r#"(?m)^[^"\n;{{}}=?]*?\b{c}\s*(?:<[^<>;{{}}()]*>)?\s*::\s*{c}\s*\("#)) else {
        return Vec::new();
    };
    re.find_iter(text)
        .filter_map(|m| {
            let params = definition_params(text, m.end() - 1)?;
            let end = text.get(m.end()..).and_then(|rest| rest.find("\n}")).map_or(text.len(), |k| m.end() + k);
            Some(Ctor { span: m.start()..end, params })
        })
        .collect()
}

/// Reads of a model's file that the solver can count on: those every constructor it uses
/// makes. With `arity` (the solver builds the family with that many arguments, `New(T)`),
/// only the constructors with that many parameters are used: `directAndDiffuse` reads `qs`
/// when built from a field, `Qs` when built from a dictionary, and the solver builds it from a
/// field. A read outside every constructor (a member function) is not required: it may never
/// run. A file without a constructor of the class keeps its reads as they are.
fn model_reads(pat: &Patterns, text: &str, class: &str, arity: Option<usize>) -> Vec<(String, Instance, bool)> {
    let ctors = constructors(text, class);
    let reads = field_reads_at(pat, text);
    if ctors.is_empty() {
        return reads.into_iter().map(|(_, r)| r).collect();
    }
    let used = used_ctors(&ctors, arity);
    let in_ctor = |at: usize, c: &Ctor| c.span.contains(&at);
    reads
        .iter()
        .map(|(at, (name, instance, required))| {
            // Required: this read must be in a used constructor, and every used constructor
            // must read the file with MUST_READ.
            let here = used.iter().any(|c| in_ctor(*at, c));
            let every = used
                .iter()
                .all(|c| reads.iter().any(|(at2, (n, i, r))| *r && n == name && i == instance && in_ctor(*at2, c)));
            (name.clone(), *instance, *required && here && every)
        })
        .collect()
}

/// The constructors the solver builds with: those with `arity` parameters when there are any,
/// else all of them.
fn used_ctors(ctors: &[Ctor], arity: Option<usize>) -> Vec<&Ctor> {
    let matching = |c: &&Ctor| arity.is_some_and(|n| c.params == n);
    if ctors.iter().any(|c| matching(&c)) { ctors.iter().filter(matching).collect() } else { ctors.iter().collect() }
}

/// What runs whenever the solver builds the model: each used constructor, plus the member
/// functions of the class (`initialise()`) that every used constructor calls. One list of
/// spans per used constructor.
fn startup(text: &str, class: &str, arity: Option<usize>) -> Vec<Vec<std::ops::Range<usize>>> {
    let ctors = constructors(text, class);
    let used = used_ctors(&ctors, arity);
    let c = regex::escape(class);
    // A definition starts a line (`void Foam::…::m::initialise()`); `"m::initialise() "` in a
    // message is not one.
    let Ok(def) = Regex::new(&format!(r#"(?m)^[^"\n;{{}}=?]*?\b{c}\s*(?:<[^<>;{{}}()]*>)?\s*::\s*(\w+)\s*\("#)) else {
        return Vec::new();
    };
    let functions: Vec<(String, usize, std::ops::Range<usize>)> = def
        .captures_iter(text)
        .filter_map(|m| {
            let (all, name) = (m.get(0)?, m.get(1)?);
            if name.as_str() == class {
                return None;
            }
            let params = definition_params(text, all.end() - 1)?;
            let end = text.get(all.end()..).and_then(|r| r.find("\n}")).map_or(text.len(), |k| all.end() + k);
            Some((name.as_str().to_string(), params, all.start()..end))
        })
        .collect();
    used.iter()
        .map(|ctor| {
            let body = text.get(ctor.span.clone()).unwrap_or("");
            let mut spans = vec![ctor.span.clone()];
            for (name, params, span) in &functions {
                // A call of this object's function, not another object's (`dict.read(`), with
                // as many arguments as this overload takes, that runs whenever the constructor
                // does (not `if (flag) initialise();`).
                let Ok(call) = Regex::new(&format!(r"(?:^|[^\w.>]){}\s*\(", regex::escape(name))) else { continue };
                let called = call.find_iter(body).any(|c| {
                    call_args(body, c.end() - 1).is_some_and(|a| split_args(a).len() == *params)
                        && unconditional(text, &ctor.span, ctor.span.start + c.end() - 1)
                });
                if called {
                    spans.push(span.clone());
                }
            }
            spans
        })
        .collect()
}

/// Whether the code at `at` runs every time the function starting at `span.start` runs: not
/// after a `return`, not in a nested block (`if`, a loop), and not in a statement with a
/// condition (`if (…) x;`, `else x;`, `forAll(…) x;`, `a ? b : c`, `&&`, `||`). A block under
/// `if (Pstream::master())` counts as taken: it runs on one process in every run.
fn unconditional(text: &str, span: &std::ops::Range<usize>, at: usize) -> bool {
    let Some(code) = text.get(span.start..at) else { return false };
    if Regex::new(r"\breturn\b").is_ok_and(|r| r.is_match(code)) {
        return false;
    }
    let Ok(master) = Regex::new(r"^\s*if\s*\(\s*Pstream\s*::\s*master\s*\(\s*\)\s*\)\s*$") else { return false };
    // Open braces: whether each one counts (false for a master block).
    let mut open: Vec<bool> = Vec::new();
    let mut from = 0;
    for (i, c) in code.char_indices() {
        match c {
            '{' => {
                let header = code.get(from..i).unwrap_or("");
                open.push(!master.is_match(header));
                from = i + 1;
            }
            '}' => {
                open.pop();
                from = i + 1;
            }
            ';' => from = i + 1,
            _ => {}
        }
    }
    if open.iter().filter(|counts| **counts).count() > 1 {
        return false;
    }
    let statement = code.get(from..).unwrap_or("");
    !(statement.contains('?')
        || statement.contains("&&")
        || statement.contains("||")
        || Regex::new(r"\b(if|while|for|switch|forAll\w*|else|do|case)\b").is_ok_and(|r| r.is_match(statement)))
}

/// Required keywords (`lookup`) the model reads every time the solver builds it: in every used
/// constructor, or a function each of them calls, unconditionally.
fn startup_keywords(pat: &Patterns, text: &str, class: &str, arity: Option<usize>) -> BTreeSet<Kw> {
    let runs = startup(text, class, arity);
    if runs.is_empty() {
        return BTreeSet::new();
    }
    let kws = keywords_at(pat, text);
    let always = |at: usize, spans: &[std::ops::Range<usize>]| {
        spans.iter().any(|s| s.contains(&at) && unconditional(text, s, at))
    };
    kws.iter()
        .filter(|(_, k)| k.access == Access::Required)
        .filter(|(_, k)| runs.iter().all(|spans| kws.iter().any(|(at, k2)| k2 == k && always(*at, spans))))
        .map(|(_, k)| k.clone())
        .collect()
}

/// The region kind a solver file sets up: `fluid/` → `fluid`, `solid/`, `veg/` → `vegetation`.
fn region_of(path: &str) -> Option<&'static str> {
    match path.split('/').next() {
        Some("fluid") => Some("fluid"),
        Some("solid") => Some("solid"),
        Some("veg") => Some("vegetation"),
        _ => None,
    }
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// Which application a file outside the model libraries belongs to.
fn app_of(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("_APP/") {
        return rest.split('/').next().unwrap_or(rest).to_string();
    }
    if let Some(rest) = path.strip_prefix("_LIB/") {
        let mut parts = rest.split('/');
        return format!("lib/{}", parts.next().unwrap_or(""));
    }
    "urbanMicroclimateFoam".to_string()
}

pub fn scan(pat: &Patterns, files: &BTreeMap<String, String>) -> Result<Scan, String> {
    let (class_re, typename_re, define_re, add_re, patch_re, ras_re, props_re) =
        (&pat.class, &pat.typename, &pat.define, &pat.add, &pat.patch, &pat.ras, &pat.props);

    // class → (TypeName, header)
    let mut classes: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (path, text) in files.iter().filter(|(p, _)| p.ends_with(".H")) {
        let decls: Vec<(usize, String)> = class_re
            .captures_iter(text)
            .filter_map(|c| Some((c.get(0)?.start(), c.get(1)?.as_str().to_string())))
            .collect();
        for t in typename_re.captures_iter(text) {
            let (Some(at), Some(name)) = (t.get(0), t.get(1)) else { continue };
            if let Some((_, class)) = decls.iter().rev().find(|(pos, _)| *pos < at.start()) {
                classes.entry(class.clone()).or_insert_with(|| (name.as_str().to_string(), path.clone()));
            }
        }
    }
    // helper macro → base class
    let mut macros: BTreeMap<String, String> = BTreeMap::new();
    for text in files.values() {
        for d in define_re.captures_iter(text) {
            let (Some(name), Some(param), Some(body)) = (d.get(1), d.get(2), d.get(3)) else { continue };
            for a in add_re.captures_iter(body.as_str()) {
                if a.get(2).map(|m| m.as_str()) == Some(param.as_str())
                    && let Some(base) = a.get(1)
                {
                    macros.insert(name.as_str().to_string(), base.as_str().to_string());
                }
            }
        }
    }
    let macro_re = if macros.is_empty() {
        None
    } else {
        let names: Vec<String> = macros.keys().map(|k| regex::escape(k)).collect();
        Some(Regex::new(&format!(r"\b({})\s*\(\s*(\w+)\s*\)", names.join("|"))).map_err(|e| e.to_string())?)
    };

    let mut regs: Vec<(String, String, Kind, String)> = Vec::new(); // (family, class, kind, file)
    for (path, text) in files {
        // Registrations, outside #define lines.
        let body: String =
            text.lines().filter(|l| !l.trim_start().starts_with("#define")).collect::<Vec<_>>().join("\n");
        for a in add_re.captures_iter(&body) {
            regs.push((a[1].to_string(), a[2].to_string(), Kind::Model, path.clone()));
        }
        if let Some(mre) = &macro_re {
            for m in mre.captures_iter(&body) {
                if let Some(base) = macros.get(&m[1]) {
                    regs.push((base.clone(), m[2].to_string(), Kind::Model, path.clone()));
                }
            }
        }
        for p in patch_re.captures_iter(&body) {
            let kind = if &p[1] == "fvPatchVectorField" { Kind::VectorBc } else { Kind::ScalarBc };
            regs.push((p[1].to_string(), p[2].to_string(), kind, path.clone()));
        }
        for r in ras_re.captures_iter(&body) {
            regs.push(("RASModel".to_string(), r[1].to_string(), Kind::Ras, path.clone()));
        }
        for r in pat.radiation.captures_iter(&body) {
            regs.push(("radiationModel".to_string(), r[1].to_string(), Kind::Model, path.clone()));
        }
    }

    let mut s = Scan::default();
    let mut claimed: BTreeSet<String> = BTreeSet::new(); // directories owned by a type or family
    for (family, class, kind, reg_file) in regs {
        let (name, source) = classes.get(&class).cloned().unwrap_or_else(|| (class.clone(), reg_file.clone()));
        let dir = dir_of(&source).to_string();
        let kws: BTreeSet<Kw> =
            files.iter().filter(|(p, _)| dir_of(p) == dir).flat_map(|(_, t)| keywords(pat, t)).collect();
        if !dir.is_empty() {
            claimed.insert(dir);
        }
        s.types.insert((family.clone(), name), TypeScan { class, kind, source, keywords: kws });
        if kind == Kind::Model && !s.families.contains_key(&family) {
            let source = classes.get(&family).map(|c| c.1.clone()).unwrap_or_default();
            s.families.insert(family, FamilyScan { source, ..FamilyScan::default() });
        }
    }
    let selector_re = |base: &str| Regex::new(&format!(r#"\.\s*lookup\s*\(\s*"({})"\s*\)"#, regex::escape(base)));
    for (base, fam) in &mut s.families {
        // A base class from OpenFOAM itself (searchableSurface, …) has no directory here: it
        // must not claim the solver's top level and its keywords.
        if fam.source.is_empty() {
            continue;
        }
        let dir = dir_of(&fam.source).to_string();
        claimed.insert(dir.clone());
        fam.keywords = files.iter().filter(|(p, _)| dir_of(p) == dir).flat_map(|(_, t)| keywords(pat, t)).collect();
        // The selector: what the family's New function looks up, else wherever the solver
        // looks up a key named after the base class.
        let new_file = files.iter().find(|(p, _)| dir_of(p) == dir && p.contains("New") && p.ends_with(".C"));
        if let Some((_, text)) = new_file {
            fam.selector = keywords(pat, text).into_iter().find(|k| k.access == Access::Required).map(|k| k.name);
            fam.dict_file = props_re.captures(text).map(|c| c[1].to_string());
        }
        if fam.selector.is_none()
            && let Ok(re) = selector_re(base)
            && files.values().any(|t| re.is_match(t))
        {
            fam.selector = Some(base.clone());
        }
    }
    // Field reads, by owner: a type's or family's directory, else the region kind or file.
    // directory → (owner, C++ class)
    let mut owners: BTreeMap<String, (Owner, String)> = BTreeMap::new();
    for ((family, name), t) in &s.types {
        let dir = dir_of(&t.source);
        if !dir.is_empty() {
            owners
                .entry(dir.to_string())
                .or_insert_with(|| (Owner::Type(family.clone(), name.clone()), t.class.clone()));
        }
    }
    for (base, f) in &s.families {
        let dir = dir_of(&f.source);
        if !dir.is_empty() {
            owners.entry(dir.to_string()).or_insert_with(|| (Owner::Family(base.clone()), base.clone()));
        }
    }
    // How many arguments the solver builds each class with (`solarLoadModel::New(T)`: one).
    let mut arities: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    for (_, text) in files.iter().filter(|(p, _)| region_of(p).is_some()) {
        for c in pat.new_call.captures_iter(text) {
            let (Some(all), Some(class)) = (c.get(0), c.get(1)) else { continue };
            let n = call_args(text, all.end() - 1).map_or(0, |a| split_args(a).len());
            arities.entry(class.as_str().to_string()).or_default().insert(n);
        }
    }
    // …and whether the family's `New` with that many parameters passes the same arguments on
    // to a constructor (`cstrIter()(T)`, OpenFOAM-8 `cstrIter->second(T)`): the count picks
    // constructors only then.
    let passes_on = |family: &str, n: usize| -> bool {
        let Some(f) = s.families.get(family) else { return false };
        let dir = dir_of(&f.source);
        let Ok(new_def) = Regex::new(&format!(r"\b{}\s*::\s*New\s*\(", regex::escape(family))) else { return false };
        // Every `New` with n parameters (the call site doesn't say which one), and at least one.
        let mut overloads = files
            .iter()
            .filter(|(p, _)| !dir.is_empty() && dir_of(p) == dir && p.contains("New"))
            .flat_map(|(_, text)| new_def.find_iter(text).map(move |m| (text, m)))
            .filter(|(text, m)| call_args(text, m.end() - 1).map_or(0, |a| split_args(a).len()) == n)
            .peekable();
        overloads.peek().is_some()
            && overloads.all(|(text, m)| {
                let body = text
                    .get(m.end()..)
                    .map_or("", |rest| rest.find("\n}").map_or(rest, |k| rest.get(..k).unwrap_or(rest)));
                pat.cstr_call.is_match(body)
                    && pat
                        .cstr_call
                        .find_iter(body)
                        .all(|c| call_args(body, c.end() - 1).map_or(0, |a| split_args(a).len()) == n)
            })
    };
    let arity = |family: &str| match arities.get(family) {
        Some(set) if set.len() == 1 => set.iter().next().copied().filter(|n| passes_on(family, *n)),
        _ => None,
    };
    // Required keywords each model type reads whenever the solver builds it.
    for ((family, name), t) in &s.types {
        let dir = dir_of(&t.source);
        if t.kind != Kind::Model || dir.is_empty() {
            continue;
        }
        // The whole directory at once: constructors and the functions they call may be split
        // over several files.
        let text: String = files
            .iter()
            .filter(|(p, _)| dir_of(p) == dir && p.ends_with(".C"))
            .map(|(_, t)| format!("{t}\n"))
            .collect();
        for k in startup_keywords(pat, &text, &t.class, arity(family)) {
            s.startup.insert((family.clone(), name.clone(), k.name, k.receiver));
        }
    }
    for (path, text) in files {
        let (owner, reads) = match (owners.get(dir_of(path)), region_of(path)) {
            (Some((o, class)), _) => {
                let family = match o {
                    Owner::Type(f, _) | Owner::Family(f) => f.as_str(),
                    _ => "",
                };
                (o.clone(), model_reads(pat, text, class, arity(family)))
            }
            (None, Some(r)) => (Owner::Region(r.to_string()), field_reads(pat, text)),
            (None, None) => (Owner::Other(app_of(path)), field_reads(pat, text)),
        };
        for (name, instance, required) in reads {
            s.fields.insert(FieldRead { owner: owner.clone(), name, instance, required, file: path.clone() });
        }
        if let Some(r) = region_of(path) {
            for c in pat.new_call.captures_iter(text) {
                s.builds.insert((r.to_string(), c[1].to_string()));
            }
        }
    }
    // Application and library keywords: every file no type or family owns.
    for (path, text) in files {
        if claimed.contains(dir_of(path)) {
            continue;
        }
        let kws = keywords(pat, text);
        if !kws.is_empty() {
            s.apps.insert((app_of(path), path.clone()), kws);
        }
    }
    Ok(s)
}

// ---------------------------------------------------------------------------------------------
// Merging the two versions and writing

type Versioned<T> = BTreeMap<T, BTreeSet<Ver>>;
/// An entry with the versions it exists in and its keywords with theirs.
type Entry<T> = (T, BTreeSet<Ver>, Versioned<Kw>);

#[derive(Debug, Default)]
pub struct Merged {
    pub solver_commit: String,
    pub v8_commit: String,
    families: BTreeMap<String, Entry<FamilyScan>>,
    types: BTreeMap<(String, String), Entry<TypeScan>>,
    apps: BTreeMap<(String, String), Versioned<Kw>>,
    fields: Versioned<FieldRead>,
    builds: Versioned<(String, String)>,
    startup: Versioned<(String, String, String, String)>,
}

fn add_kws(into: &mut Versioned<Kw>, kws: &BTreeSet<Kw>, v: Ver) {
    for k in kws {
        into.entry(k.clone()).or_default().insert(v);
    }
}

/// Later versions win for the descriptive fields (source path, selector).
pub fn merge(scans: Vec<(Ver, Scan)>) -> Merged {
    let mut m = Merged::default();
    for (v, s) in scans {
        for (base, f) in s.families {
            let e = m.families.entry(base).or_insert_with(|| (f.clone(), BTreeSet::new(), BTreeMap::new()));
            add_kws(&mut e.2, &f.keywords, v);
            e.1.insert(v);
            e.0 = f;
        }
        for (key, t) in s.types {
            let e = m.types.entry(key).or_insert_with(|| (t.clone(), BTreeSet::new(), BTreeMap::new()));
            add_kws(&mut e.2, &t.keywords, v);
            e.1.insert(v);
            e.0 = t;
        }
        for (key, kws) in s.apps {
            add_kws(m.apps.entry(key).or_default(), &kws, v);
        }
        for f in s.fields {
            m.fields.entry(f).or_default().insert(v);
        }
        for b in s.builds {
            m.builds.entry(b).or_default().insert(v);
        }
        for k in s.startup {
            m.startup.entry(k).or_default().insert(v);
        }
    }
    m
}

fn vers_rs(v: &BTreeSet<Ver>) -> String {
    let names: Vec<&str> = v
        .iter()
        .map(|v| match v {
            Ver::V8 => "OfVersion::V8",
            Ver::V12 => "OfVersion::V12",
        })
        .collect();
    format!("&[{}]", names.join(", "))
}

fn kws_rs(kws: &Versioned<Kw>, indent: &str) -> String {
    if kws.is_empty() {
        return "&[]".into();
    }
    let mut s = String::from("&[\n");
    for (k, v) in kws {
        let access = match &k.access {
            Access::Required => "Access::Required".to_string(),
            Access::Default(d) => format!("Access::Default({d:?})"),
            Access::Optional => "Access::Optional".to_string(),
            Access::SubDict => "Access::SubDict".to_string(),
        };
        s.push_str(&format!(
            "{indent}    Keyword {{ name: {:?}, access: {access}, receiver: {:?}, versions: {} }},\n",
            k.name,
            k.receiver,
            vers_rs(v)
        ));
    }
    s.push_str(indent);
    s.push(']');
    s
}

pub fn render_rs(m: &Merged) -> String {
    let mut s = format!(
        "// @generated by `cargo xtask catalog` from solver/ at {} (OpenFOAM-12) and {V8_REV} ({}, OpenFOAM-8).\n// Do not edit: run `cargo xtask catalog` after moving the solver pin.\n\nuse super::*;\n\npub(super) static CATALOG: Catalog = Catalog {{\n    solver_commit: {:?},\n    v8_commit: {:?},\n    families: &[\n",
        m.solver_commit, m.v8_commit, m.solver_commit, m.v8_commit
    );
    for (base, (f, v, kws)) in &m.families {
        s.push_str(&format!(
            "        Family {{\n            base: {base:?},\n            selector: {:?},\n            dict_file: {:?},\n            source: {:?},\n            versions: {},\n            keywords: {},\n        }},\n",
            f.selector,
            f.dict_file,
            f.source,
            vers_rs(v),
            kws_rs(kws, "            ")
        ));
    }
    s.push_str("    ],\n    types: &[\n");
    for ((family, name), (t, v, kws)) in &m.types {
        let kind = match t.kind {
            Kind::Model => "TypeKind::Model",
            Kind::ScalarBc => "TypeKind::ScalarBoundaryCondition",
            Kind::VectorBc => "TypeKind::VectorBoundaryCondition",
            Kind::Ras => "TypeKind::RasModel",
        };
        s.push_str(&format!(
            "        SolverType {{\n            name: {name:?},\n            class: {:?},\n            family: {family:?},\n            kind: {kind},\n            source: {:?},\n            versions: {},\n            keywords: {},\n        }},\n",
            t.class,
            t.source,
            vers_rs(v),
            kws_rs(kws, "            ")
        ));
    }
    s.push_str("    ],\n    app_keywords: &[\n");
    for ((app, file), kws) in &m.apps {
        s.push_str(&format!(
            "        AppKeywords {{\n            app: {app:?},\n            file: {file:?},\n            keywords: {},\n        }},\n",
            kws_rs(kws, "            ")
        ));
    }
    s.push_str("    ],\n    fields: &[\n");
    for (f, v) in &m.fields {
        let owner = match &f.owner {
            Owner::Region(r) => format!("FieldOwner::Region({r:?})"),
            Owner::Family(b) => format!("FieldOwner::Family({b:?})"),
            Owner::Type(family, name) => format!("FieldOwner::Type {{ family: {family:?}, name: {name:?} }}"),
            Owner::Other(a) => format!("FieldOwner::Other({a:?})"),
        };
        let instance = match f.instance {
            Instance::Time => "FieldInstance::StartTime",
            Instance::Zero => "FieldInstance::Zero",
            Instance::Constant => "FieldInstance::Constant",
            Instance::Other => "FieldInstance::Other",
        };
        s.push_str(&format!(
            "        FieldRead {{ name: {:?}, owner: {owner}, instance: {instance}, required: {}, file: {:?}, versions: {} }},\n",
            f.name,
            f.required,
            f.file,
            vers_rs(v)
        ));
    }
    s.push_str("    ],\n    builds: &[\n");
    for ((region, class), v) in &m.builds {
        s.push_str(&format!(
            "        RegionBuild {{ region: {region:?}, class: {class:?}, versions: {} }},\n",
            vers_rs(v)
        ));
    }
    s.push_str("    ],\n    startup_keywords: &[\n");
    for ((family, type_name, name, receiver), v) in &m.startup {
        s.push_str(&format!(
            "        StartupKeyword {{ family: {family:?}, type_name: {type_name:?}, name: {name:?}, receiver: {receiver:?}, versions: {} }},\n",
            vers_rs(v)
        ));
    }
    s.push_str("    ],\n};\n");
    s
}

fn vers_md(v: &BTreeSet<Ver>) -> &'static str {
    match (v.contains(&Ver::V8), v.contains(&Ver::V12)) {
        (true, true) => "8, 12",
        (true, false) => "8 only",
        (false, true) => "12 only",
        (false, false) => "–",
    }
}

fn kw_md(name: &str, access: &Access, v: &BTreeSet<Ver>) -> String {
    let access = match access {
        Access::Required => String::new(),
        Access::Default(d) => format!(" = `{}`", d.replace('|', "\\|")),
        Access::Optional => " (optional)".into(),
        Access::SubDict => " { }".into(),
    };
    let only = match vers_md(v) {
        "8, 12" => String::new(),
        other => format!(" *({other})*"),
    };
    format!("`{name}`{access}{only}")
}

/// Keywords for a table cell: each way a keyword is read (required, each default, optional,
/// sub-dictionary) once, with the versions of every receiver it is read through combined. The
/// receiver itself is left out: the same key read from `dict` in OpenFOAM-8 and from
/// `vegetationProperties` in OpenFOAM-12 is one keyword in both versions.
fn kws_md(kws: &Versioned<Kw>) -> String {
    let mut variants: BTreeMap<(&str, &Access), BTreeSet<Ver>> = BTreeMap::new();
    for (k, v) in kws {
        variants.entry((&k.name, &k.access)).or_default().extend(v);
    }
    if variants.is_empty() {
        return "–".into();
    }
    variants.iter().map(|((name, access), v)| kw_md(name, access, v)).collect::<Vec<_>>().join(", ")
}

pub fn render_md(m: &Merged) -> String {
    let short = m.solver_commit.get(..7).unwrap_or(&m.solver_commit);
    let mut s = String::new();
    s.push_str("# Solver coverage\n\n");
    s.push_str(&format!(
        "Everything urbanMicroclimateFoam lets a case choose, and the keywords each choice reads, as\nfound in the solver source: `solver/` at `{short}` (OpenFOAM-12) and the tag `{V8_REV}`\n(OpenFOAM-8). This page is generated by `cargo xtask catalog`; CI checks that it matches the\npinned solver.\n\n"
    ));
    s.push_str("!!! info \"GUI coverage\"\n    No solver type is editable in the GUI yet. As the case editor lands, each type gets a\n    column here saying whether umcfGui exposes it.\n\n");
    let n_types = m.types.len();
    let n_bcs = m.types.values().filter(|t| matches!(t.0.kind, Kind::ScalarBc | Kind::VectorBc)).count();
    let n_kws: usize = m.types.values().map(|t| t.2.len()).sum::<usize>()
        + m.families.values().map(|f| f.2.len()).sum::<usize>()
        + m.apps.values().map(|a| a.len()).sum::<usize>();
    s.push_str(&format!(
        "**{} families, {n_types} selectable types ({n_bcs} boundary conditions), {n_kws} keyword reads.**\n\nKeywords: `name` is required, `name = default` has a default, *(optional)* is checked\nfor, `{{ }}` is a sub-dictionary; *(8 only)* / *(12 only)* mark version differences.\n\n",
        m.families.len()
    ));
    s.push_str(
        "## Model families\n\n| Family | Selected by | In | Versions | Common keywords |\n|---|---|---|---|---|\n",
    );
    for (base, (f, v, kws)) in &m.families {
        let base = if f.source.is_empty() { format!("`{base}` (OpenFOAM)") } else { format!("`{base}`") };
        s.push_str(&format!(
            "| {base} | {} | {} | {} | {} |\n",
            f.selector.as_deref().map_or("–".to_string(), |k| format!("`{k}`")),
            f.dict_file.as_deref().map_or("–".to_string(), |d| format!("`{d}`")),
            vers_md(v),
            kws_md(kws)
        ));
    }
    let section = |title: &str, filter: &dyn Fn(&(String, String), Kind) -> bool| {
        let rows: Vec<String> = m
            .types
            .iter()
            .filter(|(k, t)| filter(k, t.0.kind))
            .map(|((_, name), (t, v, kws))| {
                format!("| `{name}` | {} | {} | `{}` |\n", vers_md(v), kws_md(kws), t.source)
            })
            .collect();
        if rows.is_empty() {
            return String::new();
        }
        format!("### {title}\n\n| Type | Versions | Keywords | Source |\n|---|---|---|---|\n{}\n", rows.concat())
    };
    s.push_str("\n## Types\n\n");
    for base in m.families.keys() {
        s.push_str(&section(&format!("`{base}`"), &|k, kind| kind == Kind::Model && &k.0 == base));
    }
    s.push_str(&section("Boundary conditions (scalar fields)", &|_, kind| kind == Kind::ScalarBc));
    s.push_str(&section("Boundary conditions (vector fields)", &|_, kind| kind == Kind::VectorBc));
    s.push_str(&section("RAS turbulence models", &|_, kind| kind == Kind::Ras));
    s.push_str("## Fields read\n\nThe files the solver and its models read when they start, from the time the run starts\nfrom (*start*), `0`, `constant/<region>`, or the mesh's directory (*mesh*: written by the\nradiation utilities). *Required* files stop the run when they are missing.\n\n| Read by | Field | From | Required | Versions | File |\n|---|---|---|---|---|---|\n");
    for (f, v) in &m.fields {
        let owner = match &f.owner {
            Owner::Region(r) => format!("every {r} region"),
            Owner::Family(b) => format!("`{b}` (every type)"),
            Owner::Type(family, name) => format!("`{family}` `{name}`"),
            Owner::Other(a) => format!("`{a}`"),
        };
        let from = match f.instance {
            Instance::Time => "start",
            Instance::Zero => "`0`",
            Instance::Constant => "`constant`",
            Instance::Other => "mesh",
        };
        s.push_str(&format!(
            "| {owner} | `{}` | {from} | {} | {} | `{}` |\n",
            f.name,
            if f.required { "yes" } else { "no" },
            vers_md(v),
            f.file
        ));
    }
    s.push_str("\n## Keywords read at start\n\nThe required keywords each type reads whenever the solver builds it: in the constructors it\nis built with, or a function each of them calls, outside any condition. `coeffs_` is the\ntype's `<Type>Coeffs` dictionary.\n\n| Type | Keyword | Read from | Versions |\n|---|---|---|---|\n");
    for ((family, type_name, name, receiver), v) in &m.startup {
        s.push_str(&format!("| `{family}` `{type_name}` | `{name}` | `{receiver}` | {} |\n", vers_md(v)));
    }
    s.push_str("\n## Built per region\n\nThe classes the solver constructs (`<class>::New`) for each kind of region.\n\n| Region | Class | Versions |\n|---|---|---|\n");
    for ((region, class), v) in &m.builds {
        s.push_str(&format!("| {region} | `{class}` | {} |\n", vers_md(v)));
    }
    s.push('\n');
    s.push_str("## Application keywords\n\nWhat the solver, its utilities and its libraries read outside the model classes.\n\n| Application | File | Keywords |\n|---|---|---|\n");
    for ((app, file), kws) in &m.apps {
        s.push_str(&format!("| `{app}` | `{file}` | {} |\n", kws_md(kws)));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_field_reads() {
        let pat = Patterns::new().unwrap();
        let src = r#"
            U_(IOobject("U", runTime.name(), mesh, IOobject::MUST_READ, IOobject::AUTO_WRITE), mesh),
            k_(IOobject(this->groupName("k"), this->runTime_.name(), this->mesh_, IOobject::MUST_READ), this->mesh_),
            LAD_(IOobject("LAD", "0", mesh_, IOobject::MUST_READ), mesh_),
            g(IOobject("g", runTime.constant(), mesh, IOobject::MUST_READ, IOobject::NO_WRITE)),
            F(IOobject("F", mesh_.facesInstance(), mesh_, IOobject::MUST_READ)),
            phi(IOobject("phi", runTime.name(), mesh, IOobject::READ_IF_PRESENT)),
            rho(IOobject("rho", runTime.name(), mesh, IOobject::NO_READ)),
            IOdictionary d(IOobject("anyDict", runTime.constant(), mesh, IOobject::MUST_READ));
            typeIOobject<IOdictionary> r(IOobject("radiationProperties", runTime.constant(), mesh, IOobject::MUST_READ));
            IOobject(fluidNames[i], runTime.name(), runTime, IOobject::MUST_READ)
            typeIOobject<IOdictionary> t(IOobject("tDict", runTime.constant(), mesh, IOobject::MUST_READ)); n(IOobject("n", runTime.name(), mesh, IOobject::MUST_READ)),
            d_(IOdictionary(IOobject("aDict", runTime.constant(), mesh, IOobject::MUST_READ))), m(IOobject("m", runTime.name(), mesh, IOobject::MUST_READ)),
            s(IOobject("s", runTime.name(), "uniform", mesh, IOobject::MUST_READ)),
            IOobject io("h", runTime.name(), mesh, IOobject::MUST_READ); if (io.headerOk()) {}
        "#;
        let got = field_reads(&pat, src);
        assert_eq!(
            got,
            [
                ("U".to_string(), Instance::Time, true),
                ("k".to_string(), Instance::Time, true),
                ("LAD".to_string(), Instance::Zero, true),
                ("g".to_string(), Instance::Constant, true),
                ("F".to_string(), Instance::Other, true),
                ("phi".to_string(), Instance::Time, false),
                ("n".to_string(), Instance::Time, true),
                ("m".to_string(), Instance::Time, true),
                ("s".to_string(), Instance::Other, true),
            ]
        );
    }

    /// With several constructors, a read only some of them make is not required.
    #[test]
    fn reads_of_every_constructor() {
        let pat = Patterns::new().unwrap();
        let src = r#"
Foam::m::m(const volScalarField& T)
:
    a_(IOobject("qs", T.mesh().time().name(), T.mesh(), IOobject::MUST_READ)),
    b_(IOobject("qr", T.mesh().time().name(), T.mesh(), IOobject::MUST_READ))
{
}

Foam::m::m(const dictionary& dict, const volScalarField& T)
:
    a_(IOobject("Qs", T.mesh().time().name(), T.mesh(), IOobject::MUST_READ)),
    b_(IOobject("qr", T.mesh().time().name(), T.mesh(), IOobject::MUST_READ))
{
}
"#;
        let got = model_reads(&pat, src, "m", None);
        let required: Vec<&str> = got.iter().filter(|r| r.2).map(|r| r.0.as_str()).collect();
        assert_eq!(required, ["qr", "qr"]);
        assert!(got.iter().any(|r| r.0 == "qs" && !r.2) && got.iter().any(|r| r.0 == "Qs" && !r.2));
        // Built with one argument: only the one-parameter constructor counts.
        let one: Vec<(String, bool)> = model_reads(&pat, src, "m", Some(1)).into_iter().map(|r| (r.0, r.2)).collect();
        assert!(one.contains(&("qs".to_string(), true)) && one.contains(&("Qs".to_string(), false)), "{one:?}");
        // No constructor of the class in the file: every read counts.
        assert!(model_reads(&pat, src, "other", None).iter().all(|r| r.2));
        // A template class, and a read in a member function.
        let t = "Foam::P<B>::P(const B& b)\n:\n    k_(IOobject(\"k\", t.name(), m, IOobject::MUST_READ))\n{\n}\n\nvoid Foam::P<B>::correct()\n{\n    volScalarField x(IOobject(\"x\", t.name(), m, IOobject::MUST_READ), m);\n}\n";
        let got: Vec<(String, bool)> = model_reads(&pat, t, "P", None).into_iter().map(|r| (r.0, r.2)).collect();
        assert_eq!(got, [("k".to_string(), true), ("x".to_string(), false)]);
        // Read only if present in one used constructor: not required in the other either.
        let mixed = src.replacen(
            "b_(IOobject(\"qr\", T.mesh().time().name(), T.mesh(), IOobject::MUST_READ))",
            "b_(IOobject(\"qr\", T.mesh().time().name(), T.mesh(), IOobject::READ_IF_PRESENT))",
            1,
        );
        assert!(model_reads(&pat, &mixed, "m", None).iter().filter(|r| r.0 == "qr").all(|r| !r.2));
    }

    #[test]
    fn calls_are_not_definitions() {
        let src = "    m::f();\nvoid Foam::m::f(const label n) const\n{\n}\n";
        let call = src.find("f(").unwrap() + 1;
        assert_eq!(definition_params(src, call), None);
        for c in ["    m::f(x).y;\n", "    m::f(x) + g;\n", "    (m::f())\n", "    m::f(x) constant;\n"] {
            assert_eq!(definition_params(c, c.find("f(").unwrap() + 1), None, "{c}");
        }
        let ctor = "Foam::m::m(const T& t)\n:\n    a_(1)\n{\n}\n";
        assert_eq!(definition_params(ctor, ctor.find("m(").unwrap() + 1), Some(1));
        let def = src.rfind("f(").unwrap() + 1;
        assert_eq!(definition_params(src, def), Some(1));
    }

    /// Required keywords read whenever the model is built: in the constructor, or a function it
    /// calls, outside any condition.
    #[test]
    fn keywords_read_at_start() {
        let pat = Patterns::new().unwrap();
        let src = r#"
void Foam::m::initialise()
{
    bool s = readBool(coeffs_.lookup("smoothing"));
    if (Pstream::master())
    {
        x = readBool(coeffs_.lookup("onMaster"));
    }
    if (dict.found("y")) y = readScalar(coeffs_.lookup("ifFound"));
    if (a) { b(); } else c = readScalar(coeffs_.lookup("inElse"));
    forAll(p, i) q = readScalar(coeffs_.lookup("inLoop"));
    Pout<< "m::later() " << endl;
    m::correct();
    z = flag ? readScalar(coeffs_.lookup("ternary")) : 0;
}

void Foam::m::correct()
{
    w = readScalar(coeffs_.lookup("later"));
}

Foam::m::m(const volScalarField& T)
:
    a_(coeffs_.lookup("a1"))
{
    initialise();
}
"#;
        let names: Vec<String> = startup_keywords(&pat, src, "m", Some(1)).into_iter().map(|k| k.name).collect();
        assert_eq!(names, ["a1", "onMaster", "smoothing"]);
        // Called only under a condition: what it reads is not read at every build.
        let cond = src.replace("{\n    initialise();\n}", "{\n    if (flag) initialise();\n}");
        assert_ne!(cond, src);
        let names: Vec<String> = startup_keywords(&pat, &cond, "m", Some(1)).into_iter().map(|k| k.name).collect();
        assert_eq!(names, ["a1"]);
        // After an early return: not always.
        let ret = src.replace("    bool s = readBool", "    if (!on) return;\n    bool s = readBool");
        let names: Vec<String> = startup_keywords(&pat, &ret, "m", Some(1)).into_iter().map(|k| k.name).collect();
        assert_eq!(names, ["a1"]);
        // A second constructor that doesn't call initialise(): only what both read.
        let two = format!(
            "{src}\nFoam::m::m(const dictionary& d, const volScalarField& T)\n:\n    a_(coeffs_.lookup(\"a1\"))\n{{\n}}\n"
        );
        let names: Vec<String> = startup_keywords(&pat, &two, "m", None).into_iter().map(|k| k.name).collect();
        assert_eq!(names, ["a1"]);
        // Built with one argument: the dictionary constructor doesn't count.
        let names: Vec<String> = startup_keywords(&pat, &two, "m", Some(1)).into_iter().map(|k| k.name).collect();
        assert_eq!(names, ["a1", "onMaster", "smoothing"]);
    }

    #[test]
    fn strips_comments_keeps_strings() {
        let src = "a(\"//not\"); // gone\n/* gone\n */ b('/'); \\\nc";
        assert_eq!(strip(src), "a(\"//not\"); \n  b('/'); c");
    }

    #[test]
    fn finds_keywords() {
        let src = r#"
            x = dict.lookupOrDefault<word>("qrNbr", "none");
            y = readScalar(coeffs_.lookup("alpha"));
            if (dict.found("refValue")) {}
            const dictionary& d = runTime.controlDict().subDict("regions");
            IOdictionary(io).lookup("grassModel") >> t;
            dict.readIfPresent("nCorr", n);
            p.lookupPatchField<volScalarField, scalar>("T");
            dict.lookup(name);
        "#;
        let pat = Patterns::new().unwrap();
        let k: Vec<(String, Access, String)> =
            keywords(&pat, src).into_iter().map(|k| (k.name, k.access, k.receiver)).collect();
        assert_eq!(
            k,
            [
                ("alpha".into(), Access::Required, "coeffs_".into()),
                ("grassModel".into(), Access::Required, "IOdictionary(io)".into()),
                ("nCorr".into(), Access::Optional, "dict".into()),
                ("qrNbr".into(), Access::Default("\"none\"".into()), "dict".into()),
                ("refValue".into(), Access::Optional, "dict".into()),
                ("regions".into(), Access::SubDict, "controlDict()".into()),
            ]
        );
    }

    #[test]
    fn scans_registrations() {
        let mut files = BTreeMap::new();
        let f = |s: &str| strip(s);
        files.insert("_LIB/grass/grassModel/grassModel.H".into(), f("class grassModel { TypeName(\"grassModel\"); };\n#define addToGrassRunTimeSelectionTables(model) \\\n addToRunTimeSelectionTable(grassModel, model, T);\n"));
        files.insert(
            "_LIB/grass/grassModel/grassModelNew.C".into(),
            f("IOobject(\"grassProperties\"); IOdictionary(io).lookup(\"grassModel\") >> t;"),
        );
        files.insert(
            "_LIB/grass/simpleGrass/simpleGrass.H".into(),
            f("class simpleGrass : public grassModel { TypeName(\"simpleGrass\"); };"),
        );
        files.insert(
            "_LIB/grass/simpleGrass/simpleGrass.C".into(),
            f("addToGrassRunTimeSelectionTables(simpleGrass);\n x = coeffs_.lookupOrDefault(\"LAI\", 2.0);"),
        );
        files.insert("_LIB/bc/myBc/myBcFvPatchScalarField.H".into(), f("class myBcFvPatchScalarField : public mixedFvPatchScalarField { public: TypeName(\"compressible::myBc\"); };"));
        files.insert(
            "_LIB/bc/myBc/myBcFvPatchScalarField.C".into(),
            f("makePatchTypeField(fvPatchScalarField, myBcFvPatchScalarField); k = dict.lookup(\"Tnbr\");"),
        );
        files.insert("solid/readSolid.H".into(), f("PtrList<dictionary> m(props.lookup(\"buildingMaterials\"));"));
        let s = scan(&Patterns::new().unwrap(), &files).unwrap();
        let fam = s.families.get("grassModel").unwrap();
        assert_eq!((fam.selector.as_deref(), fam.dict_file.as_deref()), (Some("grassModel"), Some("grassProperties")));
        let grass = s.types.get(&("grassModel".into(), "simpleGrass".into())).unwrap();
        assert_eq!(grass.kind, Kind::Model);
        assert!(grass.keywords.iter().any(|k| k.name == "LAI" && k.access == Access::Default("2.0".into())));
        let bc = s.types.get(&("fvPatchScalarField".into(), "compressible::myBc".into())).unwrap();
        assert_eq!((bc.kind, bc.class.as_str()), (Kind::ScalarBc, "myBcFvPatchScalarField"));
        assert!(bc.keywords.iter().any(|k| k.name == "Tnbr"));
        assert!(s.apps.contains_key(&("urbanMicroclimateFoam".into(), "solid/readSolid.H".into())));
    }

    #[test]
    fn markdown_merges_receivers_and_keeps_every_access() {
        // Review finding: one keyword read through two receivers in different versions is in
        // both versions; a keyword read two ways shows both.
        let kw =
            |name: &str, access: Access, receiver: &str| Kw { name: name.into(), access, receiver: receiver.into() };
        let mut kws: Versioned<Kw> = BTreeMap::new();
        kws.insert(kw("vegetationModel", Access::Required, "dict"), [Ver::V8].into());
        kws.insert(kw("vegetationModel", Access::Required, "vegetationProperties"), [Ver::V12].into());
        kws.insert(kw("Tg", Access::Default("0.5".into()), "coeffs_"), [Ver::V8, Ver::V12].into());
        kws.insert(kw("Tg", Access::Default("1e-8".into()), "residualControlDict"), [Ver::V12].into());
        kws.insert(kw("regions", Access::SubDict, "controlDict()"), [Ver::V12].into());
        kws.insert(kw("regions", Access::Optional, "controlDict()"), [Ver::V12].into());
        assert_eq!(
            kws_md(&kws),
            "`Tg` = `0.5`, `Tg` = `1e-8` *(12 only)*, `regions` (optional) *(12 only)*, `regions` { } *(12 only)*, `vegetationModel`"
        );
        assert_eq!(kws_md(&BTreeMap::new()), "–");
    }

    #[test]
    fn external_base_classes_claim_nothing() {
        // Registered with an OpenFOAM base class the solver doesn't define: the solver's own
        // top-level keywords must stay application keywords.
        let mut files = BTreeMap::new();
        files.insert("_LIB/p/fix/fix.H".into(), strip("class fix : public searchableSurface { TypeName(\"fix\"); };"));
        files.insert("_LIB/p/fix/fix.C".into(), strip("addToRunTimeSelectionTable(searchableSurface, fix, dict);"));
        files.insert(
            "urbanMicroclimateFoam.C".into(),
            strip("const dictionary& r = runTime.controlDict().subDict(\"regions\");"),
        );
        let s = scan(&Patterns::new().unwrap(), &files).unwrap();
        assert!(s.families.get("searchableSurface").is_some_and(|f| f.keywords.is_empty()));
        let app = s.apps.get(&("urbanMicroclimateFoam".into(), "urbanMicroclimateFoam.C".into())).unwrap();
        assert!(app.iter().any(|k| k.name == "regions"));
    }
}
