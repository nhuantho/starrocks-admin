//! Doris pipeline profiles. Prefer MergedProfile; never count its DetailProfile a second time.
//!
//! Submodules: `metric` (typed counter values), `version` (counter aliases per release),
//! `summary` (query phases), `rules` (operator rules), `plan` (estimate vs actual rows),
//! `variables` (context-aware advice) and `scoring`.
pub mod findings;
pub mod metric;
pub mod plan;
pub mod rules;
pub mod scoring;
pub mod summary;
pub mod variables;
pub mod version;

use super::dependency_paths::downstream_path;
use super::{AnalysisContext, ProfileAnalysisResponse, aggregate_diagnostics, models::*};
use crate::utils::i18n::Lang;
use findings::{Confidence, Finding};
use metric::{Metric, Stat, Unit, display, display_ns};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use summary::DorisSummary;

static OPERATOR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*([A-Z][A-Z0-9_]*_OPERATOR)\((.*)\):\s*$").unwrap());
static ID: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:^|\(|[ ,])id=(-?\d+)").unwrap());
static NEREIDS_ID: Lazy<Regex> = Lazy::new(|| Regex::new(r"nereids_id=(\d+)").unwrap());
static DEST: Lazy<Regex> = Lazy::new(|| Regex::new(r"dest_id=(-?\d+)").unwrap());
static FRAGMENT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*Fragment\s+(\d+):").unwrap());
static PIPELINE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*Pipeline\s+(\d+)(?:\(|:)").unwrap());
static INSTANCES: Lazy<Regex> = Lazy::new(|| Regex::new(r"instance_num=(\d+)").unwrap());
static DEPENDENCY_WAIT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^WaitForDependency\[(.+)\]Time$").unwrap());

/// Analysis limits shared with StarRocks.
pub const MAX_PROFILE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_OPERATORS: usize = 4096;

/// Options that are not part of the shared `AnalysisContext`.
#[derive(Clone, Debug, Default)]
pub struct DorisOptions {
    /// Run rules listed in `rules::EXPERIMENTAL` (counters not yet confirmed by a fixture).
    pub experimental_rules: bool,
}

impl DorisOptions {
    /// `STELLAR_DORIS_EXPERIMENTAL_RULES=1` enables experimental rules.
    pub fn from_env() -> Self {
        Self {
            experimental_rules: std::env::var("STELLAR_DORIS_EXPERIMENTAL_RULES")
                .is_ok_and(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes")),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeOperator {
    fragment: String,
    pipeline: String,
    name: String,
    plan_id: Option<i32>,
    nereids_id: Option<i64>,
    table: Option<String>,
    destination: Option<i32>,
    instances: usize,
    metrics: HashMap<String, String>,
    /// Counters copied from DetailProfile into a merged operator (or all, for detail-only input).
    detail_keys: HashSet<String>,
}

impl RuntimeOperator {
    fn key(&self) -> String {
        format!(
            "doris_f{}_p{}_{}_{}_{}",
            self.fragment,
            self.pipeline,
            self.name,
            self.plan_id.map_or("none".into(), |id| id.to_string()),
            self.destination.map_or("none".into(), |id| id.to_string())
        )
    }

    /// Resolve a canonical counter name through the version adapter.
    fn raw(&self, canonical: &str) -> Option<(&str, &str)> {
        let aliases = version::raw_names(canonical);
        let names: &[&str] = if aliases.is_empty() { &[canonical] } else { aliases };
        names.iter().find_map(|name| {
            self.metrics
                .get_key_value(*name)
                .map(|(k, v)| (k.as_str(), v.as_str()))
        })
    }

    fn metric(&self, canonical: &str) -> Option<Metric> {
        let (key, value) = self.raw(canonical)?;
        Metric::parse(key, value)
    }

    fn typed(&self, canonical: &str, stat: Stat, unit: Unit) -> Option<f64> {
        self.metric(canonical)
            .filter(|metric| metric.unit == unit)
            .and_then(|metric| metric.get(stat))
    }

    fn time(&self, canonical: &str, stat: Stat) -> Option<f64> {
        self.typed(canonical, stat, Unit::Duration)
    }

    fn count(&self, canonical: &str, stat: Stat) -> Option<f64> {
        self.typed(canonical, stat, Unit::Count)
    }

    fn bytes(&self, canonical: &str, stat: Stat) -> Option<f64> {
        self.typed(canonical, stat, Unit::Bytes)
    }

    /// Plan info lines such as `join op`, `limit`, `output`.
    fn info(&self, key: &str) -> Option<&str> {
        self.metrics
            .get(key)
            .map(String::as_str)
            .filter(|value| !metric::is_unknown(value))
    }

    /// The operator's `WaitForDependency[<DEP>]Time` counter. `<DEP>` is not always
    /// `<OPERATOR>_DEPENDENCY` (e.g. HASH_JOIN_BUILD_DEPENDENCY), so match by pattern.
    fn dependency_wait(&self) -> Option<(String, f64)> {
        let mut keys: Vec<_> = self
            .metrics
            .keys()
            .filter(|key| DEPENDENCY_WAIT.is_match(key))
            .collect();
        keys.sort();
        keys.into_iter()
            .find_map(|key| self.time(key, Stat::Avg).map(|value| (key.clone(), value)))
    }

    fn confidence(&self, counters: &[&str]) -> Confidence {
        let from_detail = counters.iter().any(|counter| {
            self.raw(counter)
                .is_some_and(|(key, _)| self.detail_keys.contains(key))
        });
        if from_detail { Confidence::Medium } else { Confidence::High }
    }
}

pub fn is_doris_profile(text: &str) -> bool {
    text.contains("Doris Version:")
        || (text.contains("MergedProfile:") && text.contains("_OPERATOR("))
}

fn text<'a>(lang: Lang, en: &'a str, zh: &'a str) -> &'a str {
    if lang == Lang::Zh { zh } else { en }
}

fn parse_operators(section: &str) -> Vec<RuntimeOperator> {
    let mut result = Vec::<RuntimeOperator>::new();
    let mut fragment = String::new();
    let mut pipeline = String::new();
    let mut instances = 1;
    let mut current = None;
    let mut operator_indent = 0;
    for line in section.lines() {
        if let Some(cap) = FRAGMENT.captures(line) {
            fragment = cap[1].to_string();
            current = None;
            continue;
        }
        if let Some(cap) = PIPELINE.captures(line) {
            pipeline = cap[1].to_string();
            instances = INSTANCES
                .captures(line)
                .and_then(|cap| cap[1].parse().ok())
                .unwrap_or(1);
            current = None;
            continue;
        }
        if let Some(cap) = OPERATOR.captures(line) {
            if fragment.is_empty() || pipeline.is_empty() {
                continue;
            }
            operator_indent = line.len() - line.trim_start().len();
            let header = &cap[2];
            let mut metrics = HashMap::new();
            let table = header.split_once("table_name=").map(|(_, table)| {
                table
                    .split(")(id=")
                    .next()
                    .unwrap_or(table)
                    .trim_end_matches(')')
                    .to_string()
            });
            if let Some(table) = &table {
                metrics.insert("Table".to_string(), table.clone());
            }
            result.push(RuntimeOperator {
                fragment: fragment.clone(),
                pipeline: pipeline.clone(),
                name: cap[1].to_string(),
                plan_id: ID.captures(header).and_then(|cap| cap[1].parse().ok()),
                nereids_id: NEREIDS_ID
                    .captures(header)
                    .and_then(|cap| cap[1].parse().ok()),
                table,
                destination: DEST.captures(header).and_then(|cap| cap[1].parse().ok()),
                instances,
                metrics,
                detail_keys: HashSet::new(),
            });
            current = Some(result.len() - 1);
            continue;
        }
        if let Some(index) = current {
            let indent = line.len() - line.trim_start().len();
            if !line.trim().is_empty() && indent <= operator_indent {
                current = None;
                continue;
            }
            let entry = line.trim().strip_prefix("- ").unwrap_or(line.trim());
            if let Some((key, value)) = entry.split_once(": ") {
                // The first counter is the operator's own counter, not an identically named nested phase.
                result[index]
                    .metrics
                    .entry(key.trim().to_string())
                    .or_insert_with(|| value.trim().to_string());
            }
        }
    }
    result
}

/// Detailed profiles repeat each operator per task/backend. Aggregate those tasks exactly once.
/// Units come from the value format, not the counter name (`GetBlockFailedTime` is a count).
fn merge_detail(operators: Vec<RuntimeOperator>) -> Vec<RuntimeOperator> {
    let mut groups: Vec<Vec<RuntimeOperator>> = Vec::new();
    let mut indexes = HashMap::new();
    for op in operators {
        let index = *indexes.entry(op.key()).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[index].push(op);
    }
    groups
        .into_iter()
        .map(|group| {
            let mut merged = group[0].clone();
            merged.instances = group.len();
            let keys: HashSet<_> = group
                .iter()
                .flat_map(|op| op.metrics.keys().cloned())
                .collect();
            for key in keys {
                let samples: Vec<_> = group.iter().filter_map(|op| op.metrics.get(&key)).collect();
                if samples.iter().any(|value| value.trim().starts_with('[')) {
                    // Per-scanner arrays are distributions, not additive duration strings.
                    merged.metrics.insert(
                        key,
                        samples
                            .iter()
                            .map(|value| value.as_str())
                            .collect::<Vec<_>>()
                            .join(" | "),
                    );
                    continue;
                }
                let parsed: Vec<_> = samples
                    .iter()
                    .filter_map(|value| Metric::parse(&key, value))
                    .filter_map(|metric| metric.avg.map(|v| (v, metric.unit)))
                    .collect();
                let Some(unit) = parsed.first().map(|(_, unit)| *unit) else { continue };
                let values: Vec<f64> = parsed
                    .iter()
                    .filter(|(_, u)| *u == unit)
                    .map(|(v, _)| *v)
                    .collect();
                let max = values.iter().copied().fold(f64::MIN, f64::max);
                let min = values.iter().copied().fold(f64::MAX, f64::min);
                let sum: f64 = values.iter().sum();
                let avg = sum / values.len() as f64;
                let value = match unit {
                    Unit::Duration => format!(
                        "avg {}, max {}, min {}",
                        display_ns(avg),
                        display_ns(max),
                        display_ns(min)
                    ),
                    Unit::Count | Unit::Bytes => format!(
                        "sum {}, avg {}, max {}, min {}",
                        display(sum, unit),
                        display(avg, unit),
                        display(max, unit),
                        display(min, unit)
                    ),
                    Unit::Rate | Unit::Percent => continue,
                };
                merged.metrics.insert(key, value);
            }
            merged
        })
        .collect()
}

fn node_type(name: &str) -> NodeType {
    if name.contains("SCAN") {
        if name.contains("OLAP") { NodeType::OlapScan } else { NodeType::ConnectorScan }
    } else if name.contains("JOIN") {
        NodeType::HashJoin
    } else if name.contains("AGGREGATION") || name.contains("AGGREGATE") {
        NodeType::Aggregate
    } else if name.contains("RESULT_SINK") {
        NodeType::ResultSink
    } else if name.contains("DATA_STREAM_SINK") {
        NodeType::ExchangeSink
    } else if name.contains("EXCHANGE") {
        NodeType::ExchangeSource
    } else if name.contains("SORT") || name.contains("TOPN") {
        NodeType::Sort
    } else if name.contains("PROJECT") {
        NodeType::Project
    } else if name.contains("LIMIT") {
        NodeType::Limit
    } else {
        NodeType::Unknown
    }
}
fn add_edge(
    nodes: &mut [ExecutionTreeNode],
    consumer: usize,
    producer: usize,
    warnings: &mut Vec<String>,
) {
    if nodes[consumer].children.contains(&nodes[producer].id) {
        return;
    }
    let index: HashMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let mut stack = vec![producer];
    let mut seen = HashSet::new();
    while let Some(next) = stack.pop() {
        if next == consumer {
            warnings.push(
                "An inconsistent dependency would form a cycle; that edge was omitted.".into(),
            );
            return;
        }
        if seen.insert(next) {
            stack.extend(
                nodes[next]
                    .children
                    .iter()
                    .filter_map(|child| index.get(child.as_str()).copied()),
            );
        }
    }
    nodes[consumer].children.push(nodes[producer].id.clone());
}
fn family(name: &str) -> String {
    // Doris uses a local merge source for a materializing SORT_SINK with the same plan ID.
    if matches!(
        name,
        "LOCAL_MERGE_SORT_SOURCE_OPERATOR"
            | "SORT_SOURCE_OPERATOR"
            | "SORT_SINK_OPERATOR"
            | "SORT_OPERATOR"
    ) {
        return "SORT".to_string();
    }
    name.trim_end_matches("_OPERATOR")
        .trim_end_matches("_SINK")
        .trim_end_matches("_SOURCE")
        .to_string()
}

fn build_tree(operators: &[RuntimeOperator], warnings: &mut Vec<String>) -> ExecutionTree {
    let total: f64 = operators
        .iter()
        .filter_map(|op| op.time("ExecTime", Stat::Avg))
        .sum();
    let mut nodes: Vec<_> = operators
        .iter()
        .map(|op| {
            let exec = op.time("ExecTime", Stat::Avg);
            let percentage = exec
                .filter(|_| total > 0.0)
                .map(|time| time / total * 100.0);
            let mut unique_metrics = op.metrics.clone();
            if let Some(time) = exec {
                unique_metrics.insert("OperatorTotalTime".into(), display_ns(time));
            }
            unique_metrics.insert(
                "TimingBasis".into(),
                "Average ExecTime per task; percentage of sum of operator averages".into(),
            );
            unique_metrics.insert("InstanceCount".into(), op.instances.to_string());
            let rows = op
                .count("RowsProduced", Stat::Sum)
                .or_else(|| op.count("InputRows", Stat::Sum));
            ExecutionTreeNode {
                id: op.key(),
                operator_name: op.name.trim_end_matches("_OPERATOR").to_string(),
                node_type: node_type(&op.name),
                plan_node_id: op.plan_id,
                parent_plan_node_id: None,
                metrics: OperatorMetrics {
                    operator_total_time: exec.map(|v| v as u64),
                    operator_total_time_raw: exec.map(display_ns),
                    operator_total_time_min: op.time("ExecTime", Stat::Min).map(|v| v as u64),
                    operator_total_time_max: op.time("ExecTime", Stat::Max).map(|v| v as u64),
                    push_row_num: op.count("InputRows", Stat::Sum).map(|v| v as u64),
                    pull_row_num: op.count("RowsProduced", Stat::Sum).map(|v| v as u64),
                    memory_usage: op.bytes("MemoryUsagePeak", Stat::Max).map(|v| v as u64),
                    ..Default::default()
                },
                children: Vec::new(),
                depth: 0,
                is_hotspot: percentage.is_some_and(|pct| pct >= 30.0),
                hotspot_severity: if percentage.is_some_and(|pct| pct >= 30.0) {
                    HotSeverity::High
                } else {
                    HotSeverity::Normal
                },
                fragment_id: Some(op.fragment.clone()),
                pipeline_id: Some(op.pipeline.clone()),
                time_percentage: percentage,
                rows: rows.map(|v| v as u64),
                is_most_consuming: percentage.is_some_and(|pct| pct >= 30.0),
                is_second_most_consuming: percentage.is_some_and(|pct| (15.0..30.0).contains(&pct)),
                unique_metrics,
                has_diagnostic: false,
                diagnostic_ids: Vec::new(),
            }
        })
        .collect();
    let mut last_in_pipeline = HashMap::new();
    for (index, op) in operators.iter().enumerate() {
        if let Some(previous) = last_in_pipeline.insert((&op.fragment, &op.pipeline), index) {
            add_edge(&mut nodes, previous, index, warnings);
        }
    }
    for (producer, op) in operators.iter().enumerate() {
        if op.name.contains("DATA_STREAM_SINK") {
            let targets: Vec<_> = operators
                .iter()
                .enumerate()
                .filter(|(_, target)| {
                    target.name == "EXCHANGE_OPERATOR"
                        && target.fragment != op.fragment
                        && op.destination.is_some()
                        && target.plan_id == op.destination
                })
                .map(|(i, _)| i)
                .collect();
            if targets.len() == 1 {
                add_edge(&mut nodes, targets[0], producer, warnings);
            } else {
                warnings.push(format!("Exchange destination {:?} has {} matching receivers; no cross-fragment edge was inferred.", op.destination, targets.len()));
            }
        } else if op.name.ends_with("_SINK_OPERATOR") && op.plan_id.is_some() {
            let targets: Vec<_> = operators
                .iter()
                .enumerate()
                .filter(|(_, target)| {
                    target.fragment == op.fragment
                        && target.pipeline != op.pipeline
                        && target.plan_id == op.plan_id
                        && !target.name.ends_with("_SINK_OPERATOR")
                        && family(&target.name) == family(&op.name)
                })
                .map(|(i, _)| i)
                .collect();
            if targets.len() == 1 {
                add_edge(&mut nodes, targets[0], producer, warnings);
            } else if !op.name.contains("RESULT") && !op.name.contains("TABLE") {
                warnings.push(format!(
                    "{} has no unambiguous paired pipeline source; its branch remains separate.",
                    op.key()
                ));
            }
        }
    }
    let children: HashSet<_> = nodes
        .iter()
        .flat_map(|node| node.children.iter().cloned())
        .collect();
    let roots: Vec<_> = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| !children.contains(&node.id))
        .map(|(i, _)| i)
        .collect();
    // add_edge rejects cycles, so a non-empty DAG always has a root; fall back defensively.
    let root = roots
        .iter()
        .find(|i| nodes[**i].node_type == NodeType::ResultSink)
        .or(roots.first())
        .copied()
        .unwrap_or(0);
    if roots.len() > 1 {
        warnings.push(format!(
            "{} disconnected output branches are shown; missing dependencies were not invented.",
            roots.len()
        ));
    }
    let index: HashMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();
    let mut queue = std::collections::VecDeque::from([(root, 0)]);
    let mut seen = HashSet::new();
    while let Some((i, depth)) = queue.pop_front() {
        if seen.insert(i) {
            nodes[i].depth = depth;
            queue.extend(
                nodes[i]
                    .children
                    .iter()
                    .filter_map(|id| index.get(id).map(|i| (*i, depth + 1))),
            );
        }
    }
    ExecutionTree { root: nodes[root].clone(), nodes }
}

/// Parse operators: MergedProfile first, DetailProfile supplements; detail-only as fallback.
/// Returns operators and whether they were reconstructed from DetailProfile only.
fn collect_operators(
    profile_text: &str,
    warnings: &mut Vec<String>,
) -> Result<(Vec<RuntimeOperator>, bool), String> {
    let merged = profile_text
        .split_once("MergedProfile:")
        .map(|(_, tail)| tail.split("DetailProfile").next().unwrap_or(tail));
    let mut operators = merged.map(parse_operators).unwrap_or_default();
    let mut detail_only = false;
    if operators.is_empty() {
        let detail = profile_text
            .split_once("DetailProfile")
            .map(|(_, tail)| tail.split("Appendix:").next().unwrap_or(tail))
            .unwrap_or(profile_text);
        operators = merge_detail(parse_operators(detail));
        for op in &mut operators {
            op.detail_keys = op.metrics.keys().cloned().collect();
        }
        detail_only = true;
        warnings.push("MergedProfile was unavailable; operator averages were computed from the available detailed tasks. Missing tasks cannot be reconstructed.".into());
    }
    if operators.is_empty() {
        return Err("No supported Doris pipeline runtime operators were found. Enable profiling for the original query and retrieve a complete MergedProfile or DetailProfile.".into());
    }
    if operators.len() > MAX_OPERATORS {
        return Err(format!("Profile exceeds the {MAX_OPERATORS} operator analysis limit"));
    }
    if merged.is_some() && !detail_only {
        // Detailed phase counters supplement merged operators; never overwrite or sum ExecTime.
        if let Some((_, detail)) = profile_text.split_once("DetailProfile") {
            let details: HashMap<_, _> =
                merge_detail(parse_operators(detail.split("Appendix:").next().unwrap_or(detail)))
                    .into_iter()
                    .map(|op| (op.key(), op))
                    .collect();
            for op in &mut operators {
                if let Some(detail) = details.get(&op.key()) {
                    op.metrics
                        .insert("DetailTaskSamples".into(), detail.instances.to_string());
                    for (key, value) in &detail.metrics {
                        if !op.metrics.contains_key(key) {
                            op.metrics.insert(key.clone(), value.clone());
                            op.detail_keys.insert(key.clone());
                        }
                    }
                    if detail.instances < op.instances {
                        warnings.push(format!("{} has {} detailed tasks for {} merged instances; phase breakdown may be partial.",op.key(),detail.instances,op.instances));
                    }
                }
            }
        }
    }
    let mut keys = HashSet::new();
    if operators.iter().any(|op| !keys.insert(op.key())) {
        return Err(
            "MergedProfile contains duplicate operator identities; refusing an ambiguous graph"
                .into(),
        );
    }
    Ok((operators, detail_only))
}

pub fn analyze(
    profile_text: &str,
    context: &AnalysisContext,
) -> Result<ProfileAnalysisResponse, String> {
    analyze_with(profile_text, context, &DorisOptions::from_env())
}

pub fn analyze_with(
    profile_text: &str,
    context: &AnalysisContext,
    options: &DorisOptions,
) -> Result<ProfileAnalysisResponse, String> {
    if profile_text.len() > MAX_PROFILE_BYTES {
        return Err("Profile exceeds the 16 MiB analysis limit".into());
    }
    let lang = context.lang;
    let variables = context.cluster_variables.as_ref();
    let mut warnings = Vec::new();
    let doris_summary = DorisSummary::parse(profile_text);
    // Real 4.1.3 failures can contain a complete Summary/Execution
    // Summary but no runtime operators (doris-4.1.3-failed-summary.txt).
    if doris_summary.version.is_some()
        && doris_summary.task_state().is_some_and(|state| {
            matches!(
                state.to_ascii_uppercase().as_str(),
                "ERR" | "ERROR" | "FAILED" | "CANCELLED" | "CANCELED" | "ABORTED" | "TIMEOUT"
            )
        })
        && !profile_text.lines().any(|line| OPERATOR.is_match(line))
    {
        return Ok(analyze_failed_summary(profile_text, &doris_summary, context));
    }
    let (operators, detail_only) = collect_operators(profile_text, &mut warnings)?;
    let mut tree = build_tree(&operators, &mut warnings);
    let running = doris_summary.is_running();

    // Operator findings.
    let input = rules::RuleInput {
        operators: &operators,
        tree: &tree,
        total_query_ns: doris_summary.total_ns(),
        exec_mem_limit: variables::exec_mem_limit(variables),
        lang,
    };
    let mut node_findings: Vec<Vec<Finding>> = (0..operators.len())
        .map(|index| rules::operator_findings(&input, index))
        .collect();

    // Plan estimates attach to a uniquely matching scan, otherwise to the query.
    let mut query_findings = doris_summary.findings(lang);
    for estimate in plan::parse(profile_text) {
        let Some(finding) = plan::finding(&estimate, lang) else { continue };
        let by_id: Vec<_> = operators
            .iter()
            .enumerate()
            .filter(|(_, op)| estimate.nereids_id.is_some() && op.nereids_id == estimate.nereids_id)
            .map(|(i, _)| i)
            .collect();
        let matches: Vec<_> = if !by_id.is_empty() {
            by_id
        } else {
            estimate
                .table
                .as_deref()
                .map(|table| {
                    operators
                        .iter()
                        .enumerate()
                        .filter(|(_, op)| {
                            op.name.contains("SCAN")
                                && op
                                    .table
                                    .as_deref()
                                    .is_some_and(|t| plan::same_table(t, table))
                        })
                        .map(|(i, _)| i)
                        .collect()
                })
                .unwrap_or_default()
        };
        match matches.as_slice() {
            [index] => node_findings[*index].push(finding),
            _ => query_findings.push(finding),
        }
    }

    // Missing detail counters: say how to get them instead of staying silent.
    let has_detail = operators.iter().any(|op| !op.detail_keys.is_empty());
    if !has_detail && profile_text.contains("MergedProfile:") {
        let level = doris_summary.profile_level();
        if let Some(finding) =
            variables::profile_level_finding(variables, level, doris_summary.version, lang)
        {
            query_findings.push(finding);
        } else if doris_summary
            .version
            .is_some_and(|v| !v.supports_profile_level())
        {
            warnings
                .push("DetailProfile counters are absent; phase-level rules could not run.".into());
        }
    }

    // Filter experimental rules, attach parameter suggestions, cap confidence.
    let keep = |finding: &Finding| {
        options.experimental_rules || !rules::EXPERIMENTAL.contains(&finding.rule_id)
    };
    let confidence_cap = if running {
        Confidence::Low
    } else if detail_only {
        Confidence::Medium
    } else {
        Confidence::High
    };
    let finish = |finding: &mut Finding, peak: Option<f64>| {
        rules::cap_confidence(finding, confidence_cap);
        variables::suggest(finding, variables, doris_summary.version, peak, lang);
    };
    let mut operator_diagnostics: HashMap<String, Vec<DiagnosticResult>> = HashMap::new();
    for (index, findings) in node_findings.into_iter().enumerate() {
        let node = &tree.nodes[index];
        let peak = operators[index].bytes("MemoryUsagePeak", Stat::Max);
        let mut diagnostics: Vec<_> = findings
            .into_iter()
            .filter(keep)
            .map(|mut finding| {
                finish(&mut finding, peak);
                finding.into_diagnostic(node.id.clone(), node.plan_node_id)
            })
            .collect();
        if !diagnostics.is_empty() {
            diagnostics.sort_by_key(|d| std::cmp::Reverse(scoring::rank(d)));
            operator_diagnostics.insert(node.id.clone(), diagnostics);
        }
    }
    let query_diagnostics: Vec<_> = query_findings
        .into_iter()
        .filter(keep)
        .map(|mut finding| {
            finish(&mut finding, None);
            finding.into_diagnostic("query".into(), None)
        })
        .collect();
    for node in &mut tree.nodes {
        if let Some(diagnostics) = operator_diagnostics.get(&node.id) {
            node.has_diagnostic = true;
            node.diagnostic_ids = diagnostics.iter().map(|d| d.rule_id.clone()).collect();
        }
    }
    if let Some(root) = tree.nodes.iter().find(|n| n.id == tree.root.id) {
        tree.root = root.clone();
    }
    let mut diagnostics: Vec<_> = query_diagnostics
        .iter()
        .cloned()
        .chain(tree.nodes.iter().flat_map(|n| {
            operator_diagnostics
                .get(&n.id)
                .into_iter()
                .flatten()
                .cloned()
        }))
        .collect();
    diagnostics.sort_by_key(|d| std::cmp::Reverse(scoring::rank(d)));

    let root_cause_paths = root_cause_paths(&tree, &operator_diagnostics);
    let summary = build_summary(&doris_summary, Some(&tree), running, lang, &mut warnings);
    let timed = tree
        .nodes
        .iter()
        .filter(|n| n.metrics.operator_total_time.is_some())
        .count();
    let conclusion = format!(
        "{} {} {} / {} {}. {}",
        text(lang, "Doris runtime profile:", "Doris 运行时 Profile："),
        tree.nodes.len(),
        text(lang, "operators", "个算子"),
        timed,
        text(lang, "with measured ExecTime", "个具有 ExecTime"),
        text(
            lang,
            "Highlighted paths are measured dependencies and diagnostic candidates, not a proven wall-clock critical path.",
            "高亮路径展示已测量依赖和候选诊断原因，并非已证实的墙钟关键路径。"
        )
    );
    let mut suggestions: Vec<_> = diagnostics
        .iter()
        .flat_map(|d| d.suggestions.iter().cloned())
        .collect();
    suggestions.sort();
    suggestions.dedup();
    let hotspots = diagnostics
        .iter()
        .filter(|d| d.node_path != "query")
        .map(|d| HotSpot {
            node_path: d.node_path.clone(),
            severity: match d.severity.as_str() {
                "Critical" => HotSeverity::Critical,
                "Warning" => HotSeverity::Moderate,
                _ => HotSeverity::Mild,
            },
            issue_type: d.rule_id.clone(),
            description: d.message.clone(),
            suggestions: d.suggestions.clone(),
        })
        .collect();
    let scored: Vec<_> = diagnostics
        .iter()
        .filter(|d| !rules::ADVISORY.contains(&d.rule_id.as_str()))
        .cloned()
        .collect();
    // A console/pool session's current setting cannot establish the level used by
    // a historical query (for example a query submitted by Airflow).
    let profile_level = doris_summary.profile_level();
    Ok(ProfileAnalysisResponse {
        engine: "doris".into(),
        metric_basis: "Operator share = average ExecTime / sum of operator average ExecTime. Parallel execution and dependency waits are not additive wall-clock time; missing counters remain unknown.".into(),
        topology_warnings: warnings,
        root_cause_paths,
        operator_diagnostics,
        hotspots,
        conclusion,
        suggestions,
        performance_score: scoring::score(&scored),
        aggregated_diagnostics: aggregate_diagnostics(&diagnostics, lang),
        diagnostics,
        execution_tree: Some(tree),
        summary: Some(summary),
        node_diagnostics: HashMap::new(),
        profile_content: Some(profile_text.into()),
        fragments: fragments(&operators),
        engine_version: doris_summary.version_raw.clone(),
        profile_level,
        confidence: Some(confidence_cap.as_str().into()),
        phase_breakdown: doris_summary.phase_breakdown(lang),
        archived: None,
    })
}

fn analyze_failed_summary(
    profile_text: &str,
    doris: &DorisSummary,
    context: &AnalysisContext,
) -> ProfileAnalysisResponse {
    let lang = context.lang;
    let warning = text(
        lang,
        "This failed query has no runtime operator counters. Only the recorded summary is available; no execution graph or operator bottleneck is inferred.",
        "此失败查询没有运行时算子计数器。仅提供已记录的摘要，不推断执行图或算子瓶颈。",
    );
    let mut warnings = vec![warning.into()];
    let summary = build_summary(doris, None, false, lang, &mut warnings);
    let mut diagnostics: Vec<_> = doris
        .findings(lang)
        .into_iter()
        .map(|mut finding| {
            finding.confidence = Confidence::Medium;
            finding.into_diagnostic("query".into(), None)
        })
        .collect();
    diagnostics.sort_by_key(|d| std::cmp::Reverse(scoring::rank(d)));
    ProfileAnalysisResponse {
        engine: "doris".into(),
        metric_basis: warning.into(),
        topology_warnings: warnings,
        root_cause_paths: Vec::new(),
        operator_diagnostics: HashMap::new(),
        hotspots: Vec::new(),
        conclusion: warning.into(),
        suggestions: diagnostics
            .iter()
            .flat_map(|d| d.suggestions.iter().cloned())
            .collect(),
        performance_score: scoring::score(&diagnostics),
        execution_tree: None,
        summary: Some(summary),
        aggregated_diagnostics: aggregate_diagnostics(&diagnostics, lang),
        diagnostics,
        node_diagnostics: HashMap::new(),
        profile_content: Some(profile_text.into()),
        fragments: Vec::new(),
        engine_version: doris.version_raw.clone(),
        profile_level: doris.profile_level(),
        confidence: Some(Confidence::Medium.as_str().into()),
        phase_breakdown: doris.phase_breakdown(lang),
        archived: None,
    }
}

/// Rank operators with findings by their largest impact (then execution time), merge paths that
/// cover the same nodes, and keep at most five.
fn root_cause_paths(
    tree: &ExecutionTree,
    operator_diagnostics: &HashMap<String, Vec<DiagnosticResult>>,
) -> Vec<RootCausePath> {
    let mut ranked: Vec<_> = tree
        .nodes
        .iter()
        .filter_map(|node| {
            let all = operator_diagnostics.get(&node.id)?;
            let problems: Vec<_> = all
                .iter()
                .filter(|d| !rules::ADVISORY.contains(&d.rule_id.as_str()))
                .collect();
            let main = problems.first().copied().unwrap_or(&all[0]);
            let key = (
                findings::severity_from_str(&main.severity),
                (all.iter().filter_map(|d| d.impact).fold(0.0, f64::max) * 1e6) as i64,
                node.metrics.operator_total_time.unwrap_or(0),
            );
            Some((key, node, all, main))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0));
    let mut paths: Vec<RootCausePath> = Vec::new();
    for (_, node, all, main) in ranked {
        let node_ids = downstream_path(tree, &node.id);
        let mut suggestions: Vec<_> = all
            .iter()
            .flat_map(|d| d.suggestions.iter().cloned())
            .collect();
        suggestions.sort();
        suggestions.dedup();
        let evidence: Vec<_> = all.iter().map(|d| d.reason.clone()).collect();
        if let Some(existing) = paths.iter_mut().find(|p| p.node_ids == node_ids) {
            existing.evidence.extend(evidence);
            existing.suggestions.extend(suggestions);
            existing.suggestions.sort();
            existing.suggestions.dedup();
            continue;
        }
        if paths.len() == 5 {
            continue;
        }
        paths.push(RootCausePath {
            node_ids,
            message: main.message.clone(),
            evidence,
            suggestions,
        });
    }
    paths
}

fn build_summary(
    doris: &DorisSummary,
    tree: Option<&ExecutionTree>,
    running: bool,
    lang: Lang,
    warnings: &mut Vec<String>,
) -> ProfileSummary {
    let mut summary = ProfileSummary::default();
    let field = |key: &str| doris.fields.get(key).cloned().unwrap_or_default();
    summary.query_id = field("Profile ID");
    summary.start_time = field("Start Time");
    summary.end_time = field("End Time");
    summary.total_time = field("Total");
    summary.total_time_ms = doris.total_ns().map(|n| n / 1e6);
    summary.query_state = field("Task State");
    summary.query_type = doris.field("Task Type").map(Into::into);
    summary.user = doris.field("User").map(Into::into);
    summary.default_db = doris.field("Default Db").map(Into::into);
    summary.starrocks_version = field("Doris Version");
    summary.sql_statement = doris.sql.clone();
    summary.planner_total_time = doris.field("Plan Time").map(Into::into);
    summary.planner_total_time_ms = doris.time_ns("Plan Time").map(|n| n / 1e6);
    summary.query_peak_schedule_time = doris.field("Schedule Time").map(Into::into);
    summary.query_peak_schedule_time_ms = doris.time_ns("Schedule Time").map(|n| n / 1e6);
    summary.result_deliver_time = doris.field("Write Result Time").map(Into::into);
    summary.result_deliver_time_ms = doris.time_ns("Write Result Time").map(|n| n / 1e6);
    summary.variables = doris.changed_variables.clone();
    summary.workload_group = doris.field("Workload Group").map(Into::into);
    if running {
        summary.is_profile_complete = Some(false);
        summary.profile_completeness_warning = Some(
            text(
                lang,
                "Query is still running. Operator counters and diagnostic candidates are provisional.",
                "查询仍在运行。算子计数器和候选诊断为临时数据。",
            )
            .into(),
        );
    } else if doris.task_state().is_some() {
        summary.is_profile_complete = Some(true);
    }
    summary.query_execution_wall_time_ms = summary.total_time_ms;
    summary.query_execution_wall_time = Some(summary.total_time.clone());
    let mut ranked: Vec<_> = tree
        .into_iter()
        .flat_map(|tree| tree.nodes.iter())
        .filter(|n| n.metrics.operator_total_time.is_some())
        .collect();
    ranked.sort_by_key(|node| std::cmp::Reverse(node.metrics.operator_total_time));
    summary.top_time_consuming_nodes = Some(
        ranked
            .iter()
            .take(10)
            .enumerate()
            .map(|(rank, node)| TopNode {
                rank: rank as u32 + 1,
                operator_name: node.operator_name.clone(),
                plan_node_id: node.plan_node_id.unwrap_or(-1),
                total_time: node
                    .metrics
                    .operator_total_time_raw
                    .clone()
                    .unwrap_or_default(),
                time_percentage: node.time_percentage.unwrap_or(0.0),
                is_most_consuming: node.is_most_consuming,
                is_second_most_consuming: node.is_second_most_consuming,
            })
            .collect(),
    );
    let operator_count = tree.map_or(0, |tree| tree.nodes.len());
    if ranked.len() < operator_count {
        warnings.push(format!(
            "ExecTime is missing for {} operators; their time and percentage remain unknown.",
            operator_count - ranked.len()
        ));
    }
    summary
}

fn fragments(operators: &[RuntimeOperator]) -> Vec<Fragment> {
    let mut fragments: Vec<Fragment> = Vec::new();
    for op in operators {
        let fragment_index = fragments
            .iter()
            .position(|fragment| fragment.id == op.fragment)
            .unwrap_or_else(|| {
                fragments.push(Fragment {
                    id: op.fragment.clone(),
                    backend_addresses: Vec::new(),
                    instance_ids: Vec::new(),
                    pipelines: Vec::new(),
                });
                fragments.len() - 1
            });
        let pipelines = &mut fragments[fragment_index].pipelines;
        let pipeline_index = pipelines
            .iter()
            .position(|pipeline| pipeline.id == op.pipeline)
            .unwrap_or_else(|| {
                pipelines.push(Pipeline {
                    id: op.pipeline.clone(),
                    metrics: HashMap::from([("InstanceCount".into(), op.instances.to_string())]),
                    operators: Vec::new(),
                });
                pipelines.len() - 1
            });
        pipelines[pipeline_index].operators.push(Operator {
            name: op.name.clone(),
            plan_node_id: op.plan_id.map(|id| id.to_string()),
            operator_id: Some(op.key()),
            common_metrics: op.metrics.clone(),
            unique_metrics: HashMap::new(),
            children: Vec::new(),
        });
    }
    fragments
}

#[cfg(test)]
mod rule_tests;
#[cfg(test)]
mod snapshot_tests;
#[cfg(test)]
mod tests;
