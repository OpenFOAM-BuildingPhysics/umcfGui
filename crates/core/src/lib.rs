//! Shared basics for every umcfGui crate: case file access ([`CaseFs`]) and the OpenFOAM
//! versions umcfGui supports ([`OfVersion`]).
//!
//! Case access goes through [`CaseFs`] so that layers above work on a directory ([`DirFs`]),
//! in memory ([`MemFs`], for tests and the web viewer) or anything else, and stay wasm-clean.
//! Saving goes through [`CaseWrite`], whose writes are atomic.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// The OpenFOAM-org versions urbanMicroclimateFoam is ported to and umcfGui supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OfVersion {
    V8,
    V12,
}

impl OfVersion {
    pub const ALL: [OfVersion; 2] = [OfVersion::V8, OfVersion::V12];

    pub fn label(self) -> &'static str {
        match self {
            OfVersion::V8 => "OpenFOAM-8",
            OfVersion::V12 => "OpenFOAM-12",
        }
    }
}

impl std::fmt::Display for OfVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("{path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("{path} is {size} bytes, more than the {limit}-byte limit")]
    TooLarge { path: String, size: u64, limit: u64 },
    #[error("`{0}` is not a path inside the case")]
    BadPath(String),
}

/// Read access to a case. Paths are relative to the case root and use `/`
/// (`system/controlDict`, `constant/air/polyMesh/boundary`).
pub trait CaseFs {
    /// The file's bytes, `Ok(None)` if there is no such file. Files bigger than `limit` are an
    /// error, so a stray 10 GB file can't exhaust memory.
    fn read(&self, rel: &str, limit: u64) -> Result<Option<Vec<u8>>, FsError>;
    /// At most the first `n` bytes of the file (a header), `Ok(None)` if there is no such file.
    /// The default reads the whole file (within `limit`) and keeps its start; a file system that
    /// can read part of a file (on disk) reads only those bytes, whatever the file's size.
    fn read_prefix(&self, rel: &str, n: usize, limit: u64) -> Result<Option<Vec<u8>>, FsError> {
        Ok(self.read(rel, limit)?.map(|mut b| {
            b.truncate(n);
            b
        }))
    }
    /// The file's bytes as a stream, `Ok(None)` if there is no such file; a file bigger than
    /// `limit` is an error, as for [`CaseFs::read`]. The default reads the whole file (within
    /// `limit`) and hands it out; a file system on disk reads the file as the stream is read, so
    /// a reader that parses as it goes never holds the file whole.
    fn reader(&self, rel: &str, limit: u64) -> Result<Option<Box<dyn std::io::Read + Send>>, FsError> {
        Ok(self.read(rel, limit)?.map(|b| Box::new(std::io::Cursor::new(b)) as Box<dyn std::io::Read + Send>))
    }
    fn is_file(&self, rel: &str) -> bool;
    fn is_dir(&self, rel: &str) -> bool;
    /// Names of the entries directly inside `rel` (files and directories), sorted. A path that
    /// doesn't exist or isn't a directory lists as empty; any other failure (permissions, I/O)
    /// is an error, never an empty listing.
    fn list(&self, rel: &str) -> Result<Vec<String>, FsError>;

    /// What the file at `rel` really is: two paths with the same identity are one file (a
    /// symlinked dictionary shared by two regions). `None` when it can't be told.
    fn identity(&self, rel: &str) -> Option<PathBuf> {
        Some(PathBuf::from(rel))
    }

    /// Whether `rel` itself is a symbolic link, whether or not what it points to exists (a
    /// linked directory on the way doesn't count).
    fn is_link(&self, _rel: &str) -> bool {
        false
    }

    /// How many hard links the file has (1 for a plain file). A save replaces the file at one
    /// path, so the other links keep the old text.
    fn hard_links(&self, _rel: &str) -> u64 {
        1
    }
}

