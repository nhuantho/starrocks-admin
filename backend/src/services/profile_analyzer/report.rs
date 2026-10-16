//! Markdown / HTML export of an analysis. The raw profile is never included; SQL literals can be
//! redacted.
use super::fingerprint::{fingerprint, redact_literals};
use super::models::ProfileAnalysisResponse;
use crate::utils::i18n::Lang;

fn text<'a>(lang: Lang, en: &'a str, zh: &'a str) -> &'a str {
    if lang == Lang::Zh { zh } else { en }
}

/// Escape characters that would break a Markdown table cell.
fn cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

pub fn markdown(response: &ProfileAnalysisResponse, redact: bool, lang: Lang) -> String {
    let summary = response.summary.clone().unwrap_or_default();
    let mut out = String::new();
    let na = || "N/A".to_string();
    out.push_str(&format!(
        "# {} {}\n\n",
        text(lang, "Query diagnostics", "查询诊断"),
        summary.query_id
    ));
    if let Some(archived) = &response.archived {
        out.push_str(&format!(
            "> {} {} ({})\n\n",
            text(lang, "Archived snapshot captured at", "归档快照，采集于"),
            archived.captured_at,
            archived.reason
        ));
    }
    let rows = [
        (
            text(lang, "Engine", "引擎"),
            format!("{} {}", response.engine, response.engine_version.clone().unwrap_or_default()),
        ),
        (text(lang, "State", "状态"), summary.query_state.clone()),
        (text(lang, "Total", "总耗时"), summary.total_time.clone()),
        (text(lang, "Score", "评分"), format!("{:.0}", response.performance_score)),
        (text(lang, "Confidence", "置信度"), response.confidence.clone().unwrap_or_else(na)),
        (
            text(lang, "Profile level", "Profile 级别"),
            response
                .profile_level
                .map(|l| l.to_string())
                .unwrap_or_else(na),
        ),
        (text(lang, "SQL fingerprint", "SQL 指纹"), fingerprint(&summary.sql_statement)),
    ];
    out.push_str(&format!(
        "| {} | {} |\n| --- | --- |\n",
        text(lang, "Field", "字段"),
        text(lang, "Value", "值")
    ));
    for (key, value) in rows {
        out.push_str(&format!("| {key} | {} |\n", cell(&value)));
    }
    if let Some(phases) = &response.phase_breakdown {
        out.push_str(&format!("\n## {}\n\n", text(lang, "Query phases", "查询阶段")));
        out.push_str(&format!("| {} | ms |\n| --- | ---: |\n", text(lang, "Phase", "阶段")));
        for segment in phases.segments.iter().chain(&phases.details) {
            out.push_str(&format!("| {} | {:.1} |\n", cell(&segment.label), segment.time_ms));
        }
        out.push_str(&format!("\n{}\n", phases.note));
    }
    if let Some(nodes) = summary
        .top_time_consuming_nodes
        .as_ref()
        .filter(|n| !n.is_empty())
    {
        out.push_str(&format!("\n## {}\n\n", text(lang, "Top operators", "主要耗时算子")));
        out.push_str(&format!(
            "| # | {} | {} | % |\n| ---: | --- | ---: | ---: |\n",
            text(lang, "Operator", "算子"),
            text(lang, "Time", "耗时")
        ));
        for node in nodes {
            out.push_str(&format!(
                "| {} | {} (id={}) | {} | {:.1} |\n",
                node.rank,
                cell(&node.operator_name),
                node.plan_node_id,
                cell(&node.total_time),
                node.time_percentage
            ));
        }
        out.push_str(&format!("\n{}\n", response.metric_basis));
    }
    out.push_str(&format!("\n## {}\n\n", text(lang, "Diagnostics", "诊断")));
    if response.diagnostics.is_empty() {
        out.push_str(text(lang, "No diagnostics.\n", "无诊断。\n"));
    }
    for d in &response.diagnostics {
        out.push_str(&format!("### [{}] {} — {}\n\n", d.severity, d.rule_name, d.rule_id));
        out.push_str(&format!("- {}: `{}`\n", text(lang, "Node", "节点"), d.node_path));
        out.push_str(&format!("- {}: {}\n", text(lang, "Finding", "结论"), d.message));
        out.push_str(&format!("- {}: {}\n", text(lang, "Evidence", "证据"), d.reason));
        if let Some(confidence) = &d.confidence {
            out.push_str(&format!("- {}: {confidence}\n", text(lang, "Confidence", "置信度")));
        }
        for suggestion in &d.suggestions {
            out.push_str(&format!("- {}: {suggestion}\n", text(lang, "Suggestion", "建议")));
        }
        for p in &d.parameter_suggestions {
            out.push_str(&format!(
                "- {}: `{}` {} → {} ({}){}\n",
                text(lang, "Parameter", "参数"),
                p.name,
                p.current.as_deref().unwrap_or("?"),
                p.recommended,
                p.scope.as_deref().unwrap_or("session"),
                p.risk
                    .as_deref()
                    .map(|r| format!(" — {r}"))
                    .unwrap_or_default()
            ));
        }
        for snippet in &d.sql_snippets {
            out.push_str(&format!(
                "- {} ({}){}:\n\n```sql\n{}\n```\n",
                snippet.label,
                snippet.scope,
                snippet
                    .warning
                    .as_deref()
                    .map(|w| format!(" ⚠ {w}"))
                    .unwrap_or_default(),
                snippet.sql
            ));
        }
        out.push('\n');
    }
    if !response.topology_warnings.is_empty() {
        out.push_str(&format!("## {}\n\n", text(lang, "Warnings", "警告")));
        for warning in &response.topology_warnings {
            out.push_str(&format!("- {warning}\n"));
        }
        out.push('\n');
    }
    let sql = if redact {
        redact_literals(&summary.sql_statement)
    } else {
        summary.sql_statement.clone()
    };
    out.push_str(&format!(
        "## SQL{}\n\n```sql\n{}\n```\n",
        if redact { text(lang, " (literals redacted)", "（字面量已脱敏）") } else { "" },
        sql.trim()
    ));
    out.push_str(&format!(
        "\n_{}_\n",
        text(
            lang,
            "Recommendations are advice only; nothing was executed on the cluster.",
            "建议仅供参考，未在集群上执行任何操作。"
        )
    ));
    out
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A self-contained HTML page with the Markdown report in a `<pre>` block (no scripts).
pub fn html(response: &ProfileAnalysisResponse, redact: bool, lang: Lang) -> String {
    let title = escape(
        &response
            .summary
            .as_ref()
            .map(|s| s.query_id.clone())
            .unwrap_or_default(),
    );
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>{title}</title><style>body{{font:14px/1.5 system-ui,sans-serif;margin:2rem;max-width:72rem}}pre{{white-space:pre-wrap}}</style></head><body><pre>{}</pre></body></html>\n",
        escape(&markdown(response, redact, lang))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::profile_analyzer::{AnalysisContext, doris};

    fn analysis() -> ProfileAnalysisResponse {
        let text = include_str!("../../../tests/fixtures/profiles/doris-4.1.3-jdbc.txt")
            .replace("LIMIT 5", "WHERE name = 'secret' LIMIT 5");
        doris::analyze_with(&text, &AnalysisContext::default(), &doris::DorisOptions::default())
            .unwrap()
    }

    #[test]
    fn markdown_contains_evidence_and_redacts_on_request() {
        let response = analysis();
        let plain = markdown(&response, false, Lang::En);
        assert!(plain.contains("DORIS_CONNECTOR_INIT") && plain.contains("InitConnectorTime"));
        assert!(plain.contains("'secret'"));
        let redacted = markdown(&response, true, Lang::En);
        assert!(!redacted.contains("secret"));
        assert!(redacted.contains("literals redacted"));
        // The raw profile is never exported.
        assert!(!plain.contains("MergedProfile"));
    }

    #[test]
    fn html_is_escaped_and_scriptless() {
        let mut response = analysis();
        response.summary.as_mut().unwrap().sql_statement =
            "SELECT '<script>alert(1)</script>'".into();
        let page = html(&response, false, Lang::En);
        assert!(!page.contains("<script>"));
        assert!(page.contains("&lt;script&gt;"));
    }
}
