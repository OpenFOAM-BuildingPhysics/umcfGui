//! The memory a session plans with: how much what is read at once may take, and how much the
//! 3D scene keeps of what isn't shown.
//!
//! Taken once per process, when the first session starts, from the memory free as Linux,
//! Windows or macOS tell it (on Linux no more than the process's cgroup v2 groups leave: a
//! container, a SLURM job; 8 GB where it can't be told), or from `UMCF_MEMORY` (a whole number
//! of megabytes) when it is set; another value of it is ignored. Half of it is for reading at
//! once, a quarter for hidden scene objects. These are plans from estimates, not limits: a
//! single read bigger than the budget still runs, alone.

/// What [`Budget::system`] assumes where the free memory can't be told.
const FALLBACK: u64 = 8 << 30;

/// How much memory the session plans for (bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// The most the readers running at once may hold, by their estimates. One reader always
    /// runs, whatever its estimate.
    pub read: u64,
    /// The most the 3D scene keeps of objects it isn't showing; past it, the ones shown least
    /// recently are dropped (and read again when shown).
    pub spare: u64,
}

impl Budget {
    /// The budget for `memory` bytes: half to read at once, a quarter for hidden objects.
    pub fn of(memory: u64) -> Budget {
        Budget { read: memory / 2, spare: memory / 4 }
    }

    /// The budget for this machine: `UMCF_MEMORY` (megabytes) when set to a number, else the
    /// memory free when the first session started (found once per process).
    pub fn system() -> Budget {
        static SYSTEM: std::sync::OnceLock<Budget> = std::sync::OnceLock::new();
        *SYSTEM.get_or_init(|| {
            let set = std::env::var("UMCF_MEMORY").ok().and_then(|v| v.trim().parse::<u64>().ok());
            Budget::of(set.map_or_else(free_memory, |mb| mb.saturating_mul(1 << 20)))
        })
    }
}

/// The memory free now, as the system tells it (Linux, Windows, macOS), and on Linux no more
/// than the process's control group leaves (see [`cgroup_free`]); else [`FALLBACK`]. A failure
/// of the system's crate falls back too: the budget is a plan, never a reason to stop.
fn free_memory() -> u64 {
    let system = std::panic::catch_unwind(|| {
        let mut system = sysinfo::System::new();
        system.refresh_memory();
        system.available_memory()
    });
    let free = match system {
        Ok(0) | Err(_) => FALLBACK,
        Ok(n) => n,
    };
    #[cfg(target_os = "linux")]
    if let Some(own) = std::fs::read_to_string("/proc/self/cgroup").ok()
        && let Some(mounts) = std::fs::read_to_string("/proc/self/mountinfo").ok()
        && let Some(left) = cgroup_free(&own, &mounts, |p| std::fs::read_to_string(p).ok())
    {
        return free.min(left);
    }
    free
}