/// Write access to a case. A write is atomic: readers see the old file or the new one, never a
/// half-written file (on disk: a temporary file next to the target, then a rename). Missing
/// parent directories are created.
pub trait CaseWrite: CaseFs {
    fn write(&mut self, rel: &str, bytes: &[u8]) -> Result<(), FsError>;
}

/// Rejects absolute paths and `..`, so a path taken from a dictionary can't leave the case.
fn check_rel(rel: &str) -> Result<(), FsError> {
    let ok = Path::new(rel).components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if ok { Ok(()) } else { Err(FsError::BadPath(rel.to_string())) }
}

/// A case directory on disk.
#[derive(Debug, Clone)]
pub struct DirFs {
    root: PathBuf,
}

impl DirFs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        DirFs { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path(&self, rel: &str) -> Option<PathBuf> {
        check_rel(rel).ok().map(|()| self.root.join(rel))
    }
}

impl CaseFs for DirFs {
    fn read(&self, rel: &str, limit: u64) -> Result<Option<Vec<u8>>, FsError> {
        let path = self.path(rel).ok_or_else(|| FsError::BadPath(rel.to_string()))?;
        let meta = match std::fs::metadata(&path) {
            Ok(m) if m.is_file() => m,
            Ok(_) => return Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(FsError::Io { path: rel.to_string(), source }),
        };
        if meta.len() > limit {
            return Err(FsError::TooLarge { path: rel.to_string(), size: meta.len(), limit });
        }
        std::fs::read(&path).map(Some).map_err(|source| FsError::Io { path: rel.to_string(), source })
    }

    fn read_prefix(&self, rel: &str, n: usize, _limit: u64) -> Result<Option<Vec<u8>>, FsError> {
        use std::io::Read;
        let path = self.path(rel).ok_or_else(|| FsError::BadPath(rel.to_string()))?;
        let io = |source| FsError::Io { path: rel.to_string(), source };
        match std::fs::metadata(&path) {
            Ok(m) if m.is_file() => {}
            Ok(_) => return Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(io(source)),
        }
        let file = std::fs::File::open(&path).map_err(io)?;
        let mut out = Vec::new();
        file.take(n as u64).read_to_end(&mut out).map_err(io)?;
        Ok(Some(out))
    }

    fn reader(&self, rel: &str, limit: u64) -> Result<Option<Box<dyn std::io::Read + Send>>, FsError> {
        let path = self.path(rel).ok_or_else(|| FsError::BadPath(rel.to_string()))?;
        let io = |source| FsError::Io { path: rel.to_string(), source };
        // Only a regular file is opened: opening a named pipe would wait for a writer.
        match std::fs::metadata(&path) {
            Ok(m) if m.is_file() => {}
            Ok(_) => return Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(io(source)),
        }
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(io(source)),
        };
        // The size of the file that was opened, not of whatever is at the path by now.
        let meta = file.metadata().map_err(io)?;
        if !meta.is_file() {
            return Ok(None);
        }
        if meta.len() > limit {
            return Err(FsError::TooLarge { path: rel.to_string(), size: meta.len(), limit });
        }
        // At most `limit` bytes, should the file grow while it is read.
        use std::io::Read;
        Ok(Some(Box::new(std::io::BufReader::new(file).take(limit))))
    }

    fn is_file(&self, rel: &str) -> bool {
        self.path(rel).is_some_and(|p| p.is_file())
    }

    fn is_dir(&self, rel: &str) -> bool {
        self.path(rel).is_some_and(|p| p.is_dir())
    }

    fn identity(&self, rel: &str) -> Option<PathBuf> {
        std::fs::canonicalize(self.path(rel)?).ok()
    }

    fn is_link(&self, rel: &str) -> bool {
        self.path(rel).and_then(|p| std::fs::symlink_metadata(p).ok()).is_some_and(|m| m.file_type().is_symlink())
    }

    #[cfg(unix)]
    fn hard_links(&self, rel: &str) -> u64 {
        use std::os::unix::fs::MetadataExt;
        self.path(rel).and_then(|p| std::fs::metadata(p).ok()).map_or(1, |m| m.nlink())
    }

    fn list(&self, rel: &str) -> Result<Vec<String>, FsError> {
        let path = self.path(rel).ok_or_else(|| FsError::BadPath(rel.to_string()))?;
        let io = |source| FsError::Io { path: rel.to_string(), source };
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound || path.is_file() => return Ok(Vec::new()),
            Err(e) => return Err(io(e)),
        };
        let mut names = Vec::new();
        for e in entries {
            names.push(e.map_err(io)?.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names)
    }
}

