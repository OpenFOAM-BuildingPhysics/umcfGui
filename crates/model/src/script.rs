//! Case scripts (`Allrun`, `Allprepare`, `Allclean`, job scripts): what they run, copy, move
//! and remove, in order.
//!
//! This is a small reading of POSIX shell, enough for the scripts cases carry: line
//! continuations, comments, quotes, `;` `&&` `||` `|` `&`, redirections, here-documents,
//! variables set to literal words, `for` loops over literal words, `if`/`while` bodies (marked
//! conditional), `mpirun`/`srun` and the OpenFOAM `RunFunctions`/`CleanFunctions` helpers.
//! What it can't resolve (`$(…)`, a variable it doesn't know, a `cd` elsewhere) is kept and
//! marked unresolved, never guessed. Nothing is run.

use std::collections::BTreeMap;

/// A script of the case, read into steps.
#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    /// Case-relative path (`Allrun`, `job_run.sh`).
    pub path: String,
    pub steps: Vec<Step>,
    /// Processes the batch system gives the job (`#SBATCH --ntasks`, `#PBS … mpiprocs`, `#BSUB -n`).
    pub ntasks: Option<u32>,
    /// The first line is a `#!` line.
    pub shebang: bool,
    /// The first line, when it is not a `#!` line but looks like a broken one (`$#!/bin/sh`).
    pub first_line: String,
    /// Reading stopped at a limit ([`MAX_DEPTH`], [`MAX_WORK`], [`MAX_STEPS`]): the steps are the
    /// script's first ones only.
    pub truncated: bool,
}

/// Deepest nesting of `for`/`while`/`if` blocks read; deeper blocks are not read.
pub const MAX_DEPTH: usize = 32;
/// Most commands visited (loops count each pass), so nested loops can't multiply without end.
pub const MAX_WORK: usize = 1_000_000;
/// Most steps a script is read into.
pub const MAX_STEPS: usize = 100_000;
/// Longest variable value or expanded word kept (`x=$x$x` in a loop would double without end);
/// a longer one is not resolved.
pub const MAX_VALUE: usize = 64 << 10;
/// Most bytes all variable names and values hold together; past it a value is not resolved.
pub const MAX_VARS: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// 1-based line in the script.
    pub line: usize,
    pub kind: StepKind,
    /// Inside an `if`, `while` or `until` body, or after `||`: it may not run.
    pub conditional: bool,
    /// A word of it couldn't be resolved (`$(…)`, an unknown variable, a `cd` elsewhere):
    /// its paths are not to be trusted.
    pub unresolved: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StepKind {
    Run(Run),
    Copy {
        from: Vec<String>,
        to: String,
    },
    Move {
        from: Vec<String>,
        to: String,
    },
    Remove {
        paths: Vec<String>,
    },
    MakeDir {
        paths: Vec<String>,
    },
    /// `ln -s`, `touch`, a `> file` redirection, `foamDictionary -set file`: the path exists after.
    Write {
        path: String,
    },
    /// A `CleanFunctions` function (`cleanCase`, `cleanTimeDirectories`, …).
    Clean {
        function: String,
    },
    /// Another script of the case (`./Allprepare`, `bash Allrun.pre`).
    Call {
        script: String,
    },
    /// A command this reading doesn't follow; it may have written files.
    Other {
        command: String,
    },
}

/// An application run (a solver or utility), directly, through `runApplication`/`runParallel`,
/// or under `mpirun`/`srun`.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub app: String,
    pub args: Vec<String>,
    pub region: Option<String>,
    pub all_regions: bool,
    pub parallel: bool,
    /// `-dict <file>`.
    pub dict: Option<String>,
    /// How many processes `mpirun -np`/`srun -n` start, if it does.
    pub np: Option<Np>,
    /// Run through `runParallel`, which reads `system/decomposeParDict` (for its default
    /// count) even when given `-np`.
    pub run_parallel: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Np {
    Count(u32),
    /// `runParallel`: `numberOfSubdomains` of `system/decomposeParDict`.
    FromDict,
    /// A launcher without a count (the batch system decides), or one not resolved.
    Unknown,
}

/// The names of the case's scripts among the entries of the case directory.
pub fn is_script_name(name: &str) -> bool {
    let backup = [".orig", ".bak", ".old", "~"].iter().any(|s| name.ends_with(s));
    !backup
        && !name.starts_with('.')
        && (name.starts_with("All") || [".sh", ".slurm", ".sbatch", ".pbs", ".job"].iter().any(|s| name.ends_with(s)))
}

/// Read a script. Never fails: what doesn't parse becomes [`StepKind::Other`].
pub fn parse(path: &str, text: &str) -> Script {
    let first = text.lines().next().unwrap_or("");
    let shebang = first.starts_with("#!");
    let first_line = if !shebang && first.contains("#!") { first.to_string() } else { String::new() };
    let (cmds, _) = commands(text);
    let mut steps = Vec::new();
    let mut env = Env::default();
    let mut i = 0;
    exec(&cmds, &mut i, &mut env, Ctx::default(), &mut steps, &[]);
    Script { path: path.to_string(), steps, ntasks: ntasks(text), shebang, first_line, truncated: env.truncated }
}

// ---------------------------------------------------------------------------------------------
// Words

#[derive(Debug, Clone, PartialEq)]
enum Part {
    Lit(String),
    /// `$name` or `${name}`; true when inside double quotes (not word-split).
    Var(String, bool),
    /// `$(…)`, backticks, `$((…))`, `${x%y}`, `$@`: not resolved.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct Word {
    parts: Vec<Part>,
}

impl Word {
    fn push(&mut self, c: char) {
        if let Some(Part::Lit(s)) = self.parts.last_mut() {
            s.push(c);
        } else {
            self.parts.push(Part::Lit(c.to_string()));
        }
    }

