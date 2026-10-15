//! Nereids estimate vs actual rows from the profile's `Appendix: PhysicalPlan`.
//!
//! Observed in doris-4.1.3-jdbc.txt:
//! `+--PhysicalJdbcScan ( qualified=external_catalog.sample_db.items, ..., stats=1 actualRows=1 )`.
//! Runtime operators carry `nereids_id=` but scan plan lines print no bracketed id, so scans are
//! matched to runtime operators by table name, and only when exactly one operator matches.
use super::findings::{Finding, Severity};
use super::metric::is_unknown;
use crate::services::profile_analyzer::models::SqlSnippet;
use crate::utils::i18n::Lang;
use once_cell::sync::Lazy;
use regex::Regex;

static NODE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(Physical[A-Za-z]+)(?:\[(\d+)\])?").unwrap());
static STATS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bstats=([0-9][0-9.,Ee+\-]*)").unwrap());
static ACTUAL: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bactualRows=(\d+)").unwrap());
static QUALIFIED: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bqualified=([^,\s)]+)").unwrap());

#[derive(Clone, Debug, PartialEq)]
pub struct PlanEstimate {
    pub node: String,
    pub nereids_id: Option<i64>,
    pub table: Option<String>,
    pub estimated: f64,
    pub actual: f64,
}

pub fn parse(profile_text: &str) -> Vec<PlanEstimate> {
    let Some((_, appendix)) = profile_text.split_once("PhysicalPlan:") else {
        return Vec::new();
    };
    let mut result: Vec<PlanEstimate> = Vec::new();
    for line in appendix.lines() {
        if line.trim_start().starts_with("DetailProfile") || line.trim() == "MergedProfile:" {
            break;
        }
        let (Some(node), Some(stats), Some(actual)) =
            (NODE.captures(line), STATS.captures(line), ACTUAL.captures(line))
        else {
            continue;
        };
        let raw = stats[1].replace(',', "");
        let (Ok(estimated), Ok(actual)) =
            (raw.trim_end_matches(['.', '+', '-']).parse::<f64>(), actual[1].parse::<f64>())
        else {
            continue;
        };
        if !estimated.is_finite() || estimated < 0.0 {
            continue;
        }
        let estimate = PlanEstimate {
            node: node[1].to_string(),
            nereids_id: node.get(2).and_then(|id| id.as_str().parse().ok()),
            table: QUALIFIED
                .captures(line)
                .map(|cap| cap[1].to_string())
                .filter(|t| !is_unknown(t)),
            estimated,
            actual,
        };
        // The plan prints each scan twice (tree and detail); keep one.
        if !result.contains(&estimate) {
            result.push(estimate);
        }
    }
    result
}

/// Normalize `catalog.db.tbl` / `` `db`.`tbl` `` for suffix comparison.
pub fn table_parts(name: &str) -> Vec<String> {
    name.split('.')
        .map(|part| part.trim().trim_matches('`').to_ascii_lowercase())
        .filter(|part| !part.is_empty())
        .collect()
}

/// True when the shorter qualified name is a suffix of the longer one.
pub fn same_table(a: &str, b: &str) -> bool {
    let (a, b) = (table_parts(a), table_parts(b));
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    !short.is_empty() && long.ends_with(&short)
}

pub fn finding(estimate: &PlanEstimate, lang: Lang) -> Option<Finding> {
    let high = estimate.estimated.max(estimate.actual);
    let low = estimate.estimated.min(estimate.actual).max(1.0);
    let ratio = high / low;
    if high < 1e5 || ratio < 10.0 {
        return None;
    }
    let text = |en: &'static str, zh: &'static str| if lang == Lang::Zh { zh } else { en };
    let direction = if estimate.estimated > estimate.actual {
        text("overestimated", "高估")
    } else {
        text("underestimated", "低估")
    };
    let mut finding = Finding::query(
        "DORIS_ESTIMATE_ERROR",
        text("Row estimate far from actual", "行数估算与实际偏差大"),
        Severity::Warning,
        format!(
            "{} {} {ratio:.0}x",
            estimate.node,
            direction
        ),
        format!(
            "{}{}: stats={} actualRows={} (from Appendix PhysicalPlan)",
            estimate.node,
            estimate.table.as_deref().map(|t| format!(" {t}")).unwrap_or_default(),
            estimate.estimated,
            estimate.actual
        ),
        vec![text(
            "Large estimation errors usually come from missing or stale column statistics and can lead to a poor join order or distribution. Collect statistics and compare the new plan.",
            "估算偏差大通常源于列统计信息缺失或过期，可能导致 Join 顺序或分布方式不佳。收集统计信息后比较新计划。",
        )
        .into()],
        Some((ratio.log10() / 4.0).min(1.0)),
        vec!["stats".into(), "actualRows".into()],
    );
    if let Some(table) = &estimate.table {
        finding.sql_snippets.push(SqlSnippet {
            label: text("Collect statistics (run manually)", "收集统计信息（请手动执行）").into(),
            sql: format!("ANALYZE TABLE {table};"),
            scope: "statement".into(),
            warning: Some(
                text(
                    "Statistics collection reads the table and uses cluster resources; run it off-peak.",
                    "收集统计信息会读取整表并占用集群资源，请在低峰期执行。",
                )
                .into(),
            ),
        });
    }
    Some(finding)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_observed_appendix_and_deduplicates() {
        let text = include_str!("../../../../tests/fixtures/profiles/doris-4.1.3-jdbc.txt");
        let estimates = parse(text);
        assert_eq!(estimates.len(), 1);
        assert_eq!(estimates[0].node, "PhysicalJdbcScan");
        assert_eq!(estimates[0].table.as_deref(), Some("external_catalog.sample_db.items"));
        assert_eq!((estimates[0].estimated, estimates[0].actual), (1.0, 1.0));
        assert!(finding(&estimates[0], Lang::En).is_none());
    }

    #[test]
    fn large_ratio_triggers_and_small_tables_do_not() {
        let mut e = PlanEstimate {
            node: "PhysicalOlapScan".into(),
            nereids_id: None,
            table: Some("db.t".into()),
            estimated: 1e3,
            actual: 5e6,
        };
        let f = finding(&e, Lang::En).unwrap();
        assert_eq!(f.rule_id, "DORIS_ESTIMATE_ERROR");
        assert_eq!(f.sql_snippets[0].sql, "ANALYZE TABLE db.t;");
        e.actual = 5e3;
        assert!(finding(&e, Lang::En).is_none());
        let line = "PhysicalHashJoin[12]@3 ( stats=1.5E+7 actualRows=10 )";
        let parsed = parse(&format!("PhysicalPlan:\n{line}\n"));
        assert_eq!((parsed[0].nereids_id, parsed[0].estimated), (Some(12), 1.5e7));
    }

    #[test]
    fn table_names_match_by_suffix() {
        assert!(same_table("external_catalog.sample_db.items", "`sample_db`.`items`"));
        assert!(!same_table("db.items", "db.items2"));
        assert!(!same_table("", "db.items"));
    }
}
