//! A patch's new name followed into the results (`patch.set` with `results`): the fields and
//! meshes of the other time directories, the `processor*` directories (their mesh, and their
//! fields at every time) and the data a `timeVaryingMappedFixedValue` condition finds by the
//! patch's name (`constant/<region>/boundaryData/<patch>`).
//!
//! These can be gigabytes, so they aren't kept in the case as its dictionaries are. The case
//! keeps the renames asked for, in order ([`Case::results`], which undo and redo carry as the
//! rest of the case), the disk state those already done; [`Case::save_results`] brings the
//! files to the case's: the renames not done yet forward, those undone since in reverse, file by
//! file, each read, edited (only the patch's entry; every other byte kept) and written back
//! atomically. No backup is kept of them: a rename back undoes it.

use umcf_core::{CaseFs, CaseWrite, FsError};
use umcf_foam_field::times;

use crate::{Case, Result, join, lock};

/// A patch renamed in the results of region `region` (`""`: a case of one region).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultRename {
    pub region: String,
    pub from: String,
    pub to: String,
}

impl ResultRename {
    fn reversed(&self) -> Self {
        ResultRename { region: self.region.clone(), from: self.to.clone(), to: self.from.clone() }
    }

    /// How [`Case::changed`] lists it until it is saved.
    pub(crate) fn label(&self) -> String {
        let at = if self.region.is_empty() { String::new() } else { format!("{}/", self.region) };
        format!("results: {at}{} renamed {}", self.from, self.to)
    }
}

/// The result files a rename of a region's patch goes into, found without reading them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultFiles {
    /// Field and mesh (`polyMesh/boundary`) files, case-relative.
    pub files: Vec<String>,
    /// Their size in bytes, where it can be told.
    pub bytes: u64,
    /// The time directories they are in (of the case and the processor directories, by name).
    pub times: Vec<String>,
    /// The processor directories.
    pub processors: Vec<String>,
    /// The `boundaryData/<patch>` directories.
    pub dirs: Vec<String>,
    /// Collated processor directories (`processors4`): not followed.
    pub collated: Vec<String>,
}

/// What [`Case::save_results`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultsSaved {
    /// The files rewritten.
    pub files: Vec<String>,
    /// The directories renamed (their new path).
    pub dirs: Vec<String>,
    /// Files and directories not changed though they may name the patch, and why.
    pub skipped: Vec<(String, String)>,
    /// The renames done (those undone since a save: reversed).
    pub renames: Vec<ResultRename>,
}

/// Where a region's files are under `base` (`""`, `processor3`, a time directory in them).
fn region_dir(base: &str, region: &str) -> String {
    join(base, region)
}

impl Case {
    /// The patch renames asked to be followed into the results, in order.
    pub fn results(&self) -> &[ResultRename] {
        &self.results
    }

    /// Asks the rename of `region`'s patch `from` to `to` to be followed into the results when
    /// the case is saved ([`Case::save_results`]).
    pub fn follow_results(&mut self, region: &str, from: &str, to: &str) {
        self.results.push(ResultRename { region: region.to_string(), from: from.to_string(), to: to.to_string() });
    }

    /// The renames [`Case::save_results`] would do now: those undone since a save, reversed
    /// (last first), then those not saved yet.
    pub fn results_pending(&self) -> Vec<ResultRename> {
        let (done, unfinished) = {
            let disk = lock(&self.disk);
            (disk.results.clone(), disk.results_unfinished.clone())
        };
        let same = done.iter().zip(&self.results).take_while(|(a, b)| a == b).count();
        let undone = done.get(same..).unwrap_or_default().iter().rev().map(ResultRename::reversed);
        // What a cancelled save was doing first, to finish it.
        unfinished
            .into_iter()
            .chain(undone)
            .chain(self.results.get(same..).unwrap_or_default().iter().cloned())
            .collect()
    }