    /// The word, if it is plain text (no expansion at all).
    fn literal(&self) -> Option<String> {
        let mut out = String::new();
        for p in &self.parts {
            match p {
                Part::Lit(s) => out.push_str(s),
                _ => return None,
            }
        }
        Some(out)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(Word),
    /// `;` `&&` `||` `|` `&` `(` `)` or a newline.
    Sep(&'static str),
    /// A redirection; `target` is false for `2>&1` and the like (no file word follows).
    Redirect {
        write: bool,
        target: bool,
    },
}

/// A simple command: its words, its line, and how it was joined to the previous one.
#[derive(Debug, Clone)]
struct Cmd {
    line: usize,
    words: Vec<Word>,
    /// Redirection targets that write files.
    writes: Vec<Word>,
    /// Runs only depending on the previous command: joined by `||`, or by `&&` to a test
    /// (`[ -f x ] && cp x y`).
    or: bool,
}

/// Split the text into simple commands.
fn commands(text: &str) -> (Vec<Cmd>, usize) {
    let chars: Vec<char> = text.chars().collect();
    let mut toks: Vec<(usize, Tok)> = Vec::new();
    let mut heredocs: Vec<(String, bool)> = Vec::new();
    let mut line = 1;
    let mut i = 0;
    let mut word: Option<Word> = None;
    let flush = |word: &mut Option<Word>, toks: &mut Vec<(usize, Tok)>, line: usize| {
        if let Some(w) = word.take() {
            toks.push((line, Tok::Word(w)));
        }
    };
    while let Some(&c) = chars.get(i) {
        match c {
            '\\' if chars.get(i + 1) == Some(&'\n') => {
                i += 2;
                line += 1;
                continue;
            }
            '\\' => {
                if let Some(&n) = chars.get(i + 1) {
                    word.get_or_insert_with(Word::default).push(n);
                }
                i += 2;
                continue;
            }
            '\n' => {
                flush(&mut word, &mut toks, line);
                toks.push((line, Tok::Sep("\n")));
                line += 1;
                i += 1;
                // Here-documents start on the next line: skip to their end markers.
                for (end, strip) in std::mem::take(&mut heredocs) {
                    while i < chars.len() {
                        let start = i;
                        while chars.get(i).is_some_and(|&c| c != '\n') {
                            i += 1;
                        }
                        let l: String = chars.get(start..i).map(|s| s.iter().collect()).unwrap_or_default();
                        i += 1;
                        line += 1;
                        let l = if strip { l.trim_start_matches('\t').to_string() } else { l };
                        if l == end {
                            break;
                        }
                    }
                }
                continue;
            }
            ' ' | '\t' | '\r' => {
                flush(&mut word, &mut toks, line);
                i += 1;
                continue;
            }
            '#' if word.is_none() => {
                while chars.get(i).is_some_and(|&c| c != '\n') {
                    i += 1;
                }
                continue;
            }
            '\'' => {
                let w = word.get_or_insert_with(Word::default);
                // A quoted empty string is still a word.
                if w.parts.is_empty() {
                    w.parts.push(Part::Lit(String::new()));
                }
                i += 1;
                while let Some(&c) = chars.get(i) {
                    i += 1;
                    if c == '\'' {
                        break;
                    }
                    if c == '\n' {
                        line += 1;
                    }
                    w.push(c);
                }
                continue;
            }
            '"' => {
                let w = word.get_or_insert_with(Word::default);
                if w.parts.is_empty() {
                    w.parts.push(Part::Lit(String::new()));
                }
                i += 1;
                while let Some(&c) = chars.get(i) {
                    match c {
                        '"' => {
                            i += 1;
                            break;
                        }
                        '\\' if chars.get(i + 1).is_some_and(|n| matches!(n, '"' | '\\' | '$' | '`' | '\n')) => {
                            if let Some(&n) = chars.get(i + 1) {
                                if n == '\n' {
                                    line += 1;
                                } else {
                                    w.push(n);
                                }
                            }
                            i += 2;
                        }
                        '$' | '`' => {
                            let (part, next, lines) = dollar(&chars, i);
                            let part = match part {
                                Part::Var(n, _) => Part::Var(n, true),
                                p => p,
                            };
                            w.parts.push(part);
                            i = next;
                            line += lines;
                        }
                        _ => {
                            if c == '\n' {
                                line += 1;
                            }
                            w.push(c);
                            i += 1;
                        }
                    }
                }
                continue;
            }
            '$' | '`' => {
                let (part, next, lines) = dollar(&chars, i);
                word.get_or_insert_with(Word::default).parts.push(part);
                i = next;
                line += lines;
                continue;
            }
            // `x=(a b c)`: an array assignment, one word (its value is not followed).
            '(' if word.as_ref().is_some_and(|w| matches!(w.parts.last(), Some(Part::Lit(l)) if l.ends_with('='))) => {
                let mut depth = 0usize;
                while let Some(&c) = chars.get(i) {
                    i += 1;
                    match c {
                        '(' => depth += 1,
                        ')' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                break;
                            }
                        }
                        '\n' => line += 1,
                        _ => {}
                    }
                }
                word.get_or_insert_with(Word::default).parts.push(Part::Unknown);
                continue;
            }
            // `name()` in a function definition is one word.
            '(' if chars.get(i + 1) == Some(&')') => {
                let w = word.get_or_insert_with(Word::default);
                w.push('(');
                w.push(')');
                i += 2;
                continue;
            }
            ';' | '&' | '|' | '(' | ')' => {
                flush(&mut word, &mut toks, line);
                let two: String = chars.get(i..i + 2).map(|s| s.iter().collect()).unwrap_or_default();
                let (sep, n) = match two.as_str() {
                    "&&" => ("&&", 2),
                    "||" => ("||", 2),
                    ";;" => (";", 2),
                    "&>" => {
                        toks.push((line, Tok::Redirect { write: true, target: true }));
                        i += if chars.get(i + 2) == Some(&'>') { 3 } else { 2 };
                        continue;
                    }
                    _ => match c {
                        ';' => (";", 1),
                        '&' => ("&", 1),
                        '|' => ("|", 1),
                        '(' => ("(", 1),
                        _ => (")", 1),
                    },
                };
                toks.push((line, Tok::Sep(sep)));
                i += n;
                continue;
            }
            '>' | '<' => {
                // A file-descriptor number right before the operator belongs to it (`2>`).
                if let Some(w) = &word
                    && w.literal().is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                {
                    word = None;
                } else {
                    flush(&mut word, &mut toks, line);
                }
                if c == '<' && chars.get(i + 1) == Some(&'<') {
                    // A here-document: `<<WORD`, `<<-WORD`, `<<'WORD'`. (`<<<` is a here-string.)
                    let mut j = i + 2;
                    if chars.get(j) == Some(&'<') {
                        toks.push((line, Tok::Redirect { write: false, target: true }));
                        i = j + 1;
                        continue;
                    }
                    let strip = chars.get(j) == Some(&'-');
                    if strip {
                        j += 1;
                    }
                    while chars.get(j).is_some_and(|&c| c == ' ' || c == '\t') {
                        j += 1;
                    }
                    let mut end = String::new();
                    while let Some(&c) = chars.get(j) {
                        if c.is_whitespace() || matches!(c, ';' | '&' | '|' | '<' | '>' | '(' | ')') {
                            break;
                        }
                        if !matches!(c, '\'' | '"' | '\\') {
                            end.push(c);
                        }
                        j += 1;
                    }
                    heredocs.push((end, strip));
                    i = j;
                    continue;
                }
                let write = c == '>';
                let mut j = i + 1;
                if chars.get(j) == Some(&'>') || chars.get(j) == Some(&'|') {
                    j += 1;
                }
                if chars.get(j) == Some(&'&') {
                    // `>&2`, `2>&1`: a descriptor, not a file. `>& file` is a file.
                    j += 1;
                    let digits = chars.get(j).is_some_and(|c| c.is_ascii_digit() || *c == '-');
                    if digits {
                        while chars.get(j).is_some_and(|c| c.is_ascii_digit() || *c == '-') {
                            j += 1;
                        }
                        toks.push((line, Tok::Redirect { write, target: false }));
                        i = j;
                        continue;
                    }
                }
                toks.push((line, Tok::Redirect { write, target: true }));
                i = j;
                continue;
            }
            _ => {
                word.get_or_insert_with(Word::default).push(c);
                i += 1;
            }
        }
    }
    flush(&mut word, &mut toks, line);

