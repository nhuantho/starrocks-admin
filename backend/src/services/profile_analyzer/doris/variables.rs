//! Doris session variables used for context-aware advice.
//!
//! The server only reads these with `SHOW VARIABLES` using this fixed list. Suggestions are text
//! for the user to copy; nothing here is executed. A suggestion is only produced when the current
//! value is known.
use super::findings::Finding;
use super::metric::{Unit, display_bytes, quantity};
use super::version::DorisVersion;
use crate::services::profile_analyzer::ClusterVariables;
use crate::services::profile_analyzer::models::{ParameterTuningSuggestion, SqlSnippet};
use crate::utils::i18n::Lang;

/// Read-only allow-list. Names that do not exist in a release are simply absent from the result.
/// Sources: https://doris.apache.org/docs/4.x/query-acceleration/query-profile/ (enable_profile,
/// profile_level, auto_profile_threshold_ms, parallel_pipeline_task_num, exec_mem_limit),
/// https://doris.apache.org/docs/4.x/key-features/spill-to-disk/ (enable_spill, enable_force_spill),
/// https://doris.apache.org/docs/2.1/query-acceleration/optimization-technology-principle/runtime-filter/
/// (runtime_filter_type, runtime_filter_mode, runtime_filter_wait_time_ms). The remaining names
/// are not on profile pages; they are shown only if `SHOW VARIABLES` returns them.
pub const DORIS_VARIABLE_NAMES: &[&str] = &[
    "enable_profile",
    "profile_level",
    "auto_profile_threshold_ms",
    "parallel_pipeline_task_num",
    "exec_mem_limit",
    "query_timeout",
    "enable_spill",
    "enable_force_spill",
    "runtime_filter_type",
    "runtime_filter_mode",
    "runtime_filter_wait_time_ms",
    "enable_query_cache",
    "enable_nereids_planner",
];

fn text<'a>(lang: Lang, en: &'a str, zh: &'a str) -> &'a str {
    if lang == Lang::Zh { zh } else { en }
}

pub fn exec_mem_limit(variables: Option<&ClusterVariables>) -> Option<f64> {
    let raw = variables?.get("exec_mem_limit")?;
    match quantity(raw, Some(Unit::Bytes))? {
        (value, Unit::Bytes | Unit::Count) if value > 0.0 => Some(value),
        _ => None,
    }
}

fn is_true(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "on" => Some(true),
        "false" | "0" | "off" => Some(false),
        _ => None,
    }
}

fn suggestion(
    name: &str,
    current: &str,
    recommended: String,
    description: &str,
    risk: &str,
    reason: String,
) -> ParameterTuningSuggestion {
    ParameterTuningSuggestion {
        name: name.into(),
        param_type: "Session".into(),
        current: Some(current.into()),
        command: format!("SET {name} = {recommended};"),
        recommended,
        description: description.into(),
        impact: String::new(),
        scope: Some("session".into()),
        risk: Some(risk.into()),
        reason: Some(reason),
    }
}

