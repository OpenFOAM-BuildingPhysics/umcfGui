//! `umcf-cli`: the headless front end of the engine. Everything it does is a command run through
//! an `umcf_engine::Session`, the same ones the GUI runs.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::io::Write;
use std::process::ExitCode;

use serde_json::{Value, json};
use umcf_engine::{COMMANDS, Session};

const USAGE: &str = "\
usage:
  umcf-cli check <case> [--json] [--no-fields]
      Check a case before it runs. Prints the diagnostics, errors first, then a summary.
      --json       one JSON object: {\"errors\", \"warnings\", \"infos\", \"diagnostics\": [...]}
      --no-fields  don't read field files (faster; boundary conditions are not checked)
      Exit status: 0 no errors, 1 errors found, 2 the case could not be checked.

  umcf-cli run [--case <case>] [--cmd '<id>=<json>']... [--cmds <file>] [--journal <file>] [--json]
      Run commands in order, stopping at the first that fails. --case opens the case first
      (case.open). --cmd gives one command, with its parameters as a JSON object
      (--cmd 'control.set={\"endTime\": 3600}'); --cmds reads JSON lines
      {\"command\": \"...\", \"params\": {...}} (a journal; blank lines and lines starting with #
      are skipped). Edits are only written by case.save. --journal writes the edits made to the
      case open at the end (opening a case starts a new journal), as JSON lines, so
      `run --case <copy> --cmds <journal>` makes the same edits to another copy. --json prints one JSON line per command.
      Exit status: 0 all ran, 1 a command failed or the journal could not be written,
      2 bad arguments.

  umcf-cli commands [--json]
      List the commands, with their parameters.
";

/// The largest commands file read.
const MAX_CMDS_BYTES: u64 = 64 << 20;

fn main() -> ExitCode {
    // `std::env::args` panics on an argument that isn't UTF-8 (a case path on Linux can be).
    let args: Result<Vec<String>, _> = std::env::args_os().skip(1).map(std::ffi::OsString::into_string).collect();
    let args = match args {
        Ok(a) => a,
        Err(bad) => {
            eprintln!("umcf-cli: an argument is not UTF-8 text: {}", bad.to_string_lossy());
            return ExitCode::from(2);
        }
    };
    let code = match args.first().map(String::as_str) {
        Some("check") => check(args.get(1..).unwrap_or_default()),
        Some("run") => run(args.get(1..).unwrap_or_default()),
        Some("commands") => commands(args.get(1..).unwrap_or_default()),
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            Ok(0)
        }
        Some("-V" | "--version") => {
            println!("umcf-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        None => Err(USAGE.to_string()),
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match code {
        Ok(c) => ExitCode::from(c),
        Err(e) => {
            eprintln!("umcf-cli: {}", e.trim_end());
            ExitCode::from(2)
        }
    }
}

/// A line to stdout. A write that fails (the reader closed the pipe) ends the command with an
/// error; `println!` would panic.
fn out(line: &str) -> Result<(), String> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}").map_err(|e| format!("writing the output: {e}"))
}

fn check(args: &[String]) -> Result<u8, String> {
    let mut case = None;
    let (mut as_json, mut fields) = (false, true);
    for a in args {
        match a.as_str() {
            "--json" => as_json = true,
            "--no-fields" => fields = false,
            s if s.starts_with('-') => return Err(format!("check: unknown option `{s}`")),
            s if case.is_none() => case = Some(s),
            s => return Err(format!("check: one case at a time (`{s}`)")),
        }
    }
    let case = case.ok_or("check: which case? (umcf-cli check <case>)")?;
    let mut s = Session::new();
    s.execute("case.open", &json!({ "path": case, "fields": fields })).map_err(|e| e.to_string())?;
    let report = s.execute("case.check", &Value::Null).map_err(|e| e.to_string())?;
    if as_json {
        out(&report.data.to_string())?;
    } else {
        for d in report.data.get("diagnostics").and_then(Value::as_array).into_iter().flatten() {
            out(&diagnostic_line(d))?;
        }
        out(&report.message)?;
    }
    let errors = report.data.get("errors").and_then(Value::as_u64).unwrap_or(0);
    Ok(if errors > 0 { 1 } else { 0 })
}

/// `error    bc.missing  0/air/U [air inlet U]: message`
fn diagnostic_line(d: &Value) -> String {
    let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("");
    let at: Vec<&str> = ["region", "patch", "field"].into_iter().map(s).filter(|x| !x.is_empty()).collect();
    let mut place = s("path").to_string();
    if !at.is_empty() {
        if !place.is_empty() {
            place.push(' ');
        }
        place.push_str(&format!("[{}]", at.join(" ")));
    }
    if place.is_empty() {
        place.push_str("case");
    }
    format!("{:<8} {:<28} {place}: {}", s("level"), s("code"), s("message"))
}

/// A command and its parameters, from `--cmd` or a commands file.
struct Call {
    command: String,
    params: Value,
}

fn parse_cmd(arg: &str) -> Result<Call, String> {
    let (command, params) = match arg.split_once('=') {
        Some((c, p)) => {
            let params: Value =
                serde_json::from_str(p).map_err(|e| format!("--cmd {c}: the parameters are not JSON: {e}"))?;
            (c, params)
        }
        None => (arg, Value::Null),
    };
    Ok(Call { command: command.trim().to_string(), params })
}

fn read_cmds(path: &str) -> Result<Vec<Call>, String> {
    use std::io::Read;
    // The read itself is capped: a pipe or a device has no length to check first.
    let file = std::fs::File::open(path).map_err(|e| format!("--cmds {path}: {e}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_CMDS_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("--cmds {path}: {e}"))?;
    if bytes.len() as u64 > MAX_CMDS_BYTES {
        return Err(format!("--cmds {path}: larger than {MAX_CMDS_BYTES} bytes"));
    }
    let text = String::from_utf8(bytes).map_err(|_| format!("--cmds {path}: not UTF-8 text"))?;
    let mut calls = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = |what: &str| format!("--cmds {path}:{}: {what}", n + 1);
        let v: Value = serde_json::from_str(line).map_err(|e| bad(&format!("not JSON: {e}")))?;
        let command = v.get("command").and_then(Value::as_str).ok_or_else(|| bad("no \"command\""))?;
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        calls.push(Call { command: command.to_string(), params });
    }
    Ok(calls)
}

fn run(args: &[String]) -> Result<u8, String> {
    let mut calls = Vec::new();
    let (mut journal, mut as_json) = (None, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("run: {a} needs a value"));
        match a.as_str() {
            "--case" => calls.push(Call { command: "case.open".to_string(), params: json!({ "path": value()? }) }),
            "--cmd" => calls.push(parse_cmd(value()?)?),
            "--cmds" => calls.extend(read_cmds(value()?)?),
            "--journal" => journal = Some(value()?.clone()),
            "--json" => as_json = true,
            s => return Err(format!("run: unexpected `{s}`")),
        }
    }
    if calls.is_empty() {
        return Err("run: no commands (--case, --cmd or --cmds)".to_string());
    }
    let mut s = Session::new();
    let mut status = 0;
    for call in &calls {
        match s.execute(&call.command, &call.params) {
            Ok(o) if as_json => {
                out(&json!({ "command": call.command, "ok": true, "message": o.message, "data": o.data }).to_string())?
            }
            Ok(o) => out(&format!("{}: {}", call.command, o.message))?,
            Err(e) => {
                if as_json {
                    out(&json!({ "command": call.command, "ok": false, "message": e.to_string() }).to_string())?;
                } else {
                    // Parameter errors already start with the command's id.
                    let e = e.to_string();
                    if e.starts_with(&format!("{}:", call.command)) {
                        eprintln!("umcf-cli: {e}");
                    } else {
                        eprintln!("umcf-cli: {}: {e}", call.command);
                    }
                }
                status = 1;
                break;
            }
        }
    }
    if let Some(case) = s.case() {
        let unsaved = case.changed();
        if !unsaved.is_empty() {
            eprintln!("umcf-cli: not saved (no case.save): {}", unsaved.join(", "));
        }
    }
    if let Some(path) = journal {
        let mut text = String::new();
        for e in s.journal() {
            text.push_str(&e.to_string());
            text.push('\n');
        }
        // A journal that can't be written fails the run (1), but doesn't hide a command that
        // failed: it is not a usage error.
        if let Err(e) = std::fs::write(&path, text) {
            eprintln!("umcf-cli: --journal {path}: {e}");
            status = 1;
        }
    }
    Ok(status)
}

fn commands(args: &[String]) -> Result<u8, String> {
    let as_json = match args {
        [] => false,
        [a] if a == "--json" => true,
        _ => return Err("commands: the only option is --json".to_string()),
    };
    for c in COMMANDS {
        if as_json {
            out(&json!({ "id": c.id, "label": c.label, "menu": c.menu, "shortcut": c.shortcut, "params": c.params })
                .to_string())?;
        } else {
            out(&format!("{:<14} {}\n               {}", c.id, c.label, c.params))?;
        }
    }
    Ok(0)
}
