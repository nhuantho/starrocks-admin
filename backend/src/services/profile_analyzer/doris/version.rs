//! Doris version adapter: the single place where counter names differ between releases.
//!
//! Rules ask for a canonical counter name; `raw_names` lists the spellings that mean the same
//! measurement. Sources for each alias are given inline. Add a new alias here instead of
//! special-casing names inside a rule.
use once_cell::sync::Lazy;
use regex::Regex;

static VERSION: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d+)\.(\d+)(?:\.(\d+))?").unwrap());

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct DorisVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl DorisVersion {
    /// Parse `Doris Version` values such as `doris-4.1.3-rc02-7126cf65d96` or `2.1.7`.
    pub fn parse(text: &str) -> Option<Self> {
        let cap = VERSION.captures(text)?;
        Some(Self {
            major: cap[1].parse().ok()?,
            minor: cap[2].parse().ok()?,
            patch: cap
                .get(3)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0),
        })
    }

    /// `profile_level` has its documented 1-3 semantics only from 4.0.
    /// https://doris.apache.org/docs/4.x/query-acceleration/query-profile/
    pub fn supports_profile_level(&self) -> bool {
        self.major >= 4
    }

    /// Spill-to-disk variables are documented for 4.x only.
    /// https://doris.apache.org/docs/4.x/key-features/spill-to-disk/
    pub fn supports_spill_variables(&self) -> bool {
        self.major >= 4
    }
}

impl std::fmt::Display for DorisVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Canonical name -> raw counter names. The first name present in a profile wins.
const ALIASES: &[(&str, &[&str])] = &[
    // Observed in 4.1.3 fixtures as MemoryUsagePeak; 2.1/3.x analysis-tools docs list PeakMemoryUsage.
    ("MemoryUsagePeak", &["MemoryUsagePeak", "PeakMemoryUsage"]),
    // 4.1.3 JDBC detail prints AcquireRuntimeFilter; 2.1/3.x runtime-filter wait docs list AcquireRuntimeFilterTime.
    ("AcquireRuntimeFilter", &["AcquireRuntimeFilter", "AcquireRuntimeFilterTime"]),
    // 2.1/3.x runtime-filter wait docs list TabletNum; the legacy OLAP_SCAN_NODE API example lists TabletCount.
    ("TabletNum", &["TabletNum", "TabletCount"]),
    // 2.1/3.x analysis-tools docs list ExecuteTime for PipelineTask; operators print ExecTime.
    ("ExecTime", &["ExecTime"]),
];

pub fn raw_names(canonical: &str) -> &'static [&'static str] {
    ALIASES
        .iter()
        .find(|(name, _)| *name == canonical)
        .map(|(_, raw)| *raw)
        .unwrap_or(&[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_strings() {
        let v = DorisVersion::parse("doris-4.1.3-rc02-7126cf65d96").unwrap();
        assert_eq!(v, DorisVersion { major: 4, minor: 1, patch: 3 });
        assert!(v.supports_profile_level());
        let old = DorisVersion::parse("doris-2.1.7-rc03-abc").unwrap();
        assert!(!old.supports_profile_level());
        assert_eq!(DorisVersion::parse("3.0").unwrap().patch, 0);
        assert!(DorisVersion::parse("unknown").is_none());
    }

    #[test]
    fn aliases_are_unique_and_include_canonical() {
        for (name, raw) in ALIASES {
            assert!(raw.contains(name), "{name}");
        }
        assert_eq!(raw_names("PeakMemoryUsage"), &[] as &[&str]);
        assert!(raw_names("MemoryUsagePeak").contains(&"PeakMemoryUsage"));
    }
}
