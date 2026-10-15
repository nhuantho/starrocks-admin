//! Doris performance score. Documented in docs/profile/doris-runtime-diagnostics.md.
//!
//! Findings are grouped by rule so one problem repeated on many instances is not counted as
//! many problems. Each rule costs
//! `weight(max severity) * (0.5 + 0.5 * max impact) + weight * 0.1 * (min(count, 5) - 1)`,
//! with unknown impact treated as 0.5. Score = max(0, 100 - sum of costs).
//! Every term is non-negative and non-decreasing in severity, impact and count, so adding a
//! finding or making one worse can never raise the score.
use super::findings::{Severity, severity_from_str};
use crate::services::profile_analyzer::models::DiagnosticResult;
use std::collections::HashMap;

pub fn weight(severity: Severity) -> f64 {
    match severity {
        Severity::Info => 1.0,
        Severity::Warning => 6.0,
        Severity::Critical => 20.0,
    }
}

pub fn score(diagnostics: &[DiagnosticResult]) -> f64 {
    let mut groups: HashMap<&str, (Severity, f64, usize)> = HashMap::new();
    for diagnostic in diagnostics {
        let severity = severity_from_str(&diagnostic.severity);
        let impact = diagnostic.impact.unwrap_or(0.5).clamp(0.0, 1.0);
        let entry = groups
            .entry(diagnostic.rule_id.as_str())
            .or_insert((severity, impact, 0));
        entry.0 = entry.0.max(severity);
        entry.1 = entry.1.max(impact);
        entry.2 += 1;
    }
    let cost: f64 = groups
        .values()
        .map(|(severity, impact, count)| {
            let weight = weight(*severity);
            weight * (0.5 + 0.5 * impact) + weight * 0.1 * (count.min(&5) - 1) as f64
        })
        .sum();
    (100.0 - cost).clamp(0.0, 100.0)
}

/// Sort key: severity first, then impact.
pub fn rank(diagnostic: &DiagnosticResult) -> (Severity, i64) {
    (severity_from_str(&diagnostic.severity), (diagnostic.impact.unwrap_or(0.0) * 1e6) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(rule: &str, severity: &str, impact: Option<f64>) -> DiagnosticResult {
        DiagnosticResult {
            rule_id: rule.into(),
            severity: severity.into(),
            impact,
            ..Default::default()
        }
    }

    #[test]
    fn empty_is_perfect_and_floor_is_zero() {
        assert_eq!(score(&[]), 100.0);
        let many: Vec<_> = (0..20)
            .map(|i| d(&format!("R{i}"), "Critical", Some(1.0)))
            .collect();
        assert_eq!(score(&many), 0.0);
    }

    #[test]
    fn score_is_monotonic() {
        let severities = ["Info", "Warning", "Critical"];
        let impacts = [None, Some(0.0), Some(0.3), Some(1.0)];
        let mut base = Vec::new();
        let mut previous = score(&base);
        // Adding findings never raises the score.
        for (i, severity) in severities.iter().cycle().take(12).enumerate() {
            base.push(d(&format!("R{}", i % 4), severity, impacts[i % 4]));
            let next = score(&base);
            assert!(next <= previous, "{next} > {previous}");
            previous = next;
        }
        // Worse severity or higher impact never raises the score.
        for pair in severities.windows(2) {
            assert!(score(&[d("X", pair[1], Some(0.5))]) <= score(&[d("X", pair[0], Some(0.5))]));
        }
        for pair in [0.0, 0.2, 0.7, 1.0].windows(2) {
            assert!(
                score(&[d("X", "Warning", Some(pair[1]))])
                    <= score(&[d("X", "Warning", Some(pair[0]))])
            );
        }
    }

    #[test]
    fn repeated_rule_costs_less_than_distinct_rules() {
        let repeated = [d("A", "Warning", Some(0.5)), d("A", "Warning", Some(0.5))];
        let distinct = [d("A", "Warning", Some(0.5)), d("B", "Warning", Some(0.5))];
        assert!(score(&repeated) > score(&distinct));
    }
}