    let mut out = Vec::new();
    let mut cur = Cmd { line: 1, words: Vec::new(), writes: Vec::new(), or: false };
    let mut next_or = false;
    let marker = |line: usize, s: &str| Cmd {
        line,
        words: vec![Word { parts: vec![Part::Lit(s.to_string())] }],
        writes: Vec::new(),
        or: false,
    };
    let is_test = |c: &Cmd| matches!(first_word(c).as_deref(), Some("[" | "[[" | "test"));
    let mut it = toks.into_iter().peekable();
    while let Some((l, t)) = it.next() {
        match t {
            Tok::Word(w) => {
                if cur.words.is_empty() && cur.writes.is_empty() {
                    cur.line = l;
                }
                cur.words.push(w);
            }
            Tok::Redirect { write, target } => {
                if target
                    && let Some((_, Tok::Word(_))) = it.peek()
                    && let Some((_, Tok::Word(w))) = it.next()
                    && write
                {
                    cur.writes.push(w);
                }
            }
            Tok::Sep(s) => {
                let mut test = false;
                let mut last_or = false;
                if !cur.words.is_empty() || !cur.writes.is_empty() {
                    cur.or = next_or;
                    test = is_test(&cur);
                    last_or = cur.or;
                    out.push(std::mem::replace(
                        &mut cur,
                        Cmd { line: l, words: Vec::new(), writes: Vec::new(), or: false },
                    ));
                }
                // A subshell: markers, so a `cd` or variable inside it stays inside. An opening
                // marker carries how the group is joined (`[ -d x ] && ( … )`).
                if s == "(" || s == ")" {
                    let mut m = marker(l, s);
                    m.or = s == "(" && next_or;
                    out.push(m);
                }
                if s != "|" && s != "(" && s != ")" {
                    // `a || b`: b depends on a; `[ test ] && b && c`: b and c depend on the test.
                    next_or = s == "||" || (s == "&&" && (test || last_or));
                }
            }
        }
    }
    if !cur.words.is_empty() || !cur.writes.is_empty() {
        cur.or = next_or;
        out.push(cur);
    }
    // `do for …`, `then if …`, `else cp …`, `{ cp …`: the keyword is a command of its own, so
    // what follows it on the line is read like any command (a loop, an `if`).
    let mut split = Vec::with_capacity(out.len());
    for c in out {
        let keyword = matches!(first_word(&c).as_deref(), Some("do" | "then" | "else" | "{"));
        if keyword && c.words.len() > 1 {
            let mut words = c.words;
            let rest = words.split_off(1);
            split.push(Cmd { line: c.line, words, writes: Vec::new(), or: c.or });
            split.push(Cmd { line: c.line, words: rest, writes: c.writes, or: false });
        } else {
            split.push(c);
        }
    }
    let mut out = split;
    // A group (`( … )`, `{ …; }`) joined conditionally makes everything in it conditional.
    let mut groups: Vec<bool> = Vec::new();
    for c in &mut out {
        let inside = groups.iter().any(|g| *g);
        let head = first_word(c);
        match head.as_deref() {
            Some("(") => groups.push(c.or || inside),
            Some("{") => {
                c.or = c.or || inside;
                groups.push(c.or);
            }
            Some(")") | Some("}") => {
                groups.pop();
            }
            _ => c.or = c.or || inside,
        }
    }
    (out, line)
}