impl CaseWrite for DirFs {
    fn write(&mut self, rel: &str, bytes: &[u8]) -> Result<(), FsError> {
        use std::io::Write;
        let path = self.path(rel).ok_or_else(|| FsError::BadPath(rel.to_string()))?;
        let io = |source| FsError::Io { path: rel.to_string(), source };
        // A symlinked file is written through the link, as an editor would, instead of being
        // replaced by a plain file. Only inside the case: a case from elsewhere must not make a
        // save write to `~/.bashrc`, through a linked file or a linked directory.
        let root = std::fs::canonicalize(&self.root).map_err(io)?;
        let target = match std::fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(&path).map_err(io)?,
            _ => path,
        };
        let dir = target.parent().ok_or_else(|| FsError::BadPath(rel.to_string()))?;
        // The nearest directory that exists decides where new ones would be created.
        let existing = dir.ancestors().find(|a| a.exists()).unwrap_or(dir);
        let real = std::fs::canonicalize(existing).map_err(io)?;
        if !real.starts_with(&root) {
            return Err(FsError::BadPath(format!("{rel} (leads to {}, outside the case)", real.display())));
        }
        std::fs::create_dir_all(dir).map_err(io)?;
        let dir = std::fs::canonicalize(dir).map_err(io)?;
        if !dir.starts_with(&root) {
            return Err(FsError::BadPath(format!("{rel} (leads to {}, outside the case)", dir.display())));
        }
        // The name as the OS has it (not lossily converted: a link to a non-UTF-8 name must
        // write that file, not a new one).
        let name = target.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
        let target = dir.join(&name);
        // The temporary file is created new, never opened: a file or symlink already at that
        // name (planted by a case, or another save running) makes this try the next name
        // instead of writing through it.
        let (tmp, mut f) = {
            let mut attempt = 0u32;
            loop {
                let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let mut tmp_name = std::ffi::OsString::from(".");
                tmp_name.push(&name);
                tmp_name.push(format!(".umcfgui-tmp-{}-{n}", std::process::id()));
                let tmp = dir.join(tmp_name);
                match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
                    Ok(f) => break (tmp, f),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 100 => attempt += 1,
                    Err(e) => return Err(io(e)),
                }
            }
        };
        let result = (|| {
            f.write_all(bytes)?;
            // Keep the mode of the file being replaced (an executable script stays executable).
            // Unix only: elsewhere a read-only flag would make the rename fail.
            #[cfg(unix)]
            if let Ok(m) = std::fs::metadata(&target) {
                use std::os::unix::fs::PermissionsExt;
                f.set_permissions(std::fs::Permissions::from_mode(m.permissions().mode()))?;
            }
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, &target)?;
            // The rename itself survives a power cut only once the directory is synced. Best
            // effort: the new file is in place either way, and some filesystems (network,
            // FUSE) refuse to sync a directory. Failing here would report a save that happened
            // as one that didn't.
            #[cfg(unix)]
            if let Ok(d) = std::fs::File::open(&dir) {
                let _ = d.sync_all();
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result.map_err(io)
    }
}

/// Makes temporary-file names unique within the process.
static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A case held in memory: `path → bytes`. Directories exist implicitly.
#[derive(Debug, Clone, Default)]
pub struct MemFs {
    files: BTreeMap<String, Vec<u8>>,
}

impl MemFs {
    pub fn new() -> Self {
        MemFs::default()
    }

