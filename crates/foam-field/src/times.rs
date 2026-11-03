//! Time and processor directories of a case.

use umcf_core::{CaseFs, FsError};

/// A time directory: its name as written (`3600`, `1e+06`, `0.5`) and its value.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeDir {
    pub name: String,
    pub value: f64,
}

/// Time directories directly under `base` (`""` for the case, `processor3` for one processor),
/// sorted by time. `0.orig`, `constant`, `system` and the like are not times.
pub fn time_dirs(fs: &dyn CaseFs, base: &str) -> Result<Vec<TimeDir>, FsError> {
    let join = |n: &str| if base.is_empty() { n.to_string() } else { format!("{base}/{n}") };
    let mut out: Vec<TimeDir> = fs
        .list(base)?
        .into_iter()
        .filter(|n| n.bytes().next().is_some_and(|b| b.is_ascii_digit() || b == b'-' || b == b'.'))
        .filter_map(|n| {
            let value: f64 = n.parse().ok().filter(|v: &f64| v.is_finite())?;
            fs.is_dir(&join(&n)).then_some(TimeDir { name: n, value })
        })
        .collect();
    out.sort_by(|a, b| a.value.total_cmp(&b.value));
    Ok(out)
}

/// `processorN` directories, sorted by N. A collated case (`processors4`) has one directory
/// holding every processor; it is reported by [`collated_dirs`] instead.
pub fn processor_dirs(fs: &dyn CaseFs) -> Result<Vec<(usize, String)>, FsError> {
    let mut out: Vec<(usize, String)> = fs
        .list("")?
        .into_iter()
        .filter_map(|n| {
            let k: usize = n.strip_prefix("processor")?.parse().ok()?;
            fs.is_dir(&n).then_some((k, n))
        })
        .collect();
    out.sort();
    Ok(out)
}

/// `processorsN` directories of the collated file format.
pub fn collated_dirs(fs: &dyn CaseFs) -> Result<Vec<String>, FsError> {
    Ok(fs
        .list("")?
        .into_iter()
        .filter(|n| n.strip_prefix("processors").is_some_and(|k| k.parse::<usize>().is_ok()))
        .collect())
}

/// Field files in `<time>/<region>` (or `<time>` for a single-region case): file names with any
/// `.gz` removed, sorted, without subdirectories such as `uniform/`.
pub fn fields(fs: &dyn CaseFs, dir: &str) -> Result<Vec<String>, FsError> {
    let mut out: Vec<String> = fs
        .list(dir)?
        .into_iter()
        .filter(|n| fs.is_file(&format!("{dir}/{n}")))
        .map(|n| n.strip_suffix(".gz").map(str::to_string).unwrap_or(n))
        .filter(|n| !n.ends_with(".orig") && !n.starts_with('.'))
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use umcf_core::MemFs;

    use super::*;

    #[test]
    fn times_sorted_by_value() {
        let fs = MemFs::new()
            .with("0/air/U", "")
            .with("0.orig/air/U", "")
            .with("3600/air/U.gz", "")
            .with("1e+06/air/U", "")
            .with("0.5/air/U", "")
            .with("constant/g", "")
            .with("processor1/3600/air/U", "")
            .with("processor0/0/air/U", "")
            .with("processor10/0/air/U", "")
            .with("processors4/0/air/U", "")
            .with("123abc", "");
        let names: Vec<_> = time_dirs(&fs, "").unwrap().into_iter().map(|t| t.name).collect();
        assert_eq!(names, ["0", "0.5", "3600", "1e+06"]);
        assert_eq!(time_dirs(&fs, "processor1").unwrap()[0].name, "3600");
        let procs: Vec<usize> = processor_dirs(&fs).unwrap().into_iter().map(|p| p.0).collect();
        assert_eq!(procs, [0, 1, 10]);
        assert_eq!(collated_dirs(&fs).unwrap(), ["processors4"]);
        assert!(time_dirs(&fs, "../x").is_err());
    }

    #[test]
    fn field_names() {
        let fs = MemFs::new()
            .with("3600/air/U.gz", "")
            .with("3600/air/T", "")
            .with("3600/air/T.gz", "")
            .with("3600/air/p.orig", "")
            .with("3600/air/uniform/time", "");
        assert_eq!(fields(&fs, "3600/air").unwrap(), ["T", "U"]);
    }
}