    /// The result files of `region` a rename of its patch `patch` goes into: every time
    /// directory but the start (whose fields the case has), each processor directory's mesh
    /// and times, and the `boundaryData/<patch>` directories. Listed, not read.
    pub fn result_files(&self, fs: &dyn CaseFs, region: &str, patch: &str) -> Result<ResultFiles> {
        let mut out = ResultFiles::default();
        let start = self.start.as_ref().map(|t| t.name.clone());
        let mut bases = vec![String::new()];
        for (_, p) in times::processor_dirs(fs)? {
            out.processors.push(p.clone());
            bases.push(p);
        }
        // Collated (`processors4`, `processors4_0-1`): one file for all processors, not followed.
        out.collated = fs.list("")?.into_iter().filter(|n| n.starts_with("processors") && fs.is_dir(n)).collect();
        for base in &bases {
            let constant = region_dir(&join(base, "constant"), region);
            if !base.is_empty() {
                out.files.extend(boundary_file(fs, &join(&constant, "polyMesh")));
            }
            let data = join(&join(&constant, "boundaryData"), patch);
            if !patch.is_empty() && fs.is_dir(&data) {
                out.dirs.push(data);
            }
            for t in times::time_dirs(fs, base)? {
                if base.is_empty() && Some(&t.name) == start.as_ref() {
                    continue;
                }
                let dir = region_dir(&join(base, &t.name), region);
                if !fs.is_dir(&dir) {
                    continue;
                }
                out.times.push(join(base, &t.name));
                // Every file (`U` and a `U.gz` beside it both), not a backup or a hidden one.
                for name in fs.list(&dir)? {
                    let f = join(&dir, &name);
                    if !name.starts_with('.') && !name.ends_with('~') && !name.contains(".orig") && fs.is_file(&f) {
                        out.files.push(f);
                    }
                }
                out.files.extend(boundary_file(fs, &join(&dir, "polyMesh")));
            }
        }
        out.bytes = out.files.iter().filter_map(|f| fs.size(f)).sum();
        Ok(out)
    }

    /// Brings the results to the renames the case asks for ([`Case::results_pending`]), in
    /// order: each region's result files ([`Case::result_files`]) read one at a time, the
    /// patch's entry renamed in a field's `boundaryField` and a mesh's `boundary`, and the
    /// patches of every region's result meshes that name it as their partner (`neighbourPatch`,
    /// `samplePatch`, `originalPatch`) following (every other byte kept), written back
    /// atomically; then the `boundaryData/<patch>` directories renamed. Run after
    /// [`Case::save`].
    ///
    /// Once the first file is written nothing stops it but a cancel: a file bigger than `cap`
    /// (as read, and decompressed), a link, one that can't be read, written or written back
    /// byte for byte, and a directory whose new name is taken or that can't be renamed, are
    /// skipped and listed, each saying that its time still has the old name there; the
    /// renames count as done, so an undo takes back what was done and only that. A cancel
    /// keeps how far each file and the directories got: the next call gives each only the
    /// renames it hasn't had (then those asked since), whatever they are (a swap through a
    /// third name too). That record is the open case's only (its shared disk state): a case
    /// opened again has none. An error before (listing the directories) changes nothing.
    pub fn save_results(&mut self, fs: &mut dyn CaseWrite, cap: u64) -> Result<ResultsSaved> {
        let ops = self.results_pending();
        let mut saved = ResultsSaved { renames: ops.clone(), ..ResultsSaved::default() };
        if ops.is_empty() {
            return Ok(saved);
        }
        // How far a cancelled call got with the first of them.
        let (mut progress, dirs_from) = {
            let disk = lock(&self.disk);
            (disk.results_progress.clone(), disk.results_dirs_done)
        };
        // Each file once: the renamed regions' fields and meshes, every region's meshes (their
        // patches may be partners), with the region it is of. All listed before any is written.
        let mut regions: Vec<String> = self.regions.iter().map(|r| r.name.clone()).collect();
        if regions.is_empty() {
            regions.push(String::new());
        }
        let mut jobs: Vec<(String, String)> = Vec::new();
        for region in &regions {
            let renamed = ops.iter().any(|o| &o.region == region);
            for f in self.result_files(&*fs, region, "")?.files {
                // Each region's files are under its own directories: no file comes twice.
                if renamed || f.contains("/polyMesh/") {
                    jobs.push((f, region.clone()));
                }
            }
        }
        let limits = umcf_foam_field::Limits { max_bytes: cap, ..Default::default() };
        let mut skipped: Vec<(String, String)> = Vec::new();
        let names = |region: &str| -> String {
            let mut olds: Vec<&str> = ops.iter().filter(|o| o.region == region).map(|o| o.from.as_str()).collect();
            olds.dedup();
            if olds.is_empty() { "its partners' old names".to_string() } else { format!("`{}`", olds.join("`, `")) }
        };
        let mut skip = |f: String, region: &str, why: String| {
            let why = format!(
                "{why}: it may still name {}, so a run from {} needs it renamed there by hand",
                names(region),
                at_time(&f)
            );
            skipped.push((f, why));
        };
        for (f, region) in jobs {
            let todo = ops.get(progress.get(&f).copied().unwrap_or(0)..).unwrap_or_default();
            if todo.is_empty() {
                continue;
            }
            if fs.is_link(&f) {
                skip(f.clone(), &region, "it is a link (the file it points to may be another case's)".to_string());
                progress.insert(f, ops.len());
                continue;
            }
            let bytes = match fs.read(&f, cap) {
                Ok(Some(b)) => b,
                Ok(None) => continue,
                Err(FsError::TooLarge { size, .. }) => {
                    skip(
                        f.clone(),
                        &region,
                        format!("it is {}, more than the memory to edit it allows ({})", shown(size), shown(cap)),
                    );
                    progress.insert(f, ops.len());
                    continue;
                }
                Err(e @ FsError::Cancelled { .. }) => return Err(self.results_cancelled(ops, progress, dirs_from, e)),
                Err(e) => {
                    skip(f.clone(), &region, format!("it can't be read ({e})"));
                    progress.insert(f, ops.len());
                    continue;
                }
            };
            // Plain text that names none of them has nothing to rename, nor a file that isn't
            // OpenFOAM's (gzip: read through).
            let plain = !bytes.starts_with(&[0x1f, 0x8b]);
            let head = bytes.get(..4096).unwrap_or(&bytes);
            if plain && (!mentions(head, "FoamFile") || !todo.iter().any(|o| mentions(&bytes, &o.from))) {
                progress.insert(f, ops.len());
                continue;
            }
            let mesh = f.contains("/polyMesh/");
            let mut changed = false;
            // In order, in memory: a swap through a third name comes out a swap.
            let out = umcf_foam_field::rewrite(&bytes, &limits, |doc| {
                for op in todo {
                    let own = op.region == region;
                    if mesh {
                        changed |= follow_partners(doc, &region, op)?;
                        if own {
                            changed |= rename_patch(doc, &op.from, &op.to)?;
                        }
                    } else if own {
                        changed |= rename_field_entry(doc, &op.from, &op.to)?;
                    }
                }
                Ok(())
            });
            drop(bytes);
            match out {
                Ok(out) if changed => match fs.write(&f, &out) {
                    Ok(()) => saved.files.push(f.clone()),
                    Err(e) => skip(f.clone(), &region, format!("it can't be written ({e})")),
                },
                Ok(_) => {}
                Err(e) => skip(f.clone(), &region, e.to_string()),
            }
            progress.insert(f, ops.len());
        }
        saved.skipped = skipped;
        // The data found by the patch's name, in order (a rename back moves it back).
        for (k, op) in ops.iter().enumerate().skip(dirs_from) {
            let dirs = match self.result_files(&*fs, &op.region, &op.from) {
                Ok(found) => found.dirs,
                Err(crate::Error::Fs(e @ FsError::Cancelled { .. })) => {
                    return Err(self.results_cancelled(ops.clone(), progress, k, e));
                }
                Err(e) => {
                    saved.skipped.push((format!("boundaryData/{}", op.from), format!("can't be looked for ({e})")));
                    continue;
                }
            };
            for dir in dirs {
                let to = match dir.rsplit_once('/') {
                    Some((parent, _)) => join(parent, &op.to),
                    None => op.to.clone(),
                };
                let why = if fs.is_dir(&to) || fs.is_file(&to) {
                    format!("{to} is there already")
                } else if fs.is_link(&dir) {
                    "it is a link".to_string()
                } else {
                    match fs.rename_dir(&dir, &to) {
                        Ok(()) => {
                            saved.dirs.push(to);
                            continue;
                        }
                        Err(e) => format!("it can't be renamed ({e})"),
                    }
                };
                saved.skipped.push((dir, format!("{why}: a condition that finds its data by the patch's name (timeVaryingMappedFixedValue) needs it renamed {} by hand", op.to)));
            }
        }
        let mut disk = lock(&self.disk);
        disk.results = self.results.clone();
        disk.results_unfinished.clear();
        disk.results_progress.clear();
        disk.results_dirs_done = 0;
        drop(disk);
        Ok(saved)
    }

