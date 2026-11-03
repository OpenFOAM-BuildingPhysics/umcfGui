//! The run controls of `system/controlDict`.

use umcf_foam_dict::Document;
use umcf_foam_field::times::TimeDir;

/// The entries of `system/controlDict` that set up a run, as written (`$macro`s and all).
/// Missing entries are `None`; nothing is defaulted here, so an editor shows what the file says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Controls {
    pub application: Option<String>,
    /// `firstTime`, `startTime` or `latestTime`.
    pub start_from: Option<String>,
    pub start_time: Option<String>,
    /// `endTime`, `writeNow`, `noWriteNow` or `nextWrite`.
    pub stop_at: Option<String>,
    pub end_time: Option<String>,
    pub delta_t: Option<String>,
    pub write_control: Option<String>,
    pub write_interval: Option<String>,
}

impl Controls {
    pub fn read(doc: &Document) -> Controls {
        // Words may be written quoted (`startFrom "latestTime";`): OpenFOAM reads them the same.
        let v = |key: &str| doc.value(&[key]).map(|v| umcf_foam_dict::unquote(v.trim()).to_string());
        Controls {
            application: v("application"),
            start_from: v("startFrom"),
            start_time: v("startTime"),
            stop_at: v("stopAt"),
            end_time: v("endTime"),
            delta_t: v("deltaT"),
            write_control: v("writeControl"),
            write_interval: v("writeInterval"),
        }
    }

    /// An entry as a number, when it is one (`3600`, `1e5`); `None` for a missing entry or a
    /// `$macro`.
    pub fn number(value: Option<&String>) -> Option<f64> {
        value.and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
    }

    /// The time directory a run starts from, as OpenFOAM picks it: the first or the latest time,
    /// or the one at `startTime`. Without `startFrom`, OpenFOAM resumes from the latest time
    /// (`Time::setControls`); an unknown value is an error there, and gives `None` here.
    pub fn start_dir(&self, times: &[TimeDir]) -> Option<TimeDir> {
        match self.start_from.as_deref() {
            Some("firstTime") => times.first().cloned(),
            Some("latestTime") | None => times.last().cloned(),
            Some("startTime") => {
                // By name first (`startTime 3600;` and `3600/`), then by value: OpenFOAM names
                // time directories with 6 significant digits by default, which rounds a value
                // by at most 5e-6 of itself.
                let written = self.start_time.as_deref().map(str::trim)?;
                if let Some(d) = times.iter().find(|d| d.name == written) {
                    return Some(d.clone());
                }
                let t = Controls::number(self.start_time.as_ref())?;
                times
                    .iter()
                    .filter(|d| (d.value - t).abs() <= 5e-6 * d.value.abs().max(t.abs()))
                    .min_by(|a, b| (a.value - t).abs().total_cmp(&(b.value - t).abs()))
                    .cloned()
            }
            Some(_) => None,
        }
    }

    /// `startFrom` and `startTime` for messages: `startTime 0`, `latestTime`.
    pub fn describe_start(&self) -> String {
        match self.start_from.as_deref() {
            Some(s @ ("firstTime" | "latestTime")) => s.to_string(),
            None => "latestTime, the default".to_string(),
            Some("startTime") => format!("startTime {}", self.start_time.as_deref().unwrap_or("(not set)")),
            Some(other) => format!("startFrom {other}"),
        }
    }
}
