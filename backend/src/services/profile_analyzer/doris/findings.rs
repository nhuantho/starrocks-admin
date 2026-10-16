//! A Doris finding before it is attached to a node and converted to the API model.
use crate::services::profile_analyzer::models::{
    DiagnosticResult, ParameterTuningSuggestion, SqlSnippet,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    /// Reserved for OOM risk, failures, very large amplification and near-total skew.
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "Info",
            Severity::Warning => "Warning",
            Severity::Critical => "Critical",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::Low => "low",
            Confidence::Medium => "medium",
            Confidence::High => "high",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub rule_id: &'static str,
    pub rule_name: String,
    pub severity: Severity,
    pub message: String,
    pub reason: String,
    pub suggestions: Vec<String>,
    /// Share (0-1) of measured operator time, query time or rows affected.
    pub impact: Option<f64>,
    pub evidence_counters: Vec<String>,
    pub confidence: Confidence,
    /// Variable whose live value could refine this finding (see `variables.rs`).
    pub related_variable: Option<String>,
    pub parameter_suggestions: Vec<ParameterTuningSuggestion>,
    pub sql_snippets: Vec<SqlSnippet>,
}

impl Finding {
    #[allow(clippy::too_many_arguments)]
    pub fn query(
        rule_id: &'static str,
        rule_name: &str,
        severity: Severity,
        message: String,
        reason: String,
        suggestions: Vec<String>,
        impact: Option<f64>,
        evidence_counters: Vec<String>,
    ) -> Self {
        Self {
            rule_id,
            rule_name: rule_name.to_string(),
            severity,
            message,
            reason,
            suggestions,
            impact: impact.map(|value| value.clamp(0.0, 1.0)),
            evidence_counters,
            confidence: Confidence::High,
            related_variable: None,
            parameter_suggestions: Vec::new(),
            sql_snippets: Vec::new(),
        }
    }

    pub fn into_diagnostic(self, node_path: String, plan_node_id: Option<i32>) -> DiagnosticResult {
        DiagnosticResult {
            rule_id: self.rule_id.into(),
            rule_name: self.rule_name,
            severity: self.severity.as_str().into(),
            node_path,
            plan_node_id,
            message: self.message,
            reason: self.reason,
            suggestions: self.suggestions,
            parameter_suggestions: self.parameter_suggestions,
            impact: self.impact,
            confidence: Some(self.confidence.as_str().into()),
            evidence_counters: self.evidence_counters,
            sql_snippets: self.sql_snippets,
        }
    }
}

pub fn severity_from_str(value: &str) -> Severity {
    match value {
        "Critical" => Severity::Critical,
        "Warning" => Severity::Warning,
        _ => Severity::Info,
    }
}