/// A `$…` or backtick expansion at `chars[i]`: (part, index after it, newlines inside).
fn dollar(chars: &[char], i: usize) -> (Part, usize, usize) {
    let count_lines = |a: usize, b: usize| chars.get(a..b).map_or(0, |s| s.iter().filter(|&&c| c == '\n').count());
    if chars.get(i) == Some(&'`') {
        let mut j = i + 1;
        while let Some(&c) = chars.get(j) {
            j += 1;
            if c == '\\' {
                j += 1;
            } else if c == '`' {
                break;
            }
        }
        return (Part::Unknown, j, count_lines(i, j));
    }
    match chars.get(i + 1) {
        Some('(') => {
            // `$(…)` or `$((…))`: to the matching parenthesis, skipping quoted text.
            let mut depth = 0usize;
            let mut j = i + 1;
            let mut quote: Option<char> = None;
            while let Some(&c) = chars.get(j) {
                j += 1;
                match (quote, c) {
                    (Some(q), c) if c == q => quote = None,
                    (Some(_), _) => {}
                    (None, '\'' | '"') => quote = Some(c),
                    (None, '\\') => j += 1,
                    (None, '(') => depth += 1,
                    (None, ')') => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            (Part::Unknown, j, count_lines(i, j))
        }
        Some('{') => {
            let mut j = i + 2;
            let mut name = String::new();
            let mut simple = true;
            while let Some(&c) = chars.get(j) {
                j += 1;
                if c == '}' {
                    break;
                }
                if c.is_ascii_alphanumeric() || c == '_' {
                    name.push(c);
                } else {
                    simple = false;
                }
            }
            let named = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
            if simple && named { (Part::Var(name, false), j, 0) } else { (Part::Unknown, j, count_lines(i, j)) }
        }
        Some(c) if c.is_ascii_alphabetic() || *c == '_' => {
            let mut j = i + 1;
            let mut name = String::new();
            while let Some(&c) = chars.get(j) {
                if c.is_ascii_alphanumeric() || c == '_' {
                    name.push(c);
                    j += 1;
                } else {
                    break;
                }
            }
            (Part::Var(name, false), j, 0)
        }
        // `$1`, `$@`, `$#`, `$?`, `$$`, `$!`, `$*`, `$-`, `$0`.
        Some(c) if c.is_ascii_digit() || "@#?$!*-".contains(*c) => (Part::Unknown, i + 2, 0),
        // A lone `$` is itself.
        _ => (Part::Lit("$".to_string()), i + 1, 0),
    }
}

// ---------------------------------------------------------------------------------------------
// Running through the commands

/// The variables, glob loop variables and `cd` state outside a subshell.
type Saved = (BTreeMap<String, Option<String>>, std::collections::BTreeSet<String>, bool, usize);

#[derive(Debug, Default, Clone)]
struct Env {
    /// Variables set to a literal value; `None` for one set to something not resolved.
    vars: BTreeMap<String, Option<String>>,
    /// Bytes the names and values in `vars` hold (at most [`MAX_VARS`]).
    var_bytes: usize,
    /// Loop variables over a glob (`for p in processor*`): bound to the pattern, which expands
    /// to that text but is not resolved.
    globs: std::collections::BTreeSet<String>,
    /// A `cd` somewhere other than the case: later paths are relative to an unknown place.
    lost: bool,
    /// The state outside each subshell `( … )` being read.
    outer: Vec<Saved>,
    /// Blocks being read inside each other.
    depth: usize,
    /// Commands visited so far.
    work: usize,
    /// A limit was reached: reading stopped.
    truncated: bool,
}

#[derive(Debug, Default, Clone, Copy)]
struct Ctx {
    conditional: bool,
}

impl Env {
    /// Set a variable. Names and values count against the [`MAX_VARS`] budget: a value that
    /// doesn't fit is not resolved, a new name that doesn't fit is not kept (an unknown
    /// variable is not resolved either).
    fn set(&mut self, name: &str, value: Option<String>) {
        let old = match self.vars.get(name) {
            Some(v) => v.as_ref().map_or(0, String::len),
            None => {
                if self.var_bytes + name.len() > MAX_VARS {
                    return;
                }
                self.var_bytes += name.len();
                0
            }
        };
        self.var_bytes = self.var_bytes.saturating_sub(old);
        let value = value.filter(|v| self.var_bytes + v.len() <= MAX_VARS);
        self.var_bytes += value.as_ref().map_or(0, String::len);
        self.vars.insert(name.to_string(), value);
    }
}

/// A word expanded into words: an unquoted `$x` alone is split at blanks, as the shell does
/// (`for r in $REGIONS`).
fn expand_words(w: &Word, env: &Env) -> Vec<(String, bool)> {
    if let [Part::Var(v, false)] = w.parts.as_slice()
        && let Some(Some(value)) = env.vars.get(v)
        && value.split_whitespace().nth(1).is_some()
    {
        let ok = !env.globs.contains(v);
        return value.split_whitespace().map(|s| (s.to_string(), ok)).collect();
    }
    vec![expand(w, env)]
}

/// Words, expanded: (text, resolved). Past [`MAX_VALUE`] bytes the rest is left out and the
/// word is not resolved.
fn expand(w: &Word, env: &Env) -> (String, bool) {
    let mut out = String::new();
    let mut ok = true;
    for p in &w.parts {
        if out.len() > MAX_VALUE {
            return (out, false);
        }
        match p {
            Part::Lit(s) => out.push_str(s),
            Part::Var(v, _) => match env.vars.get(v) {
                Some(Some(s)) => {
                    out.push_str(s);
                    if env.globs.contains(v) {
                        ok = false;
                    }
                }
                _ => {
                    ok = false;
                    out.push_str(&format!("${{{v}}}"));
                }
            },
            Part::Unknown => {
                ok = false;
                out.push_str("$(…)");
            }
        }
    }
    if out.len() > MAX_VALUE {
        ok = false;
    }
    (out, ok)
}

fn first_word(c: &Cmd) -> Option<String> {
    c.words.first().and_then(Word::literal)
}

/// Words that open a block, and the word that closes it.
fn closer(word: &str) -> Option<&'static str> {
    match word {
        "for" | "while" | "until" | "select" => Some("done"),
        "if" => Some("fi"),
        "case" => Some("esac"),
        "{" => Some("}"),
        _ => None,
    }
}

/// `then echo x` is `then` followed by the command `echo x`: split such a keyword off.
fn split_keyword(c: &Cmd) -> Option<(String, Cmd)> {
    let k = first_word(c)?;
    if matches!(k.as_str(), "then" | "do" | "else" | "{") && c.words.len() > 1 {
        let rest =
            Cmd { line: c.line, words: c.words.get(1..).unwrap_or(&[]).to_vec(), writes: c.writes.clone(), or: c.or };
        return Some((k, rest));
    }
    None
}

/// Run the commands from `cmds[*i]` until one whose first word is in `stop` (not consumed).
/// [`exec_block`], within the limits: a block nested deeper than [`MAX_DEPTH`] is not read.
fn exec(cmds: &[Cmd], i: &mut usize, env: &mut Env, ctx: Ctx, out: &mut Vec<Step>, stop: &[&str]) {
    if env.truncated || env.depth >= MAX_DEPTH {
        env.truncated = true;
        *i = cmds.len();
        return;
    }
    env.depth += 1;
    exec_block(cmds, i, env, ctx, out, stop);
    env.depth -= 1;
}

fn exec_block(cmds: &[Cmd], i: &mut usize, env: &mut Env, ctx: Ctx, out: &mut Vec<Step>, stop: &[&str]) {
    while let Some(c) = cmds.get(*i) {
        env.work += 1;
        if env.truncated || env.work > MAX_WORK || out.len() >= MAX_STEPS {
            env.truncated = true;
            *i = cmds.len();
            return;
        }
        let head = first_word(c).unwrap_or_default();
        if stop.contains(&head.as_str()) {
            return;
        }
        // A keyword followed by a command on the same line.
        if let Some((k, rest)) = split_keyword(c) {
            if stop.contains(&k.as_str()) {
                return;
            }
            // `{ cmd` or a stray `do cmd`: run the command.
            *i += 1;
            run_one(&rest, env, ctx, out);
            continue;
        }
        match head.as_str() {
            "for" | "select" => {
                *i += 1;
                let var = c.words.get(1).and_then(Word::literal).unwrap_or_default();
                // `for x in a b c`: the values, if all are plain words without globs.
                // `for x in a b c`: the values, if all are resolved; a glob (`processor*`) is
                // bound as its pattern, marked unresolved.
                let values: Option<Vec<(String, bool)>> = match c.words.get(2).and_then(Word::literal).as_deref() {
                    Some("in") => c
                        .words
                        .get(3..)
                        .unwrap_or(&[])
                        .iter()
                        .flat_map(|w| expand_words(w, env))
                        .map(|(s, ok)| {
                            ok.then(|| {
                                let glob = s.contains(['*', '?', '[']);
                                (s, glob)
                            })
                        })
                        .collect(),
                    _ => None,
                };
                let body = block(cmds, i, "done", env);
                let inner = Ctx { conditional: ctx.conditional };
                match values {
                    Some(vs) => {
                        for (v, glob) in vs {
                            // Each pass is work, also when the body is empty.
                            env.work += 1;
                            if env.truncated || env.work > MAX_WORK {
                                env.truncated = true;
                                break;
                            }
                            env.set(&var, Some(v));
                            // A glob variable is marked only once kept: its name is then in the
                            // MAX_VARS budget, which bounds what a subshell copies.
                            if glob && env.vars.contains_key(&var) {
                                env.globs.insert(var.clone());
                            } else {
                                env.globs.remove(&var);
                            }
                            let mut k = 0;
                            exec(body, &mut k, env, inner, out, &[]);
                        }
                    }
                    None => {
                        env.globs.remove(&var);
                        env.set(&var, None);
                        let mut k = 0;
                        exec(body, &mut k, env, inner, out, &[]);
                    }
                }
            }
            "while" | "until" => {
                *i += 1;
                let body = block(cmds, i, "done", env);
                // The condition, then the body after `do`; both may run any number of times.
                let mut k = 0;
                exec(body, &mut k, env, Ctx { conditional: true }, out, &["do"]);
                if let Some(d) = body.get(k)
                    && let Some((_, rest)) = split_keyword(d)
                {
                    run_one(&rest, env, Ctx { conditional: true }, out);
                }
                k += 1;
                exec(body, &mut k, env, Ctx { conditional: true }, out, &[]);
            }
            "if" => {
                *i += 1;
                let body = block(cmds, i, "fi", env);
                // The condition is the rest of the `if` line and what follows up to `then`.
                let cond = Cmd {
                    line: c.line,
                    words: c.words.get(1..).unwrap_or(&[]).to_vec(),
                    writes: c.writes.clone(),
                    or: false,
                };
                let quiet = |cmd: &Cmd| matches!(first_word(cmd).as_deref(), Some("[" | "[[" | "test" | "!"));
                if !cond.words.is_empty() && !quiet(&cond) {
                    run_one(&cond, env, ctx, out);
                }
                let mut k = 0;
                let arms = Ctx { conditional: true };
                loop {
                    // Up to `then` (more condition), then the arm up to `elif`/`else`/end.
                    while let Some(b) = body.get(k) {
                        if first_word(b).as_deref() == Some("then") {
                            break;
                        }
                        if !quiet(b) {
                            run_one(b, env, arms, out);
                        }
                        k += 1;
                    }
                    let Some(t) = body.get(k) else { break };
                    if let Some((_, rest)) = split_keyword(t) {
                        run_one(&rest, env, arms, out);
                    }
                    k += 1;
                    exec(body, &mut k, env, arms, out, &["elif", "else"]);
                    match body.get(k).and_then(first_word).as_deref() {
                        Some("elif") => {
                            if let Some(e) = body.get(k) {
                                let cond = Cmd {
                                    line: e.line,
                                    words: e.words.get(1..).unwrap_or(&[]).to_vec(),
                                    writes: Vec::new(),
                                    or: false,
                                };
                                if !cond.words.is_empty() && !quiet(&cond) {
                                    run_one(&cond, env, arms, out);
                                }
                            }
                            k += 1;
                        }
                        Some("else") => {
                            if let Some((_, rest)) = body.get(k).and_then(split_keyword) {
                                run_one(&rest, env, arms, out);
                            }
                            k += 1;
                            exec(body, &mut k, env, arms, out, &[]);
                            break;
                        }
                        _ => break,
                    }
                }
            }
            "case" => {
                *i += 1;
                let _ = block(cmds, i, "esac", env);
                out.push(step(c.line, StepKind::Other { command: "case".into() }, ctx, false));
            }
            _ if head.ends_with("()") || c.words.get(1).and_then(Word::literal).as_deref() == Some("()") => {
                // A function definition: its body runs only when called, which isn't followed.
                *i += 1;
                if cmds.get(*i).and_then(first_word).as_deref() == Some("{")
                    || c.words.last().and_then(Word::literal).as_deref() == Some("{")
                {
                    if cmds.get(*i).and_then(first_word).as_deref() == Some("{") {
                        *i += 1;
                    }
                    let _ = block(cmds, i, "}", env);
                }
            }
            "{" | "}" | "done" | "fi" | "esac" | "then" | "do" | "else" | "elif" => *i += 1,
            _ => {
                *i += 1;
                run_one(c, env, ctx, out);
            }
        }
    }
}

/// The commands up to the `end` word that closes the block just opened (nesting counted), which
/// is consumed. A slice of `cmds`: nested blocks don't copy.
fn block<'a>(cmds: &'a [Cmd], i: &mut usize, end: &str, env: &mut Env) -> &'a [Cmd] {
    let start = *i;
    let mut depth = 0usize;
    while let Some(c) = cmds.get(*i) {
        // Scanning is work too: a body skipped unread (`case`, a function, `for x in ;`) inside
        // a loop is scanned on every pass.
        env.work += 1;
        if env.work > MAX_WORK {
            env.truncated = true;
            *i = cmds.len();
            return &[];
        }
        *i += 1;
        // The block keyword: the first word, or the one after `do`/`then`/`else` on its line.
        // Compared in place: this runs for every command at every nesting level.
        let word = |k: usize| match c.words.get(k).map(|w| w.parts.as_slice()) {
            Some([Part::Lit(s)]) => s.as_str(),
            _ => "",
        };
        let mut head = word(0);
        if matches!(head, "do" | "then" | "else") && c.words.len() > 1 {
            head = word(1);
        }
        if closer(head).is_some() {
            depth += 1;
        } else if matches!(head, "done" | "fi" | "esac" | "}") {
            if depth == 0 && head == end {
                return cmds.get(start..*i - 1).unwrap_or(&[]);
            }
            depth = depth.saturating_sub(1);
        }
    }
    cmds.get(start..).unwrap_or(&[])
}