/// Attach parameter suggestions to findings that have a related variable with a known value.
pub fn suggest(
    finding: &mut Finding,
    variables: Option<&ClusterVariables>,
    version: Option<DorisVersion>,
    peak_bytes: Option<f64>,
    lang: Lang,
) {
    let Some(variables) = variables else { return };
    match (finding.rule_id, finding.related_variable.as_deref()) {
        ("DORIS_OPERATOR_MEMORY" | "DORIS_SPILL", Some("exec_mem_limit")) => {
            let (Some(current), Some(limit), Some(peak)) =
                (variables.get("exec_mem_limit"), exec_mem_limit(Some(variables)), peak_bytes)
            else {
                return;
            };
            if peak >= limit * 0.5 {
                // Round 1.5x the peak up to whole GiB; never suggest lowering the limit.
                let gib = 1024.0 * 1024.0 * 1024.0;
                let target = ((peak * 1.5) / gib).ceil() * gib;
                if target > limit {
                    finding.parameter_suggestions.push(suggestion(
                        "exec_mem_limit",
                        current,
                        format!("{}", target as u64),
                        text(lang, "Per-query memory limit on each backend.", "单个查询在每个 BE 上的内存上限。"),
                        text(lang, "Raises memory available to this session's queries; concurrent queries share BE memory, so prefer fixing the plan first.", "提高本会话查询可用内存；并发查询共享 BE 内存，建议优先优化计划。"),
                        format!("MemoryUsagePeak max={} vs exec_mem_limit={}", display_bytes(peak), display_bytes(limit)),
                    ));
                }
            }
            if version.is_some_and(|v| v.supports_spill_variables())
                && variables.get("enable_spill").and_then(|v| is_true(v)) == Some(false)
            {
                finding.parameter_suggestions.push(suggestion(
                    "enable_spill",
                    &variables["enable_spill"],
                    "true".into(),
                    text(lang, "Allow join, aggregation and sort operators to spill to disk under memory pressure (4.x).", "允许 Join、聚合和排序算子在内存压力下落盘（4.x）。"),
                    text(lang, "Spilling avoids memory-limit failures but makes the query slower and uses BE disk.", "落盘可避免内存超限失败，但会变慢并占用 BE 磁盘。"),
                    format!("MemoryUsagePeak max={}", display_bytes(peak)),
                ));
            }
        },
        ("DORIS_QUERY_FAILED", Some("query_timeout")) => {
            let Some(current) = variables.get("query_timeout") else { return };
            let Ok(seconds) = current.trim().parse::<u64>() else { return };
            finding.parameter_suggestions.push(suggestion(
                "query_timeout",
                current,
                (seconds.saturating_mul(2)).to_string(),
                text(lang, "Query timeout in seconds.", "查询超时时间（秒）。"),
                text(lang, "Only hides the slowness; long queries hold resources longer. Fix the dominant operator first.", "仅掩盖慢的问题，长查询会更久占用资源。应优先优化主要耗时算子。"),
                format!("Task failed with a timeout; query_timeout={current}s"),
            ));
        },
        _ => {},
    }
}

/// Advisory finding: diagnostics are limited because detail counters are missing.
pub fn profile_level_finding(
    variables: Option<&ClusterVariables>,
    profile_level: Option<u8>,
    version: Option<DorisVersion>,
    lang: Lang,
) -> Option<Finding> {
    if !version.is_some_and(|v| v.supports_profile_level()) {
        return None;
    }
    let current = variables
        .and_then(|vars| vars.get("profile_level").cloned())
        .or_else(|| profile_level.map(|level| level.to_string()));
    if current
        .as_deref()
        .and_then(|v| v.trim().parse::<u8>().ok())
        .is_some_and(|level| level >= 2)
    {
        return None;
    }
    let mut finding = Finding::query(
        "DORIS_PROFILE_LEVEL",
        text(lang, "Profile has no per-task detail", "Profile 缺少任务级明细"),
        super::findings::Severity::Info,
        text(lang, "Scanner, connector and exchange phase counters are missing, so some rules could not run.", "缺少 Scanner、连接器和 Exchange 阶段计数器，部分规则无法运行。").into(),
        format!("No DetailProfile operator counters; profile_level={}", current.as_deref().unwrap_or("unknown")),
        vec![text(lang, "Re-run the query with SET profile_level = 2 in the same session for finer diagnosis.", "在同一会话中 SET profile_level = 2 后重新执行查询以获得更细诊断。").into()],
        None,
        Vec::new(),
    );
    finding.sql_snippets.push(SqlSnippet {
        label: text(lang, "Enable detailed profile for this session", "为当前会话开启详细 Profile")
            .into(),
        sql: "SET enable_profile = true;\nSET profile_level = 2;".into(),
        scope: "session".into(),
        warning: None,
    });
    if let Some(current) = current {
        finding.parameter_suggestions.push(suggestion(
            "profile_level",
            &current,
            "2".into(),
            text(lang, "Profile detail level (4.0+).", "Profile 详细级别（4.0+）。"),
            text(
                lang,
                "Collects more counters with small overhead; keep it session-scoped.",
                "收集更多计数器，开销较小；建议仅在会话级开启。",
            ),
            text(
                lang,
                "Detail counters are absent from this profile.",
                "此 Profile 缺少明细计数器。",
            )
            .into(),
        ));
    }
    Some(finding)
}