    /// A cancelled [`Case::save_results`]: the renames it was doing are kept to be finished
    /// (by the next save, or taken back by an undo and a save), with how far each file and the
    /// directories got; never counted as done.
    fn results_cancelled(
        &self,
        doing: Vec<ResultRename>,
        progress: std::collections::BTreeMap<String, usize>,
        dirs_done: usize,
        e: FsError,
    ) -> crate::Error {
        let mut disk = lock(&self.disk);
        disk.results = self.results.clone();
        disk.results_unfinished = doing;
        disk.results_progress = progress;
        disk.results_dirs_done = dirs_done;
        e.into()
    }
}

/// The time a result file is of, for "a run from …": `3600`, `processor0/3600`, or any time of
/// `processor0` for its decomposed mesh (in its `constant`).
fn at_time(f: &str) -> String {
    let mut parts = f.split('/');
    match (parts.next(), parts.next()) {
        (Some(p), Some("constant")) if p.starts_with("processor") => format!("any time of {p}"),
        (Some(p), Some(t)) if p.starts_with("processor") => format!("{p}/{t}"),
        (Some(t), _) => t.to_string(),
        _ => f.to_string(),
    }
}

/// The patches of a mesh of region `region` that name `op`'s patch as their partner, made to
/// name its new name: `neighbourPatch` and `samplePatch` of a patch whose `neighbourRegion` or
/// `sampleRegion` is its region (its own region without one: a `cyclic`), `originalPatch` in
/// its own region. As [`Case::set_patch`] does for the case's mesh.
fn follow_partners(
    doc: &mut umcf_foam_dict::Document,
    region: &str,
    op: &ResultRename,
) -> umcf_foam_field::Result<bool> {
    let mut changed = false;
    for name in umcf_foam_mesh::boundary_names(doc) {
        let own = region == op.region;
        if own && (name == op.from || name == op.to) {
            continue;
        }
        let Some((_, dict)) = umcf_foam_mesh::boundary_patch_mut(doc, &name) else { continue };
        let get = |k: &str| {
            dict.get(k)
                .and_then(umcf_foam_dict::Entry::value_text)
                .map(|t| umcf_foam_dict::unquote(t.trim()).to_string())
        };
        let partner = get("neighbourRegion").or_else(|| get("sampleRegion")).unwrap_or_else(|| region.to_string());
        let mut keys: Vec<&str> = Vec::new();
        if partner == op.region {
            keys.extend(["neighbourPatch", "samplePatch"].into_iter().filter(|k| get(k).as_deref() == Some(&op.from)));
        }
        if own && get("originalPatch").as_deref() == Some(&op.from) {
            keys.push("originalPatch");
        }
        for k in keys {
            dict.set(&[k], &op.to)?;
            changed = true;
        }
    }
    Ok(changed)
}