fn step(line: usize, kind: StepKind, ctx: Ctx, unresolved: bool) -> Step {
    Step { line, kind, conditional: ctx.conditional, unresolved }
}

/// Builtins and tools that change no case file (except through a redirection).
const QUIET: &[&str] = &[
    "echo",
    "printf",
    "set",
    "continue",
    "break",
    "export",
    "unset",
    "exit",
    "return",
    "true",
    "false",
    ":",
    ".",
    "source",
    "sleep",
    "module",
    "ml",
    "wait",
    "test",
    "[",
    "[[",
    "ulimit",
    "date",
    "cat",
    "shift",
    "read",
    "local",
    "trap",
    "umask",
    "pwd",
    "ls",
    "grep",
    "egrep",
    "tail",
    "head",
    "which",
    "type",
    "command",
    "hash",
    "awk",
    "wc",
    "sort",
    "uniq",
    "tee",
    "basename",
    "dirname",
    "readlink",
    "realpath",
    "find",
    "env",
    "hostname",
    "whoami",
    "id",
    "uname",
    "nproc",
    "free",
    "df",
    "du",
    "seq",
    "expr",
    "let",
    "cut",
    "tr",
    "diff",
    "cmp",
    "md5sum",
    "sha256sum",
    "stat",
    "file",
    "history",
    "alias",
    "isTest",
    "getApplication",
    "getNumberOfProcessors",
    "notTest",
    "foamInfo",
    "foamVersion",
    "foamListTimes",
    "squeue",
    "sacct",
    "scontrol",
    "sinfo",
    "nvidia-smi",
    "mail",
    "logger",
    "printenv",
];

