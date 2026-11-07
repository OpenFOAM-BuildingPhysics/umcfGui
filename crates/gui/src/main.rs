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

use umcf_ui_egui::{Frame, Options, Picked, Start, run};

const USAGE: &str = "\
usage: umcf-gui [<case>] [--cmd '<id>=<json>']... [--show <name>]... [--open <id>] [--dark]
                [--size <w>x<h>] [--renderer cpu|gpu|auto] [--screenshot <file.png>]
  <case>        a case directory to open
  --cmd         a command to run once the case is open (as umcf-cli run --cmd); repeat for more
  --show        a mesh region or surface (walls.stl) the 3D view shows first; repeat for more
  --open        a command whose card opens at the start, as its ribbon button opens it (bc.set)
  --dark        the dark theme
  --size        the window's size in points (default 1440x900)
  --renderer    what draws the 3D view at the start: without it, the CPU, with the window on a
                hardware GPU where there is one so that the view can be switched to it (GPU in
                the View tab, the navigation bar or the history bar); gpu, or auto (a hardware
                GPU where there is one) to start on the GPU; cpu: the window always on OpenGL,
                no switch (for a machine where a GPU window opens but can't show, as over
                ssh -X). A GPU window that doesn't open is opened again on OpenGL.
  --screenshot  write a PNG of the window once it has drawn, then quit
";

/// The system's own folder dialog, for *Open*: `IFileDialog` on Windows, `NSOpenPanel` on macOS,
/// the desktop portal (`xdg-desktop-portal`, through D-Bus) or else `zenity` on Linux; over the
/// app's window.
fn pick_folder(start: Option<&str>, parent: &Frame) -> Picked {
    if !dialog_available() {
        return Picked::Unavailable;
    }
    let mut dialog = rfd::FileDialog::new().set_title("Open a case").set_parent(parent);
    if let Some(dir) = start.filter(|d| std::path::Path::new(d).is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    let Some(path) = dialog.pick_folder() else { return Picked::Cancelled };
    match path.into_os_string().into_string() {
        Ok(p) => Picked::Folder(p),
        Err(p) => {
            eprintln!("umcf-gui: {} is not UTF-8 text: it can't be opened", p.to_string_lossy());
            Picked::Cancelled
        }
    }
}

/// Whether there is a folder dialog to show. Windows and macOS always have theirs. On Linux the
/// portal runs with a desktop (`XDG_CURRENT_DESKTOP` set, and a D-Bus session), and `zenity`
/// (the fallback) must be on the PATH: without either, the dialog would come back as if
/// cancelled. A desktop without a portal service running can still come back so.
fn dialog_available() -> bool {
    if !cfg!(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "netbsd",
        target_os = "openbsd"
    )) {
        return true;
    }
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    let bus = set("DBUS_SESSION_BUS_ADDRESS")
        || std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|d| std::path::Path::new(&d).join("bus").exists());
    let portal = bus && set("XDG_CURRENT_DESKTOP");
    let zenity =
        std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("zenity").is_file()));
    portal || zenity
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options { pick_folder: Some(pick_folder), ..Options::default() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--dark" => o.dark = true,
            "--show" => o.show.push(value("--show")?),
            "--open" => o.open = Some(value("--open")?),
            "--screenshot" => o.screenshot = Some(value("--screenshot")?),
            "--renderer" => {
                let v = value("--renderer")?;
                o.renderer = Start::parse(&v).ok_or(format!("--renderer is cpu, gpu or auto, not `{v}`"))?;
            }
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
