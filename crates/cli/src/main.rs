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

  umcf-cli mesh <case> [--region <region>] [--all-geometry] [--json]
      Check the mesh of each region (or one) as checkMesh does by default: closed cells and
      boundary, aspect ratio, face areas, cell volumes, non-orthogonality, face pyramids,
      skewness, with checkMesh's thresholds. Prints the problems, then a line per region.
      --all-geometry  also the checks of checkMesh -allGeometry: face tets, short edges, near
                      points, concave faces and cells, warped faces, cell determinants, face
                      interpolation weights, volume ratios (slower)
      --json  one JSON object, with each region's numbers and the faces and cells involved
      Exit status: 0 no errors, 1 errors found (a mesh that can't be read is one), 2 the case
      could not be opened.

  umcf-cli surface <case | file.stl> [--file <name>] [--json]
      Check STL surfaces as snappyHexMesh will read them: a file OpenFOAM can't read, open and
      non-manifold edges, inconsistent and inward normals, degenerate and duplicate triangles,
      coordinates that look like millimetres or are far from the origin. Given a case, every
      STL surface in its constant/triSurface (or the one --file names); given a file, that file.
      --json  one JSON object, with each surface's numbers and the triangles involved
      Exit status: 0 no errors, 1 errors found (a file that can't be read is one), 2 the case
      or file could not be opened.

  umcf-cli render <case> --out <file.png> [--view <view>] [--size <w>x<h>] [--region <r>]...
                 [--surface <file>]... [--edges] [--select <what>]... [--pick <x>,<y>] [--json]
      Draw the case in 3D (the CPU renderer) into a PNG file: the boundary patches of every
      region with a mesh, or the STL surfaces when there is none, each in its own colour.
      --view       iso (default), top, bottom, front, back, left or right; --azimuth <deg>,
                   --elevation <deg> and --zoom <factor> adjust it
      --size       image size in pixels (default 1280x800)
      --region, --surface  what to draw (a mesh region, \"\" in a single-region case; an STL file
                   in constant/triSurface); both can be repeated
      --edges      draw the faces' edges (the mesh's cells at the boundary)
      --select     draw a patch (<region>/<patch>, or <patch> in a single-region case) or a
                   surface region (<file.stl>/<region>) as selected (orange); can be repeated
      --pick       select what is under pixel x,y of the image, and list everything under it
      --json       one JSON object: the image's camera and objects, and the pick
      Exit status: 0 drawn, 2 the case could not be opened or drawn.

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
        Some("mesh") => mesh(args.get(1..).unwrap_or_default()),
        Some("surface") => surface(args.get(1..).unwrap_or_default()),
        Some("render") => render(args.get(1..).unwrap_or_default()),
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

fn mesh(args: &[String]) -> Result<u8, String> {
    let (mut case, mut region, mut as_json, mut all_geometry) = (None, None, false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => as_json = true,
            "--all-geometry" => all_geometry = true,
            "--region" => region = Some(it.next().ok_or("mesh: --region needs a value")?.clone()),
            s if s.starts_with('-') => return Err(format!("mesh: unknown option `{s}`")),
            s if case.is_none() => case = Some(s.to_string()),
            s => return Err(format!("mesh: one case at a time (`{s}`)")),
        }
    }
    let case = case.ok_or("mesh: which case? (umcf-cli mesh <case>)")?;
    let mut s = Session::new();
    // Fields are not needed for the mesh.
    s.execute("case.open", &json!({ "path": case, "fields": false })).map_err(|e| e.to_string())?;
    let params = json!({ "region": region, "all_geometry": all_geometry });
    let report = s.execute("mesh.check", &params).map_err(|e| e.to_string())?;
    if as_json {
        out(&report.data.to_string())?;
    } else {
        for d in report.data.get("diagnostics").and_then(Value::as_array).into_iter().flatten() {
            out(&diagnostic_line(d))?;
        }
        for r in report.data.get("regions").and_then(Value::as_array).into_iter().flatten() {
            for d in r.get("diagnostics").and_then(Value::as_array).into_iter().flatten() {
                out(&diagnostic_line(d))?;
            }
        }
        for line in report.message.split("; ") {
            out(line)?;
        }
    }
    let errors = report.data.get("errors").and_then(Value::as_u64).unwrap_or(0);
    Ok(if errors > 0 { 1 } else { 0 })
}

fn surface(args: &[String]) -> Result<u8, String> {
    let (mut target, mut file, mut as_json) = (None, None, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => as_json = true,
            "--file" => file = Some(it.next().ok_or("surface: --file needs a value")?.clone()),
            s if s.starts_with('-') => return Err(format!("surface: unknown option `{s}`")),
            s if target.is_none() => target = Some(s.to_string()),
            s => return Err(format!("surface: one case or file at a time (`{s}`)")),
        }
    }
    let target = target.ok_or("surface: which case or file? (umcf-cli surface <case | file.stl>)")?;
    let mut s = Session::new();
    let params = if std::path::Path::new(&target).is_dir() {
        s.execute("case.open", &json!({ "path": target, "fields": false })).map_err(|e| e.to_string())?;
        json!({ "file": file })
    } else if file.is_some() {
        return Err("surface: --file names a surface of a case; give the case directory".to_string());
    } else {
        json!({ "path": target })
    };
    let report = s.execute("geometry.check", &params).map_err(|e| e.to_string())?;
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

