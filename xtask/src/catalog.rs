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
    for c in args.chars() {
        match c {
            '"' => in_str = !in_str,
            '(' | '<' if !in_str => depth += 1,
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
    let mut out = BTreeSet::new();
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
        out.insert(Kw { name: key.to_string(), access, receiver });
    }
    out
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