/// The `CleanFunctions` functions of OpenFOAM-8 and 12.
const CLEAN: &[&str] = &[
    "cleanCase",
    "cleanCase0",
    "cleanTimeDirectories",
    "cleanDynamicCode",
    "cleanSnappyFiles",
    "cleanOptimisation",
    "cleanPostProcessing",
    "cleanUcomponents",
    "cleanApplication",
    "cleanExplicitFeatures",
    "cleanVoFCase",
    "cleanAuxiliary",
    "removeCase",
    "cleanFaMesh",
    "cleanSamples",
];

/// Prefixes that run the rest of the line as a command.
const PREFIX: &[&str] = &["time", "nohup", "exec", "nice", "stdbuf", "builtin", "eval"];

fn run_one(c: &Cmd, env: &mut Env, ctx: Ctx, out: &mut Vec<Step>) {
    // Reading stopped at a limit: nothing more is read, also from an `if` condition's lines.
    if env.truncated || out.len() >= MAX_STEPS {
        env.truncated = true;
        return;
    }
    // Subshell markers (also inside an `if` condition): what changes inside stays inside.
    match first_word(c).as_deref() {
        Some("(") if c.words.len() == 1 => {
            // Subshells nested deeper than MAX_DEPTH stop the reading, like deep blocks: the
            // saved states stay bounded, and no `)` restores another subshell's state.
            if env.outer.len() >= MAX_DEPTH {
                env.truncated = true;
                return;
            }
            // Saving the state copies it: charged as work by its size (at most MAX_VARS).
            // Glob names are names in `vars`, so `var_bytes` covers their bytes too.
            env.work += 1 + env.var_bytes / 1024 + env.vars.len() + env.globs.len();
            if env.work > MAX_WORK {
                env.truncated = true;
                return;
            }
            env.outer.push((env.vars.clone(), env.globs.clone(), env.lost, env.var_bytes));
            return;
        }
        Some(")") if c.words.len() == 1 => {
            if let Some((vars, globs, lost, var_bytes)) = env.outer.pop() {
                env.vars = vars;
                env.var_bytes = var_bytes;
                env.globs = globs;
                env.lost = lost;
            }
            return;
        }
        _ => {}
    }
    let ctx = Ctx { conditional: ctx.conditional || c.or };
    let mut words: Vec<(String, bool)> = c.words.iter().flat_map(|w| expand_words(w, env)).collect();
    // Assignments: alone they set variables; before a command they only set its environment.
    let assigns = words.iter().take_while(|(w, _)| w.split_once('=').is_some_and(|(n, _)| valid_name(n))).count();
    if assigns == words.len() {
        for (k, w) in c.words.iter().enumerate().take(assigns) {
            let Some((name, _)) = words.get(k).and_then(|(s, _)| s.split_once('=')) else { continue };
            // Literal only if the raw word is: `n=$(foamDictionary …)` is not resolved.
            let value = w.literal().and_then(|l| l.split_once('=').map(|(_, v)| v.to_string()));
            let value = value.or_else(|| {
                // `i=$j` with `j` known.
                let (s, ok) = expand(w, env);
                ok.then(|| s.split_once('=').map(|(_, v)| v.to_string())).flatten()
            });
            env.globs.remove(name);
            let value = value.filter(|v| v.len() <= MAX_VALUE);
            env.set(name, value);
        }
        writes(c, env, ctx, out);
        return;
    }
    words.drain(..assigns);
    while words.first().is_some_and(|(w, _)| PREFIX.contains(&w.as_str())) {
        words.remove(0);
    }
    let Some((head, head_ok)) = words.first().cloned() else {
        writes(c, env, ctx, out);
        return;
    };
    let all_ok = words.iter().all(|(_, ok)| *ok) && !env.lost;
    let args: Vec<String> = words.iter().skip(1).map(|(w, _)| w.clone()).collect();
    let line = c.line;
    let push = |out: &mut Vec<Step>, kind: StepKind, ok: bool| out.push(step(line, kind, ctx, !ok));
    if !head_ok {
        push(out, StepKind::Other { command: head }, false);
        writes(c, env, ctx, out);
        return;
    }
    match head.as_str() {
        "cd" => {
            // `cd ${0%/*}` and `cd "$(dirname "$0")"` go to the script's directory: the case.
            let to = c.words.get(1);
            let to_script_dir = to.is_none_or(|w| w.parts.iter().any(|p| matches!(p, Part::Unknown)))
                || args.first().is_some_and(|a| a == "." || a == "${SLURM_SUBMIT_DIR}" || a == "${PBS_O_WORKDIR}");
            if !to_script_dir {
                env.lost = true;
            }
        }
        h if QUIET.contains(&h) => {}
        h if CLEAN.contains(&h) => push(out, StepKind::Clean { function: h.to_string() }, all_ok),
        "restore0Dir" => {
            push(out, StepKind::Remove { paths: vec!["0".into()] }, !env.lost);
            push(out, StepKind::Copy { from: vec!["0.orig".into()], to: "0".into() }, !env.lost);
        }
        "cp" | "mv" => {
            let paths = operands(&args);
            if let Some((to, from)) = paths.split_last()
                && !from.is_empty()
            {
                let from: Vec<String> = from.iter().map(|p| normal(p)).collect();
                let to = normal(to);
                let kind = if head == "cp" { StepKind::Copy { from, to } } else { StepKind::Move { from, to } };
                push(out, kind, all_ok);
            } else {
                push(out, StepKind::Other { command: head.clone() }, all_ok);
            }
        }
        "rm" | "rmdir" => {
            let paths: Vec<String> = operands(&args).iter().map(|p| normal(p)).collect();
            push(out, StepKind::Remove { paths }, all_ok);
        }
        "mkdir" => {
            let paths: Vec<String> = operands(&args).iter().map(|p| normal(p)).collect();
            push(out, StepKind::MakeDir { paths }, all_ok);
        }
        "touch" => {
            for p in operands(&args) {
                push(out, StepKind::Write { path: normal(p) }, all_ok);
            }
        }
        "ln" => {
            if let Some(to) = operands(&args).last() {
                push(out, StepKind::Write { path: normal(to) }, all_ok);
            }
        }
        "sh" | "bash" | "dash" | "zsh" | "ksh" => match operands(&args).first() {
            Some(s) => push(out, StepKind::Call { script: normal(s) }, all_ok),
            None => push(out, StepKind::Other { command: head.clone() }, all_ok),
        },
        h if h.starts_with("./") || (h.contains('/') && !h.starts_with('/') && !h.starts_with('$')) => {
            push(out, StepKind::Call { script: normal(h) }, all_ok);
        }
        "runApplication" | "runParallel" => {
            // RunFunctions options: -append/-a, -overwrite/-o, -suffix/-s X, -log/-l X, -np/-n N.
            let mut k = 0;
            let mut np = (head == "runParallel").then_some(Np::FromDict);
            while let Some(a) = args.get(k) {
                match a.as_str() {
                    "-a" | "-append" | "-o" | "-overwrite" => k += 1,
                    "-s" | "-suffix" | "-l" | "-log" => k += 2,
                    "-n" | "-np" => {
                        np = Some(args.get(k + 1).and_then(|n| n.parse().ok()).map_or(Np::Unknown, Np::Count));
                        k += 2;
                    }
                    _ => break,
                }
            }
            match args.get(k) {
                Some(app) => {
                    let mut r = run(app, args.get(k + 1..).unwrap_or(&[]));
                    if head == "runParallel" {
                        r.parallel = true;
                        r.np = np;
                        r.run_parallel = true;
                    }
                    push(out, StepKind::Run(r), all_ok);
                }
                None => push(out, StepKind::Other { command: head.clone() }, all_ok),
            }
        }
        "mpirun" | "mpiexec" | "orterun" | "prterun" | "srun" => {
            let (np, k) = launcher(&head, &args);
            match args.get(k) {
                Some(app) => {
                    let mut r = run(app, args.get(k + 1..).unwrap_or(&[]));
                    r.np = Some(np);
                    push(out, StepKind::Run(r), all_ok);
                }
                None => push(out, StepKind::Other { command: head.clone() }, all_ok),
            }
        }
        "foamDictionary" => {
            let r = run(&head, &args);
            // `foamDictionary file -set …` changes the file.
            let edits = args.iter().any(|a| matches!(a.as_str(), "-set" | "-add" | "-remove" | "-merge"));
            let file = args.iter().find(|a| !a.starts_with('-')).cloned();
            push(out, StepKind::Run(r), all_ok);
            if edits && let Some(f) = file {
                push(out, StepKind::Write { path: normal(&f) }, all_ok);
            }
        }
        _ => push(out, StepKind::Run(run(&head, &args)), all_ok),
    }
    writes(c, env, ctx, out);
}

