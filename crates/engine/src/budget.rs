//! The memory a session plans with: how much what is read at once may take, and how much the
//! 3D scene keeps of what isn't shown.
//!
//! Taken from the memory free when the first session starts, as Linux, Windows or macOS tell
//! it (8 GB where it can't be told), or from `UMCF_MEMORY` (megabytes) when it is set. Half of
//! it is for reading at once, a quarter for hidden scene objects. These are plans from
//! estimates, not limits: a single read bigger than the budget still runs, alone.

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

    /// The budget as the commands report it: megabytes.
    pub fn to_json(self) -> serde_json::Value {
        serde_json::json!({ "read_mb": self.read >> 20, "spare_mb": self.spare >> 20 })
    }
}

/// The memory free now, as the system tells it (Linux, Windows, macOS), else [`FALLBACK`].
fn free_memory() -> u64 {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    match system.available_memory() {
        0 => FALLBACK,
        n => n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_tells_its_free_memory() {
        // Linux, Windows and macOS all tell it; CI runs this on each.
        assert!(free_memory() > 0);
    }

    #[test]
    fn the_budget_splits_the_memory() {
        assert_eq!(Budget::of(8 << 30), Budget { read: 4 << 30, spare: 2 << 30 });
    }
}
