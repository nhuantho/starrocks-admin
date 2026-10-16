//! Typed access to Doris profile counters.
//!
//! Every value format below was observed in `tests/fixtures/profiles/doris-4.1.3-*.txt`
//! (see `docs/profile/doris-counter-catalog.md`). Unknown or unparseable values are `None`,
//! never zero.
use once_cell::sync::Lazy;
use regex::Regex;

static DURATION: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(?:\d+(?:\.\d+)?(?:hour|min|sec|ms|us|μs|ns|h|m|s))+$").unwrap());
static DURATION_PART: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(\d+(?:\.\d+)?)(hour|min|sec|ms|us|μs|ns|h|m|s)").unwrap());
static BYTES: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(\d+(?:\.\d+)?)\s*(B|KB|MB|GB|TB|PB)$").unwrap());
static COUNT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([\d,]+(?:\.\d+)?)\s*([KMGT])?$").unwrap());
static RAW_COUNT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[\d,.]+\s*[KMGTB]?\s*\((\d[\d,]*)\)$").unwrap());
static PERCENT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\d+(?:\.\d+)?)\s*%$").unwrap());
static RATE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(\d+(?:\.\d+)?)\s*(B|KB|MB|GB|TB)?\s*/sec$").unwrap());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Nanoseconds.
    Duration,
    Bytes,
    Count,
    /// Units per second.
    Rate,
    Percent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stat {
    Sum,
    Avg,
    Max,
    Min,
}

/// A parsed counter. A detail scalar sets all four statistics to the same value except `sum`,
/// which is only meaningful for additive units.
#[derive(Clone, Debug, PartialEq)]
pub struct Metric {
    pub sum: Option<f64>,
    pub avg: Option<f64>,
    pub max: Option<f64>,
    pub min: Option<f64>,
    pub unit: Unit,
}

impl Metric {
    pub fn get(&self, stat: Stat) -> Option<f64> {
        match stat {
            Stat::Sum => self.sum,
            Stat::Avg => self.avg,
            Stat::Max => self.max,
            Stat::Min => self.min,
        }
    }

    /// Parse a raw counter value. `key` only disambiguates unitless zero byte counters
    /// (Doris prints `0.00` for an empty `MemoryUsage`).
    pub fn parse(key: &str, raw: &str) -> Option<Metric> {
        let raw = raw.trim();
        if is_unknown(raw) || raw.starts_with('[') {
            return None;
        }
        let hint = key_hint(key);
        if ["sum ", "avg ", "max ", "min "]
            .iter()
            .any(|prefix| raw.starts_with(prefix))
        {
            let mut metric =
                Metric { sum: None, avg: None, max: None, min: None, unit: Unit::Count };
            let mut unit = None;
            for (stat, slot) in [
                ("sum", &mut metric.sum),
                ("avg", &mut metric.avg),
                ("max", &mut metric.max),
                ("min", &mut metric.min),
            ] {
                if let Some((value, parsed)) = statistic(raw, stat).and_then(|v| quantity(v, hint))
                {
                    *slot = Some(value);
                    // A non-count unit (e.g. "4.13 KB") wins over a bare "0.00".
                    if unit.is_none() || unit == Some(Unit::Count) {
                        unit = Some(parsed);
                    }
                }
            }
            let unit = unit?;
            metric.unit = unit;
            if unit == Unit::Duration && !raw.starts_with("sum ") && !raw.contains(", sum ") {
                metric.sum = None;
            }
            return Some(metric);
        }
        let (value, unit) = quantity(raw, hint)?;
        Some(Metric {
            sum: matches!(unit, Unit::Count | Unit::Bytes).then_some(value),
            avg: Some(value),
            max: Some(value),
            min: Some(value),
            unit,
        })
    }
}

pub fn is_unknown(value: &str) -> bool {
    let value = value.trim();
    value.is_empty() || matches!(value, "N/A" | "NULL" | "-1" | "null")
}

/// Byte counters whose zero is printed without a unit in 4.1.3 (`MemoryUsage: 0.00`,
/// `UncompressedRowBatchSize: 0.00`). `HashTableSize` is a count, so "size" alone is not enough.
fn key_hint(key: &str) -> Option<Unit> {
    let key = key.to_ascii_lowercase();
    (key.contains("bytes") || key.contains("memory") || key.ends_with("batchsize"))
        .then_some(Unit::Bytes)
}