/// Copyable snippet for enabling profiles cluster-wide, with the scope warning.
pub fn global_profile_snippet(lang: Lang) -> SqlSnippet {
    SqlSnippet {
        label: text(lang, "Enable profiles for all new sessions", "为所有新会话开启 Profile")
            .into(),
        sql: "SET GLOBAL enable_profile = true;".into(),
        scope: "global".into(),
        warning: Some(
            text(
                lang,
                "Affects every new connection on the cluster and adds profile collection overhead.",
                "影响集群上所有新连接，并增加 Profile 收集开销。",
            )
            .into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::profile_analyzer::doris::findings::Severity;

    fn vars(pairs: &[(&str, &str)]) -> ClusterVariables {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn memory_finding() -> Finding {
        let mut f = Finding::query(
            "DORIS_OPERATOR_MEMORY",
            "x",
            Severity::Critical,
            String::new(),
            String::new(),
            vec![],
            None,
            vec![],
        );
        f.related_variable = Some("exec_mem_limit".into());
        f
    }

    #[test]
    fn no_suggestion_without_a_known_current_value() {
        let mut f = memory_finding();
        suggest(&mut f, None, DorisVersion::parse("4.1.3"), Some(3e9), Lang::En);
        suggest(&mut f, Some(&vars(&[])), DorisVersion::parse("4.1.3"), Some(3e9), Lang::En);
        assert!(f.parameter_suggestions.is_empty());
    }

    #[test]
    fn memory_pressure_suggests_limit_and_spill_on_4x_only() {
        let v = vars(&[("exec_mem_limit", "2147483648"), ("enable_spill", "false")]);
        let mut f = memory_finding();
        suggest(&mut f, Some(&v), DorisVersion::parse("4.1.3"), Some(1.9e9), Lang::En);
        let names: Vec<_> = f
            .parameter_suggestions
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, ["exec_mem_limit", "enable_spill"]);
        assert_eq!(f.parameter_suggestions[0].current.as_deref(), Some("2147483648"));
        assert_eq!(f.parameter_suggestions[0].recommended, (3u64 << 30).to_string());
        let mut old = memory_finding();
        suggest(&mut old, Some(&v), DorisVersion::parse("2.1.7"), Some(1.9e9), Lang::En);
        assert_eq!(old.parameter_suggestions.len(), 1);
    }

    #[test]
    fn profile_level_hint_is_version_aware() {
        assert!(
            profile_level_finding(None, None, DorisVersion::parse("2.1.7"), Lang::En).is_none()
        );
        let f = profile_level_finding(
            Some(&vars(&[("profile_level", "1")])),
            None,
            DorisVersion::parse("4.1.3"),
            Lang::En,
        )
        .unwrap();
        assert_eq!(f.parameter_suggestions[0].recommended, "2");
        assert!(
            profile_level_finding(
                Some(&vars(&[("profile_level", "2")])),
                None,
                DorisVersion::parse("4.1.3"),
                Lang::En
            )
            .is_none()
        );
        // Unknown current value: advice text only, no parameter suggestion.
        assert!(
            profile_level_finding(None, None, DorisVersion::parse("4.1.3"), Lang::En)
                .unwrap()
                .parameter_suggestions
                .is_empty()
        );
    }
}