/// What `--select` names: `region/patch`, `patch` (single-region case) or `file.stl/region`.
fn selection_item(arg: &str) -> Value {
    let stl = |f: &str| [".stl", ".stlb", ".stl.gz", ".stlb.gz"].iter().any(|e| f.ends_with(e));
    match arg.rsplit_once('/') {
        Some((file, solid)) if stl(file) => json!({ "surface": file, "solid": solid }),
        Some((region, patch)) => json!({ "region": region, "patch": patch }),
        None => json!({ "region": "", "patch": arg }),
    }
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let abc = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let digits: Option<Vec<u32>> = text
        .bytes()
        .filter(|b| *b != b'=')
        .map(|b| abc.iter().position(|a| *a == b).and_then(|p| u32::try_from(p).ok()))
        .collect();
    let mut out = Vec::new();
    for c in digits?.chunks(4) {
        let n = c.iter().zip([18, 12, 6, 0]).fold(0u32, |n, (d, s)| n | (d << s));
        out.extend_from_slice(n.to_be_bytes().get(1..c.len())?);
    }
    Some(out)
}

fn render(args: &[String]) -> Result<u8, String> {
    let mut case = None;
    let (mut out_file, mut pick, mut as_json) = (None, None, false);
    let (mut regions, mut surfaces, mut select) = (Vec::new(), Vec::new(), Vec::new());
    let mut params = serde_json::Map::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or(format!("render: {name} needs a value"));
        match a.as_str() {
            "--json" => as_json = true,
            "--edges" => {
                params.insert("edges".into(), json!(true));
            }
            "--out" => out_file = Some(value("--out")?),
            "--view" => {
                params.insert("view".into(), json!(value("--view")?));
            }
            o @ ("--azimuth" | "--elevation" | "--zoom") => {
                let v = value(o)?;
                let x: f64 = v
                    .parse::<f64>()
                    .ok()
                    .filter(|x| x.is_finite())
                    .ok_or_else(|| format!("render: {o} is a finite number, not `{v}`"))?;
                params.insert(o.trim_start_matches('-').into(), json!(x));
            }
            "--size" => {
                let v = value("--size")?;
                let bad = || format!("render: --size is <width>x<height> in pixels, not `{v}`");
                let (w, h) = v.split_once('x').ok_or_else(bad)?;
                let (w, h): (u32, u32) = (w.parse().map_err(|_| bad())?, h.parse().map_err(|_| bad())?);
                params.insert("width".into(), json!(w));
                params.insert("height".into(), json!(h));
            }
            "--region" => regions.push(value("--region")?),
            "--surface" => surfaces.push(value("--surface")?),
            "--select" => select.push(selection_item(&value("--select")?)),
            "--pick" => {
                let v = value("--pick")?;
                let bad = || format!("render: --pick is <x>,<y> in pixels, not `{v}`");
                let (x, y) = v.split_once(',').ok_or_else(bad)?;
                let (x, y): (f64, f64) = (x.trim().parse().map_err(|_| bad())?, y.trim().parse().map_err(|_| bad())?);
                pick = Some(json!([x, y]));
            }
            s if s.starts_with('-') => return Err(format!("render: unknown option `{s}`")),
            s if case.is_none() => case = Some(s.to_string()),
            s => return Err(format!("render: one case at a time (`{s}`)")),
        }
    }
    let case = case.ok_or("render: which case? (umcf-cli render <case> --out <file.png>)")?;
    let out_file = out_file.ok_or("render: where to? (--out <file.png>)")?;
    if !regions.is_empty() {
        params.insert("regions".into(), json!(regions));
    }
    if !surfaces.is_empty() {
        params.insert("surfaces".into(), json!(surfaces));
    }
    let mut s = Session::new();
    s.execute("case.open", &json!({ "path": case, "fields": false })).map_err(|e| e.to_string())?;
    if !select.is_empty() {
        s.execute("select.set", &json!({ "items": select })).map_err(|e| e.to_string())?;
    }
    let picked = match pick {
        Some(px) => {
            let mut p = params.clone();
            p.remove("edges");
            p.insert("pixel".into(), px);
            if !select.is_empty() {
                p.insert("mode".into(), json!("add"));
            }
            Some(s.execute("select.pick", &Value::Object(p)).map_err(|e| e.to_string())?)
        }
        None => None,
    };
    let shot = s.execute("scene.snapshot", &Value::Object(params)).map_err(|e| e.to_string())?;
    let png =
        shot.data.get("png").and_then(Value::as_str).and_then(base64_decode).ok_or("render: no image came back")?;
    std::fs::write(&out_file, &png).map_err(|e| format!("render: writing {out_file}: {e}"))?;
    if as_json {
        let mut data = shot.data.clone();
        if let Some(o) = data.as_object_mut() {
            o.remove("png");
            o.insert("file".into(), json!(out_file));
            o.insert("pick".into(), picked.map_or(Value::Null, |p| p.data));
        }
        out(&data.to_string())?;
    } else {
        if let Some(p) = &picked {
            for h in p.data.get("hits").and_then(Value::as_array).into_iter().flatten() {
                let st = |k: &str| h.get(k).and_then(Value::as_str);
                let what = match (st("region"), st("patch"), st("surface"), st("solid")) {
                    (Some(""), Some(patch), _, _) => format!("patch {patch}"),
                    (Some(r), Some(patch), _, _) => format!("patch {patch} of region {r}"),
                    (_, _, Some(f), Some(solid)) => format!("region {solid} of {f}"),
                    _ => continue,
                };
                let d = h.get("distance").and_then(Value::as_f64).unwrap_or(f64::NAN);
                out(&format!("under the cursor: {what}, {d:.3} m away"))?;
            }
            out(&p.message)?;
        }
        out(&format!("{}: {}", out_file, shot.message))?;
    }
    Ok(0)
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