/// Extract one statistic without treating comma separators inside counts as stat delimiters.
pub fn statistic<'a>(value: &'a str, stat: &str) -> Option<&'a str> {
    let value = value.trim();
    let prefix = format!("{} ", stat);
    let start = if value.starts_with(&prefix) {
        0
    } else {
        value.find(&format!(", {} ", stat)).map(|index| index + 2)?
    } + prefix.len();
    let tail = &value[start..];
    let end = [", avg ", ", max ", ", min ", ", sum "]
        .iter()
        .filter_map(|marker| tail.find(marker))
        .min()
        .unwrap_or(tail.len());
    let result = tail[..end].trim();
    (!result.is_empty()).then_some(result)
}

fn scale(suffix: Option<&str>, base: f64) -> f64 {
    match suffix {
        Some("K" | "KB") => base,
        Some("M" | "MB") => base.powi(2),
        Some("G" | "GB") => base.powi(3),
        Some("T" | "TB") => base.powi(4),
        Some("PB") => base.powi(5),
        _ => 1.0,
    }
}

fn finite(value: f64) -> Option<f64> {
    (value.is_finite() && value >= 0.0).then_some(value)
}

/// Parse one quantity. Doris prints negative values (e.g. `-1`) for unset counters.
pub fn quantity(value: &str, hint: Option<Unit>) -> Option<(f64, Unit)> {
    let value = value.trim();
    if is_unknown(value) || value.starts_with('-') {
        return None;
    }
    if let Some(cap) = RATE.captures(value) {
        let number: f64 = cap[1].parse().ok()?;
        return finite(number * scale(cap.get(2).map(|m| m.as_str()), 1024.0))
            .map(|v| (v, Unit::Rate));
    }
    if let Some(cap) = PERCENT.captures(value) {
        return finite(cap[1].parse().ok()?).map(|v| (v, Unit::Percent));
    }
    if DURATION.is_match(value) {
        let mut ns = 0.0;
        for cap in DURATION_PART.captures_iter(value) {
            let number: f64 = cap[1].parse().ok()?;
            ns += number
                * match &cap[2] {
                    "hour" | "h" => 3.6e12,
                    "min" | "m" => 6e10,
                    "sec" | "s" => 1e9,
                    "ms" => 1e6,
                    "us" | "μs" => 1e3,
                    _ => 1.0,
                };
        }
        // Whole nanoseconds: avoid binary float noise such as 32994999.999999996.
        return finite(ns.round()).map(|v| (v, Unit::Duration));
    }
    if let Some(cap) = BYTES.captures(value) {
        let number: f64 = cap[1].parse().ok()?;
        return finite(number * scale(Some(&cap[2]), 1024.0)).map(|v| (v, Unit::Bytes));
    }
    if let Some(cap) = RAW_COUNT.captures(value) {
        return finite(cap[1].replace(',', "").parse().ok()?).map(|v| (v, Unit::Count));
    }
    if let Some(cap) = COUNT.captures(value) {
        let number: f64 = cap[1].replace(',', "").parse().ok()?;
        let unit = if hint == Some(Unit::Bytes) && cap.get(2).is_none() {
            Unit::Bytes
        } else {
            Unit::Count
        };
        return finite(number * scale(cap.get(2).map(|m| m.as_str()), 1000.0)).map(|v| (v, unit));
    }
    None
}

pub fn display_ns(value: f64) -> String {
    if value >= 1e9 {
        format!("{:.3}s", value / 1e9)
    } else if value >= 1e6 {
        format!("{:.3}ms", value / 1e6)
    } else if value >= 1e3 {
        format!("{:.3}us", value / 1e3)
    } else {
        format!("{}ns", value as u64)
    }
}

pub fn display_bytes(value: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = value;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2} {}", UNITS[unit])
}