/// Redirection targets: the files exist after the command.
fn writes(c: &Cmd, env: &Env, ctx: Ctx, out: &mut Vec<Step>) {
    for w in &c.writes {
        let (p, ok) = expand(w, env);
        if p != "/dev/null" {
            out.push(step(c.line, StepKind::Write { path: normal(&p) }, ctx, !ok || env.lost));
        }
    }
}

fn valid_name(n: &str) -> bool {
    n.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The operands of `cp`/`mv`/`rm`/…: the arguments that are not options (`--` ends options).
fn operands(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut opts = true;
    for a in args {
        if opts && a == "--" {
            opts = false;
        } else if opts && a.starts_with('-') && a.len() > 1 {
            continue;
        } else {
            out.push(a.as_str());
        }
    }
    out
}

/// `mpirun`/`srun` options: (process count, index of the application).
fn launcher(head: &str, args: &[String]) -> (Np, usize) {
    let mut np = Np::Unknown;
    let mut k = 0;
    // Options that take a value.
    let valued: &[&str] = if head == "srun" {
        &[
            "-n",
            "--ntasks",
            "-N",
            "--nodes",
            "-c",
            "--cpus-per-task",
            "-p",
            "--partition",
            "-t",
            "--time",
            "-o",
            "-e",
            "--mem",
            "-J",
            "--job-name",
            "-A",
            "--account",
            "--mpi",
            "-w",
            "--nodelist",
            "--distribution",
            "-m",
        ]
    } else {
        &[
            "-np",
            "-n",
            "-c",
            "--np",
            "-N",
            "-npernode",
            "--npernode",
            "-x",
            "-H",
            "--host",
            "-host",
            "--hostfile",
            "-hostfile",
            "-machinefile",
            "--machinefile",
            "--map-by",
            "-map-by",
            "--bind-to",
            "-bind-to",
            "--rank-by",
            "-wdir",
            "--wdir",
            "-ppn",
            "--rankfile",
            "-rf",
            "-env",
            "-genv",
            "--prefix",
            "-launcher",
            "-f",
        ]
    };
    while let Some(a) = args.get(k) {
        if !a.starts_with('-') {
            break;
        }
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (a.as_str(), None),
        };
        let count = matches!(name, "-np" | "-n" | "-c" | "--np" | "--ntasks") && !(head == "srun" && name == "-c");
        if name == "--mca" || name == "-mca" || name == "-genv" || name == "-env" {
            k += 3;
            continue;
        }
        if valued.contains(&name) {
            let v = inline.or_else(|| args.get(k + 1).cloned());
            if count {
                np = v.and_then(|v| v.parse().ok()).map_or(Np::Unknown, Np::Count);
            }
            k += if a.contains('=') && a.starts_with("--") { 1 } else { 2 };
        } else {
            k += 1;
        }
    }
    (np, k)
}