/// What the control groups (v2) the process is in leave free: for each of them from its own up to
/// the root that sets `memory.max`, the limit less what is used (`memory.current` without the
/// page cache the kernel can drop: `file` less `shmem` in `memory.stat`, as shared memory and
/// tmpfs stay), the least of them. `own` is `/proc/self/cgroup`, `mounts` is
/// `/proc/self/mountinfo` (where cgroup2 file systems are mounted, and which group each one's
/// root is: a container may see only its own part of the tree; the group is read through the
/// mount whose root holds it, the longest), `read` reads a file. `None` when none sets a limit
/// (no cgroup v2, no mount that holds the group, or no limit: a desktop session).
///
/// This is how a SLURM job's, a container's or a `systemd-run -p MemoryMax=` limit is found,
/// set on the job's group while the process runs in a group below it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn cgroup_free(own: &str, mounts: &str, read: impl Fn(&str) -> Option<String>) -> Option<u64> {
    let path = own.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
    if !path.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let all = cgroup2_mounts(mounts);
    // The mount the group is seen through, and the group's parts below the mount's root: the
    // mount whose root is the longest that holds the group. A group outside the namespace
    // (`/../x`) sees only the first mount's own group; one no mount holds, none.
    let (point, below): (&str, &[&str]) = if parts.contains(&"..") {
        (all.first()?.1, &[])
    } else {
        all.iter()
            .filter_map(|(root, point)| {
                let root: Vec<&str> = root.split('/').filter(|p| !p.is_empty()).collect();
                parts.starts_with(&root).then_some((root.len(), *point))
            })
            // The longest root; of equal ones, the first listed.
            .min_by_key(|(n, _)| std::cmp::Reverse(*n))
            .map(|(n, point)| (point, parts.get(n..).unwrap_or(&[])))?
    };
    let point = point.trim_end_matches('/');
    let number = |file: &str| read(file).and_then(|t| t.trim().parse::<u64>().ok());
    let mut least: Option<u64> = None;
    // The process's group first, then each one above it up to the mount's own.
    for k in (0..=below.len()).rev() {
        let mut dir = point.to_string();
        for part in below.get(..k).unwrap_or(&[]) {
            dir.push('/');
            dir.push_str(part);
        }
        // `max` (no limit) doesn't parse: that group doesn't count.
        if let Some(max) = number(&format!("{dir}/memory.max")) {
            let current = number(&format!("{dir}/memory.current")).unwrap_or(0);
            let stat = read(&format!("{dir}/memory.stat")).unwrap_or_default();
            let entry = |key: &str| {
                stat.lines()
                    .find_map(|l| l.strip_prefix(key)?.strip_prefix(' ')?.trim().parse::<u64>().ok())
                    .unwrap_or(0)
            };
            let cache = entry("file").saturating_sub(entry("shmem"));
            let left = max.saturating_sub(current.saturating_sub(cache));
            least = Some(least.map_or(left, |l| l.min(left)));
        }
    }
    least
}