    /// Adds a file (builder style).
    pub fn with(mut self, rel: &str, contents: impl Into<Vec<u8>>) -> Self {
        self.insert(rel, contents);
        self
    }

    pub fn insert(&mut self, rel: &str, contents: impl Into<Vec<u8>>) {
        self.files.insert(rel.trim_matches('/').to_string(), contents.into());
    }
}

impl CaseFs for MemFs {
    fn read(&self, rel: &str, limit: u64) -> Result<Option<Vec<u8>>, FsError> {
        check_rel(rel)?;
        match self.files.get(rel) {
            Some(b) if b.len() as u64 > limit => {
                Err(FsError::TooLarge { path: rel.to_string(), size: b.len() as u64, limit })
            }
            other => Ok(other.cloned()),
        }
    }

    fn is_file(&self, rel: &str) -> bool {
        self.files.contains_key(rel)
    }

    fn is_dir(&self, rel: &str) -> bool {
        let prefix = format!("{}/", rel.trim_matches('/'));
        rel.is_empty() || self.files.keys().any(|k| k.starts_with(&prefix))
    }

    fn list(&self, rel: &str) -> Result<Vec<String>, FsError> {
        check_rel(rel)?;
        let rel = rel.trim_matches('/');
        let prefix = if rel.is_empty() { String::new() } else { format!("{rel}/") };
        let mut names: Vec<String> = self
            .files
            .keys()
            .filter_map(|k| k.strip_prefix(&prefix))
            .filter_map(|rest| rest.split('/').next())
            .map(str::to_string)
            .collect();
        names.dedup();
        Ok(names)
    }
}