/// An application with OpenFOAM's common options read.
fn run(app: &str, args: &[String]) -> Run {
    let mut r = Run {
        app: app.rsplit('/').next().unwrap_or(app).to_string(),
        args: args.to_vec(),
        region: None,
        all_regions: false,
        parallel: false,
        dict: None,
        np: None,
        run_parallel: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-region" => r.region = it.next().cloned(),
            "-allRegions" => r.all_regions = true,
            "-parallel" => r.parallel = true,
            "-dict" => r.dict = it.next().map(|d| normal(d)),
            _ => {}
        }
    }
    r
}

/// A path as the case sees it: no leading `./`, no `.` or empty components, no trailing `/`.
/// A path that ends in `/` or `/.` names a directory: it keeps one trailing `/`.
pub fn normal(p: &str) -> String {
    let dir = p.ends_with('/') || p.ends_with("/.") || p == ".";
    let abs = p.starts_with('/');
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    let mut out = parts.join("/");
    if abs {
        out.insert(0, '/');
    }
    if dir && !out.is_empty() {
        out.push('/');
    }
    out
}

/// `#SBATCH --ntasks`/`-n`, `--nodes` × `--ntasks-per-node`; `#PBS -l select=…:mpiprocs=…` or
/// `nodes=…:ppn=…`; `#BSUB -n`.
fn ntasks(text: &str) -> Option<u32> {
    let mut ntasks = None;
    let mut nodes = None;
    let mut per_node = None;
    for l in text.lines() {
        let l = l.trim();
        if let Some(rest) = l.strip_prefix("#SBATCH") {
            let toks: Vec<&str> = rest.split_whitespace().take_while(|t| !t.starts_with('#')).collect();
            let mut k = 0;
            while let Some(t) = toks.get(k) {
                let (name, val) = match t.split_once('=') {
                    Some((n, v)) => (n, Some(v)),
                    None if !t.starts_with("--") && t.strip_prefix("-n").is_some_and(|v| !v.is_empty()) => {
                        ("-n", t.strip_prefix("-n"))
                    }
                    None => (*t, toks.get(k + 1).copied()),
                };
                let num = val.and_then(|v| v.parse::<u32>().ok());
                match name {
                    "--ntasks" | "-n" => ntasks = num.or(ntasks),
                    "--nodes" | "-N" => nodes = num.or(nodes),
                    "--ntasks-per-node" => per_node = num.or(per_node),
                    _ => {}
                }
                k +=
                    if t.contains('=') || (t.starts_with("-n") && t.len() > 2 && !t.starts_with("--")) { 1 } else { 2 };
            }
        } else if let Some(rest) = l.strip_prefix("#PBS") {
            for spec in rest.split_whitespace().filter(|t| t.contains("select=") || t.contains("nodes=")) {
                let mut kv = BTreeMap::new();
                for part in spec.split(':') {
                    if let Some((k, v)) = part.split_once('=') {
                        kv.insert(k.trim_start_matches("-l").trim(), v.parse::<u32>().ok());
                    }
                }
                let get = |k: &str| kv.get(k).copied().flatten();
                let n = get("select").or(get("nodes")).unwrap_or(1);
                if let Some(p) = get("mpiprocs").or(get("ppn")).or(get("ncpus")) {
                    ntasks = n.checked_mul(p);
                }
            }
        } else if let Some(rest) = l.strip_prefix("#BSUB") {
            let toks: Vec<&str> = rest.split_whitespace().collect();
            if let Some(k) = toks.iter().position(|t| *t == "-n") {
                ntasks = toks.get(k + 1).and_then(|v| v.parse().ok()).or(ntasks);
            }
        }
    }
    ntasks.or_else(|| nodes.zip(per_node).and_then(|(a, b)| a.checked_mul(b)))
}

#[cfg(test)]
#[path = "script_tests.rs"]
mod tests;