pub fn display(value: f64, unit: Unit) -> String {
    match unit {
        Unit::Duration => display_ns(value),
        Unit::Bytes => display_bytes(value),
        Unit::Count => format!("{}", value.round() as u64),
        Unit::Rate => format!("{value:.1} /sec"),
        Unit::Percent => format!("{value:.1}%"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(raw: &str) -> (f64, Unit) {
        quantity(raw, None).unwrap()
    }

    #[test]
    fn durations_in_every_observed_and_documented_form() {
        assert_eq!(one("1min2sec3ms"), (62_003_000_000.0, Unit::Duration));
        assert_eq!(one("32.995ms"), (32_995_000.0, Unit::Duration));
        assert_eq!(one("129.342us"), (129_342.0, Unit::Duration));
        assert_eq!(one("0ns"), (0.0, Unit::Duration));
        assert_eq!(one("2s151ms"), (2_151_000_000.0, Unit::Duration));
        assert_eq!(one("1hour3min"), (3_780_000_000_000.0, Unit::Duration));
        assert!(quantity("-1ms", None).is_none());
    }

    #[test]
    fn counts_bytes_rates_and_percentages() {
        assert_eq!(one("150.0M (150000000)"), (150_000_000.0, Unit::Count));
        assert_eq!(one("1,234,567"), (1_234_567.0, Unit::Count));
        assert_eq!(one("1.5K"), (1500.0, Unit::Count));
        assert_eq!(one("4.13 KB"), ((4.13f64 * 1024.0), Unit::Bytes));
        assert_eq!(one("8.00 B"), (8.0, Unit::Bytes));
        assert_eq!(one("2 GB"), (2.0 * 1024f64.powi(3), Unit::Bytes));
        assert_eq!(one("0.0 /sec"), (0.0, Unit::Rate));
        assert_eq!(one("12.5%"), (12.5, Unit::Percent));
        assert_eq!(quantity("0.00", Some(Unit::Bytes)), Some((0.0, Unit::Bytes)));
    }

    #[test]
    fn unknown_values_are_none_not_zero() {
        for raw in ["", "N/A", "NULL", "-1", "sum , avg , max , min", "[1, ]", "True"] {
            assert!(Metric::parse("X", raw).is_none(), "{raw}");
        }
    }

    #[test]
    fn merged_statistics_keep_each_stat_and_unit() {
        let time = Metric::parse("ExecTime", "avg 21.883us, max 39.648us, min 16.641us").unwrap();
        assert_eq!(time.unit, Unit::Duration);
        assert_eq!(time.sum, None);
        assert_eq!(time.avg, Some(21_883.0));
        assert_eq!(time.max, Some(39_648.0));
        let rows = Metric::parse("InputRows", "sum 1, avg 0, max 1, min 0").unwrap();
        assert_eq!((rows.sum, rows.avg, rows.unit), (Some(1.0), Some(0.0), Unit::Count));
        let memory =
            Metric::parse("MemoryUsagePeak", "sum 4.13 KB, avg 528.00 B, max 4.13 KB, min 0.00")
                .unwrap();
        assert_eq!(memory.unit, Unit::Bytes);
        assert_eq!(memory.min, Some(0.0));
        let empty = Metric::parse("MemoryUsage", "sum 0.00 , avg 0.00 , max 0.00 , min 0.00");
        assert_eq!(empty.map(|m| (m.unit, m.max)), Some((Unit::Bytes, Some(0.0))));
        let thousands =
            Metric::parse("RowsProduced", "sum 1,234, avg 617, max 1,000, min 234").unwrap();
        assert_eq!((thousands.sum, thousands.max), (Some(1234.0), Some(1000.0)));
    }

    #[test]
    fn hash_table_size_is_a_count_but_batch_size_is_bytes() {
        assert_eq!(
            Metric::parse("HashTableSize", "sum 20, avg 1, max 1, min 1")
                .unwrap()
                .unit,
            Unit::Count
        );
        assert_eq!(
            Metric::parse("UncompressedRowBatchSize", "0.00")
                .unwrap()
                .unit,
            Unit::Bytes
        );
    }

    #[test]
    fn counter_named_time_that_is_a_count_stays_a_count() {
        // GetBlockFailedTime is a count in Doris 4.1.3.
        let metric = Metric::parse("GetBlockFailedTime", "sum 1, avg 0, max 1, min 0").unwrap();
        assert_eq!(metric.unit, Unit::Count);
    }

    #[test]
    fn scalar_detail_values() {
        let metric = Metric::parse("ExecTime", "34.680ms").unwrap();
        assert_eq!((metric.sum, metric.max), (None, Some(34_680_000.0)));
        let rows = Metric::parse("RowsRead", "1").unwrap();
        assert_eq!(rows.sum, Some(1.0));
    }
}