/// The root and the mount point of each cgroup2 file system in `/proc/self/mountinfo`'s text
/// (`id parent dev root point options … - cgroup2 source options`), in its order; one whose
/// paths have escaped characters (`\040` for a space) is passed over.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn cgroup2_mounts(mounts: &str) -> Vec<(&str, &str)> {
    mounts
        .lines()
        .filter_map(|l| {
            let (fields, kind) = l.split_once(" - ")?;
            if kind.split_whitespace().next() != Some("cgroup2") {
                return None;
            }
            let mut f = fields.split_whitespace().skip(3);
            let (root, point) = (f.next()?, f.next()?);
            (root.starts_with('/') && point.starts_with('/') && !root.contains('\\') && !point.contains('\\'))
                .then_some((root, point))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_tells_its_free_memory() {
        // Linux, Windows and macOS all tell it; CI runs this on each. Asked directly: the
        // fallback would hide a failure.
        let mut system = sysinfo::System::new();
        system.refresh_memory();
        assert!(system.available_memory() > 0);
    }

    /// The cgroup2 file system mounted as on a host (the whole tree at `/sys/fs/cgroup`).
    const HOST: &str = "43 1 259:5 / / rw - xfs /dev/nvme0n1p3 rw\n47 45 0:29 / /sys/fs/cgroup rw,nosuid shared:7 - cgroup2 cgroup2 rw,nsdelegate\n";

    /// A tree of control groups as files: `(path, text)`.
    fn groups(files: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let files: Vec<(String, String)> = files.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect();
        move |p| files.iter().find(|(q, _)| q == p).map(|(_, t)| t.clone())
    }

    #[test]
    fn a_job_limit_above_the_process_group_counts() {
        // SLURM: the limit is on the job's group, the process runs in a task's group below it.
        let own = "0::/system.slice/slurmstepd.scope/job_7/step_0/user/task_0\n";
        let job = "/sys/fs/cgroup/system.slice/slurmstepd.scope/job_7";
        let read = groups(&[
            (&format!("{job}/memory.max"), "4294967296\n"),
            (&format!("{job}/memory.current"), "1073741824\n"),
            (
                &format!("{job}/memory.stat"),
                "anon 1\nfile_mapped 999\nshmem_thp 999\nfile 805306368\nshmem 268435456\ninactive_file 2\n",
            ),
            (&format!("{job}/step_0/user/task_0/memory.max"), "max\n"),
        ]);
        // 4 GB less 1 GB used, of which 0.75 GB is page cache, 0.25 GB of it shared memory that
        // stays: 0.5 GB can be dropped.
        assert_eq!(cgroup_free(own, HOST, read), Some(3584 << 20));
    }

    #[test]
    fn the_tightest_group_counts_and_a_full_one_leaves_nothing() {
        let own = "0::/a/b\n";
        let read = groups(&[
            ("/sys/fs/cgroup/a/memory.max", "1000\n"),
            ("/sys/fs/cgroup/a/memory.current", "100\n"),
            ("/sys/fs/cgroup/a/b/memory.max", "500\n"),
            ("/sys/fs/cgroup/a/b/memory.current", "600\n"),
        ]);
        assert_eq!(cgroup_free(own, HOST, read), Some(0));
    }

    #[test]
    fn no_limit_or_no_cgroup_v2_is_none() {
        // A desktop session: no group sets a limit.
        let read = groups(&[("/sys/fs/cgroup/user.slice/memory.max", "max\n")]);
        assert_eq!(cgroup_free("0::/user.slice/user-1000.slice/app.scope\n", HOST, read), None);
        // cgroup v1 only: no `0::` line.
        assert_eq!(cgroup_free("4:memory:/user.slice\n", HOST, groups(&[])), None);
        // The root group itself.
        let root = || groups(&[("/sys/fs/cgroup/memory.max", "100\n")]);
        assert_eq!(cgroup_free("0::/\n", HOST, root()), Some(100));
        // Outside the namespace's root: the root's limit.
        assert_eq!(cgroup_free("0::/../etc\n", HOST, root()), Some(100));
        assert_eq!(cgroup_free("0::etc\n", HOST, groups(&[])), None);
        // No cgroup2 mount.
        assert_eq!(cgroup_free("0::/\n", "43 1 259:5 / / rw - xfs /dev/x rw\n", root()), None);
    }

    #[test]
    fn a_mount_of_part_of_the_tree_is_resolved_from_its_root() {
        // `/jobs/job7` mounted at `/sys/fs/cgroup`: the task's group `/jobs/job7/task0` is
        // `/sys/fs/cgroup/task0` there, below the job's limit at the mount itself.
        let mounts = "47 45 0:29 /jobs/job7 /sys/fs/cgroup rw - cgroup2 cgroup2 rw\n";
        let read = || {
            groups(&[
                ("/sys/fs/cgroup/memory.max", "1000\n"),
                ("/sys/fs/cgroup/task0/memory.max", "600\n"),
                ("/sys/fs/cgroup/task0/memory.current", "100\n"),
            ])
        };
        assert_eq!(cgroup_free("0::/jobs/job7/task0\n", mounts, read()), Some(500));
        // A group the mount doesn't hold: not seen through it.
        assert_eq!(cgroup_free("0::/jobs/job8\n", mounts, read()), None);
        assert_eq!(cgroup_free("0::/jobs/job7x\n", mounts, read()), None);
        // Outside the namespace: the mount's own group.
        assert_eq!(cgroup_free("0::/../x\n", mounts, read()), Some(1000));
        // Mounted elsewhere.
        let other = "47 45 0:29 / /run/cg rw - cgroup2 none rw\n";
        assert_eq!(cgroup_free("0::/a\n", other, groups(&[("/run/cg/a/memory.max", "7\n")])), Some(7));
    }

    #[test]
    fn the_mount_that_holds_the_group_counts() {
        // A bind mount of another group listed first: the whole tree's mount holds the process.
        let mounts = "50 45 0:29 /system.slice/foo /mnt/cg rw - cgroup2 cgroup2 rw\n47 45 0:29 / /sys/fs/cgroup rw - cgroup2 cgroup2 rw\n";
        let read = groups(&[("/mnt/cg/memory.max", "5\n"), ("/sys/fs/cgroup/user.slice/memory.max", "900\n")]);
        assert_eq!(cgroup_free("0::/user.slice/x\n", mounts, read), Some(900));
        // Mounted at `/`: the root group is read too.
        let at_root = "47 45 0:29 / / rw - cgroup2 cgroup2 rw\n";
        let read = groups(&[("/memory.max", "1000\n"), ("/a/b/memory.max", "max\n")]);
        assert_eq!(cgroup_free("0::/a/b\n", at_root, read), Some(1000));
    }

    #[test]
    fn the_budget_splits_the_memory() {
        assert_eq!(Budget::of(8 << 30), Budget { read: 4 << 30, spare: 2 << 30 });
    }
}
