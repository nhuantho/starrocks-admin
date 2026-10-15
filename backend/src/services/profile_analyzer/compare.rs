//! Compare two analyzed profiles (for example the same SQL before and after a change).
//!
//! Operators are matched by node id, which for Doris encodes fragment, pipeline, operator,
//! plan id and exchange destination. Unmatched operators are listed separately instead of
//! being paired by guess.
use super::fingerprint::fingerprint;
use super::models::{ExecutionTreeNode, ProfileAnalysisResponse};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PhaseDelta {
    pub key: String,
    pub label: String,
    pub base_ms: Option<f64>,
    pub target_ms: Option<f64>,
    pub delta_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperatorDelta {
    pub node_id: String,
    pub operator_name: String,
    pub base_ns: Option<u64>,
    pub target_ns: Option<u64>,
    pub delta_ns: Option<i64>,
    pub base_rows: Option<u64>,
    pub target_rows: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiagnosticKey {
    pub rule_id: String,
    pub node_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileComparison {
    pub base_query_id: String,
    pub target_query_id: String,
    pub same_fingerprint: bool,
    pub base_total_ms: Option<f64>,
    pub target_total_ms: Option<f64>,
    pub total_delta_ms: Option<f64>,
    pub score_delta: f64,
    pub phases: Vec<PhaseDelta>,
    /// Matched operators, largest absolute change first (at most 50)
    pub operators: Vec<OperatorDelta>,
    pub only_in_base: Vec<String>,
    pub only_in_target: Vec<String>,
    pub new_diagnostics: Vec<DiagnosticKey>,
    pub resolved_diagnostics: Vec<DiagnosticKey>,
    pub warnings: Vec<String>,
}

fn diff(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    Some(b? - a?)
}

fn diagnostic_keys(response: &ProfileAnalysisResponse) -> BTreeSet<DiagnosticKey> {
    response
        .diagnostics
        .iter()
        .map(|d| DiagnosticKey { rule_id: d.rule_id.clone(), node_path: d.node_path.clone() })
        .collect()
}

pub fn compare(
    base: &ProfileAnalysisResponse,
    target: &ProfileAnalysisResponse,
) -> ProfileComparison {
    let summary = |r: &ProfileAnalysisResponse| r.summary.clone().unwrap_or_default();
    let (base_summary, target_summary) = (summary(base), summary(target));
    let mut warnings = Vec::new();
    let same_fingerprint = !base_summary.sql_statement.trim().is_empty()
        && fingerprint(&base_summary.sql_statement) == fingerprint(&target_summary.sql_statement);
    if !same_fingerprint {
        warnings.push("The two statements have different SQL fingerprints; operator-level differences may reflect different queries.".into());
    }
    if base.engine != target.engine {
        warnings.push("The profiles come from different engines.".into());
    }

    let phase_map = |r: &ProfileAnalysisResponse| -> Vec<(String, String, f64)> {
        r.phase_breakdown
            .iter()
            .flat_map(|p| p.segments.iter().chain(&p.details))
            .map(|s| (s.key.clone(), s.label.clone(), s.time_ms))
            .collect()
    };
    let (base_phases, target_phases) = (phase_map(base), phase_map(target));
    let mut keys: Vec<(String, String)> = Vec::new();
    for (key, label, _) in base_phases.iter().chain(&target_phases) {
        if !keys.iter().any(|(k, _)| k == key) {
            keys.push((key.clone(), label.clone()));
        }
    }
    let lookup = |phases: &[(String, String, f64)], key: &str| {
        phases.iter().find(|(k, _, _)| k == key).map(|(_, _, v)| *v)
    };
    let phases = keys
        .into_iter()
        .map(|(key, label)| {
            let (b, t) = (lookup(&base_phases, &key), lookup(&target_phases, &key));
            PhaseDelta { key, label, base_ms: b, target_ms: t, delta_ms: diff(b, t) }
        })
        .collect();

    let nodes = |r: &ProfileAnalysisResponse| -> HashMap<String, ExecutionTreeNode> {
        r.execution_tree
            .as_ref()
            .map(|tree| {
                tree.nodes
                    .iter()
                    .map(|n| (n.id.clone(), n.clone()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (base_nodes, target_nodes) = (nodes(base), nodes(target));
    let mut operators: Vec<OperatorDelta> = base_nodes
        .iter()
        .filter_map(|(id, b)| {
            let t = target_nodes.get(id)?;
            let (bt, tt) = (b.metrics.operator_total_time, t.metrics.operator_total_time);
            Some(OperatorDelta {
                node_id: id.clone(),
                operator_name: b.operator_name.clone(),
                base_ns: bt,
                target_ns: tt,
                delta_ns: match (bt, tt) {
                    (Some(b), Some(t)) => Some(t as i64 - b as i64),
                    _ => None,
                },
                base_rows: b.rows,
                target_rows: t.rows,
            })
        })
        .collect();
    operators.sort_by(|a, b| {
        b.delta_ns
            .map(i64::abs)
            .cmp(&a.delta_ns.map(i64::abs))
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
    operators.truncate(50);
    let mut only_in_base: Vec<_> = base_nodes
        .keys()
        .filter(|id| !target_nodes.contains_key(*id))
        .cloned()
        .collect();
    let mut only_in_target: Vec<_> = target_nodes
        .keys()
        .filter(|id| !base_nodes.contains_key(*id))
        .cloned()
        .collect();
    only_in_base.sort();
    only_in_target.sort();
    if !only_in_base.is_empty() || !only_in_target.is_empty() {
        warnings.push("Some operators exist in only one profile (the plan changed); they are listed separately.".into());
    }

    let (base_diag, target_diag) = (diagnostic_keys(base), diagnostic_keys(target));
    ProfileComparison {
        base_query_id: base_summary.query_id.clone(),
        target_query_id: target_summary.query_id.clone(),
        same_fingerprint,
        base_total_ms: base_summary.total_time_ms,
        target_total_ms: target_summary.total_time_ms,
        total_delta_ms: diff(base_summary.total_time_ms, target_summary.total_time_ms),
        score_delta: target.performance_score - base.performance_score,
        phases,
        operators,
        only_in_base,
        only_in_target,
        new_diagnostics: target_diag.difference(&base_diag).cloned().collect(),
        resolved_diagnostics: base_diag.difference(&target_diag).cloned().collect(),
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::profile_analyzer::{AnalysisContext, doris};

    fn analyze(text: &str) -> ProfileAnalysisResponse {
        doris::analyze_with(text, &AnalysisContext::default(), &doris::DorisOptions::default())
            .unwrap()
    }

    #[test]
    fn identical_profiles_have_zero_deltas() {
        let text = include_str!("../../../tests/fixtures/profiles/doris-4.1.3-join-merged.txt");
        let result = compare(&analyze(text), &analyze(text));
        assert!(result.same_fingerprint);
        assert_eq!(result.total_delta_ms, Some(0.0));
        assert!(result.operators.iter().all(|o| o.delta_ns == Some(0)));
        assert!(result.new_diagnostics.is_empty() && result.resolved_diagnostics.is_empty());
        assert!(result.only_in_base.is_empty());
        assert_eq!(
            result
                .phases
                .iter()
                .find(|p| p.key == "plan")
                .unwrap()
                .delta_ms,
            Some(0.0)
        );
    }

    #[test]
    fn changes_are_reported_and_plan_changes_are_not_paired() {
        let base = include_str!("../../../tests/fixtures/profiles/doris-4.1.3-jdbc.txt");
        let slower = base
            .replace(
                "ExecTime: avg 34.680ms, max 34.680ms, min 34.680ms",
                "ExecTime: avg 134.680ms, max 134.680ms, min 134.680ms",
            )
            .replace("   - Total: 41ms", "   - Total: 141ms");
        let result = compare(&analyze(base), &analyze(&slower));
        assert_eq!(result.total_delta_ms, Some(100.0));
        assert_eq!(result.operators[0].operator_name, "JDBC_SCAN");
        assert_eq!(result.operators[0].delta_ns, Some(100_000_000));
        let other = compare(
            &analyze(base),
            &analyze(include_str!("../../../tests/fixtures/profiles/doris-4.1.3-join-merged.txt")),
        );
        assert!(!other.same_fingerprint);
        assert!(!other.only_in_target.is_empty());
        assert!(other.warnings.len() >= 2);
    }
}
