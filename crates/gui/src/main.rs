//! `umcf-gui`: the umcfGui desktop application.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::process::ExitCode;

use umcf_ui_egui::{Options, run};

const USAGE: &str = "\
usage: umcf-gui [<case>] [--cmd '<id>=<json>']... [--show <name>]... [--dark] [--size <w>x<h>]
                [--screenshot <file.png>]
  <case>        a case directory to open
  --cmd         a command to run once the case is open (as umcf-cli run --cmd); repeat for more
  --show        a mesh region or surface (walls.stl) the 3D view shows first; repeat for more
  --dark        the dark theme
  --size        the window's size in points (default 1440x900)
  --screenshot  write a PNG of the window once it has drawn, then quit
";

fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--dark" => o.dark = true,
            "--show" => o.show.push(value("--show")?),
            "--screenshot" => o.screenshot = Some(value("--screenshot")?),
            "--size" => {
                let v = value("--size")?;
                let bad = || format!("--size is <width>x<height>, not `{v}`");
                let (w, h) = v.split_once('x').ok_or_else(bad)?;
                let (w, h): (f32, f32) = (w.parse().map_err(|_| bad())?, h.parse().map_err(|_| bad())?);
                if !(w >= 200.0 && h >= 150.0 && w.is_finite() && h.is_finite()) {
                    return Err(bad());
                }
                o.size = Some((w, h));
            }
            "--cmd" => {
                let v = value("--cmd")?;
                let (id, params) = match v.split_once('=') {
                    Some((id, p)) => (
                        id.trim().to_string(),
                        umcf_ui_egui::parse_params(p).map_err(|e| format!("--cmd {id}: not JSON: {e}"))?,
                    ),
                    None => (v.trim().to_string(), umcf_ui_egui::empty_params()),
                };
                o.commands.push((id, params));
            }
            s if s.starts_with('-') => return Err(format!("unknown option `{s}`")),
            s if o.case.is_none() => o.case = Some(s.to_string()),
            s => return Err(format!("one case at a time (`{s}`)")),
        }
    }
    Ok(o)
}

fn main() -> ExitCode {
    let args: Result<Vec<String>, _> = std::env::args_os().skip(1).map(std::ffi::OsString::into_string).collect();
    let Ok(args) = args else {
        eprintln!("umcf-gui: an argument is not UTF-8 text");
        return ExitCode::from(2);
    };
    let options = match parse(&args) {
        Ok(o) => o,
        Err(e) if e.is_empty() => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("umcf-gui: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("umcf-gui: {e}");
            ExitCode::FAILURE
        }
    }
}