/// The mesh's `boundary` file in `dir`, plain or compressed, when there is one.
fn boundary_file(fs: &dyn CaseFs, dir: &str) -> Option<String> {
    let plain = join(dir, "boundary");
    let gz = format!("{plain}.gz");
    [plain, gz].into_iter().find(|p| fs.is_file(p))
}

/// Whether `bytes` has `word` as a word (not inside a longer name).
fn mentions(bytes: &[u8], word: &str) -> bool {
    let w = word.as_bytes();
    let inside = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b':');
    bytes.windows(w.len().max(1)).enumerate().any(|(i, s)| {
        s == w
            && i.checked_sub(1).and_then(|k| bytes.get(k)).is_none_or(|&b| !inside(b))
            && bytes.get(i + w.len()).is_none_or(|&b| !inside(b))
    })
}

/// Renames `from` to `to` among a field's `boundaryField` entries; `false` when it has no such
/// entry. Refused (the file skipped) when it has `to` too, or `from` twice.
fn rename_field_entry(doc: &mut umcf_foam_dict::Document, from: &str, to: &str) -> umcf_foam_field::Result<bool> {
    let Some(bf) = doc.root.get_mut("boundaryField").and_then(umcf_foam_dict::Entry::dict_mut) else {
        return Ok(false);
    };
    let count = |k: &str| bf.entries().filter(|e| umcf_foam_dict::unquote(&e.key.text) == k).count();
    match (count(from), count(to)) {
        (0, _) => Ok(false),
        (1, 0) => Ok(bf.rename(from, to)),
        (1, _) => Err(unsupported(format!("it has entries `{from}` and `{to}`: rename one by hand"))),
        _ => Err(unsupported(format!("it has `{from}` twice: keep one first"))),
    }
}

/// Renames the patch `from` to `to` in a mesh's `boundary`; `false` when it has no such patch.
/// Refused when it has a patch `to` too.
fn rename_patch(doc: &mut umcf_foam_dict::Document, from: &str, to: &str) -> umcf_foam_field::Result<bool> {
    if umcf_foam_mesh::boundary_patch_mut(doc, to).is_some() {
        return if umcf_foam_mesh::boundary_patch_mut(doc, from).is_some() {
            Err(unsupported(format!("it has patches `{from}` and `{to}`")))
        } else {
            Ok(false)
        };
    }
    match umcf_foam_mesh::boundary_patch_mut(doc, from) {
        Some((name, _)) => {
            name.text = to.to_string();
            Ok(true)
        }
        None => Ok(false),
    }
}

/// A size in bytes as people read it: `3 kB`, `600 MB`.
fn shown(bytes: u64) -> String {
    match bytes {
        b if b >= 10 << 30 => format!("{} GB", b >> 30),
        b if b >= 10 << 20 => format!("{} MB", b >> 20),
        b if b >= 10 << 10 => format!("{} kB", b >> 10),
        b => format!("{b} bytes"),
    }
}

fn unsupported(why: String) -> umcf_foam_field::Error {
    umcf_foam_field::Error::Unsupported(why)
}

#[cfg(test)]
#[path = "results_tests.rs"]
mod tests;