impl CaseWrite for MemFs {
    fn write(&mut self, rel: &str, bytes: &[u8]) -> Result<(), FsError> {
        check_rel(rel)?;
        self.insert(rel, bytes.to_vec());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefix_reads_only_the_start() {
        let root = std::env::temp_dir().join(format!("umcf-core-prefix-{}", std::process::id()));
        std::fs::create_dir_all(root.join("d")).unwrap();
        std::fs::write(root.join("d/f"), b"0123456789").unwrap();
        let fs = DirFs::new(&root);
        assert_eq!(fs.read_prefix("d/f", 4, 0).unwrap().as_deref(), Some(&b"0123"[..]));
        // The whole file when it is shorter; nothing for a missing file or a directory.
        assert_eq!(fs.read_prefix("d/f", 64, 0).unwrap().as_deref(), Some(&b"0123456789"[..]));
        assert_eq!(fs.read_prefix("d/missing", 4, 0).unwrap(), None);
        assert_eq!(fs.read_prefix("d", 4, 0).unwrap(), None);
        assert!(fs.read_prefix("../x", 4, 0).is_err());
        // The default (in memory) the same.
        let mut m = MemFs::new();
        m.insert("d/f", b"0123456789".to_vec());
        assert_eq!(m.read_prefix("d/f", 4, 1 << 20).unwrap().as_deref(), Some(&b"0123"[..]));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_reader_streams_the_file() {
        use std::io::Read;
        let root = std::env::temp_dir().join(format!("umcf-core-reader-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("d")).unwrap();
        std::fs::write(root.join("d/f"), b"0123456789").unwrap();
        let fs = DirFs::new(&root);
        let mut out = Vec::new();
        fs.reader("d/f", 10).unwrap().unwrap().read_to_end(&mut out).unwrap();
        assert_eq!(out, b"0123456789");
        // Too big, missing, a directory, outside the case.
        assert!(matches!(fs.reader("d/f", 9), Err(FsError::TooLarge { size: 10, limit: 9, .. })));
        assert!(fs.reader("d/missing", 10).unwrap().is_none());
        assert!(fs.reader("d", 10).unwrap().is_none());
        assert!(fs.reader("../x", 10).is_err());
        // The default (in memory) the same.
        let m = MemFs::new().with("d/f", b"0123456789".to_vec());
        let mut out = Vec::new();
        m.reader("d/f", 10).unwrap().unwrap().read_to_end(&mut out).unwrap();
        assert_eq!(out, b"0123456789");
        assert!(m.reader("d/f", 9).is_err());
        assert!(m.reader("d/missing", 10).unwrap().is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_reader_doesnt_open_a_named_pipe() {
        // Opening a pipe for reading waits for a writer: it isn't a file, so it isn't opened.
        let root = std::env::temp_dir().join(format!("umcf-core-fifo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let made = std::process::Command::new("mkfifo").arg(root.join("p")).status();
        if !made.is_ok_and(|s| s.success()) {
            eprintln!("SKIP a_reader_doesnt_open_a_named_pipe: no mkfifo");
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        let fs = DirFs::new(&root);
        assert!(fs.reader("p", 10).unwrap().is_none());
        assert!(fs.read("p", 10).unwrap().is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn writes_replace_files_atomically_and_keep_links() {
        let root = std::env::temp_dir().join(format!("umcf-core-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut d = DirFs::new(&root);
        // The case itself must exist; directories inside it are created.
        assert!(d.write("system/x", b"").is_err());
        std::fs::create_dir_all(&root).unwrap();
        d.write("system/air/fvSchemes", b"a 1;").unwrap();
        assert_eq!(d.read("system/air/fvSchemes", 100).unwrap().as_deref(), Some(&b"a 1;"[..]));
        d.write("system/air/fvSchemes", b"a 2;").unwrap();
        assert_eq!(d.read("system/air/fvSchemes", 100).unwrap().as_deref(), Some(&b"a 2;"[..]));
        // No temporary file is left behind.
        assert_eq!(d.list("system/air").unwrap(), ["fvSchemes"]);
        // A hard-linked file is replaced, not written through: the other link keeps the old
        // text (an atomic save can't do otherwise; documented).
        assert!(matches!(d.write("../x", b""), Err(FsError::BadPath(_))));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(root.join("shared"), b"old").unwrap();
            std::os::unix::fs::symlink(root.join("shared"), root.join("system/link")).unwrap();
            d.write("system/link", b"new").unwrap();
            assert!(d.is_link("system/link") && !d.is_link("shared") && !d.is_link("none"));
            std::os::unix::fs::symlink("nowhere", root.join("system/dangling")).unwrap();
            assert!(d.is_link("system/dangling"), "a link to nothing is a link");
            assert!(std::fs::symlink_metadata(root.join("system/link")).unwrap().file_type().is_symlink());
            assert_eq!(std::fs::read(root.join("shared")).unwrap(), b"new");
            // A link out of the case is refused, and the file it points to is untouched.
            let outside = std::env::temp_dir().join(format!("umcf-core-outside-{}", std::process::id()));
            std::fs::write(&outside, b"keep").unwrap();
            std::os::unix::fs::symlink(&outside, root.join("system/escape")).unwrap();
            assert!(matches!(d.write("system/escape", b"x"), Err(FsError::BadPath(_))));
            assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
            std::fs::remove_file(&outside).unwrap();
            // A symlink planted where the temporary file would go is not written through.
            let victim = std::env::temp_dir().join(format!("umcf-core-victim-{}", std::process::id()));
            std::fs::write(&victim, b"victim").unwrap();
            let next = TMP_COUNTER.load(std::sync::atomic::Ordering::Relaxed);
            for n in next..next + 5 {
                let planted = root.join(format!("system/.planted.umcfgui-tmp-{}-{n}", std::process::id()));
                std::os::unix::fs::symlink(&victim, planted).unwrap();
            }
            d.write("system/planted", b"new text").unwrap();
            assert_eq!(std::fs::read(&victim).unwrap(), b"victim");
            assert_eq!(std::fs::read(root.join("system/planted")).unwrap(), b"new text");
            assert!(!std::fs::symlink_metadata(root.join("system/planted")).unwrap().file_type().is_symlink());
            std::fs::remove_file(&victim).unwrap();
            // So is a linked directory leading out.
            let out_dir = std::env::temp_dir().join(format!("umcf-core-outdir-{}", std::process::id()));
            std::fs::create_dir_all(&out_dir).unwrap();
            std::os::unix::fs::symlink(&out_dir, root.join("constant")).unwrap();
            assert!(matches!(d.write("constant/air/g", b"x"), Err(FsError::BadPath(_))));
            assert!(std::fs::read_dir(&out_dir).unwrap().next().is_none(), "nothing created outside");
            std::fs::remove_dir_all(&out_dir).unwrap();
            std::fs::write(root.join("Allrun"), b"#!/bin/sh").unwrap();
            std::fs::set_permissions(root.join("Allrun"), std::fs::Permissions::from_mode(0o755)).unwrap();
            d.write("Allrun", b"#!/bin/sh\n").unwrap();
            assert_eq!(std::fs::metadata(root.join("Allrun")).unwrap().permissions().mode() & 0o777, 0o755);
        }
        std::fs::remove_dir_all(&root).unwrap();
        let mut m = MemFs::new();
        m.write("a/b", b"x").unwrap();
        assert!(m.is_file("a/b"));
        assert!(matches!(m.write("/abs", b""), Err(FsError::BadPath(_))));
    }

    #[test]
    fn memfs_files_and_dirs() {
        let fs = MemFs::new().with("system/controlDict", "a 1;").with("system/air/fvSchemes", "").with("0/air/U", "");
        assert!(fs.is_file("system/controlDict"));
        assert!(fs.is_dir("system") && fs.is_dir("system/air") && !fs.is_dir("constant"));
        assert_eq!(fs.list("").unwrap(), ["0", "system"]);
        assert_eq!(fs.list("system").unwrap(), ["air", "controlDict"]);
        assert!(fs.list("nope").unwrap().is_empty());
        assert_eq!(fs.read("system/controlDict", 100).unwrap().as_deref(), Some(&b"a 1;"[..]));
        assert!(fs.read("system/nope", 100).unwrap().is_none());
        assert!(matches!(fs.read("system/controlDict", 2), Err(FsError::TooLarge { .. })));
    }

    #[test]
    fn paths_cannot_leave_the_case() {
        let fs = MemFs::new();
        assert!(matches!(fs.read("../etc/passwd", 100), Err(FsError::BadPath(_))));
        assert!(matches!(fs.read("/etc/passwd", 100), Err(FsError::BadPath(_))));
        let d = DirFs::new("/tmp");
        assert!(matches!(d.read("../etc/passwd", 100), Err(FsError::BadPath(_))));
        assert!(!d.is_file("../etc/passwd"));
        assert!(matches!(d.list(".."), Err(FsError::BadPath(_))));
        assert!(matches!(fs.list("../x"), Err(FsError::BadPath(_))));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_is_an_error_not_empty() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("umcf-core-list-{}", std::process::id()));
        let locked = root.join("locked");
        std::fs::create_dir_all(locked.join("inner")).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = DirFs::new(&root).list("locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        // Root can read anything: then there is nothing to check.
        if let Ok(names) = &result {
            assert_eq!(names, &["inner"]);
            return;
        }
        assert!(matches!(result, Err(FsError::Io { .. })), "{result:?}");
    }

    #[test]
    fn dirfs_reads_missing_as_none() {
        let d = DirFs::new(env!("CARGO_MANIFEST_DIR"));
        assert!(d.is_file("Cargo.toml"));
        assert!(d.read("no-such-file", 10).unwrap().is_none());
        assert!(d.read("src", 10).unwrap().is_none());
        assert!(d.list("").unwrap().contains(&"src".to_string()));
        // Missing directories and files list as empty; they are not errors.
        assert!(d.list("no-such-dir").unwrap().is_empty());
        assert!(d.list("Cargo.toml").unwrap().is_empty());
        assert!(matches!(d.read("Cargo.toml", 3), Err(FsError::TooLarge { .. })));
    }
}
