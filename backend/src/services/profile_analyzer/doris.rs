//! Doris pipeline profiles. Prefer MergedProfile; never count its DetailProfile a second time.
use super::dependency_paths::downstream_path;
use super::parser::core::ValueParser;
use super::{AnalysisContext, ProfileAnalysisResponse, aggregate_diagnostics, models::*};
use crate::utils::i18n::Lang;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};

static OPERATOR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*([A-Z][A-Z0-9_]*_OPERATOR)\((.*)\):\s*$").unwrap());
static ID: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:^|\(|[ ,])id=(-?\d+)").unwrap());
static DEST: Lazy<Regex> = Lazy::new(|| Regex::new(r"dest_id=(-?\d+)").unwrap());
static FRAGMENT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*Fragment\s+(\d+):").unwrap());
static PIPELINE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*Pipeline\s+(\d+)(?:\(|:)").unwrap());
static INSTANCES: Lazy<Regex> = Lazy::new(|| Regex::new(r"instance_num=(\d+)").unwrap());
static RAW_NUMBER: Lazy<Regex> = Lazy::new(|| Regex::new(r"\(([\d,]+)\)").unwrap());
static NUMBER: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([\d,.]+)\s*([KMGT]?)").unwrap());

#[derive(Clone, Debug)]
struct RuntimeOperator {
    fragment: String,
    pipeline: String,
    name: String,
    plan_id: Option<i32>,
    destination: Option<i32>,
    instances: usize,
    metrics: HashMap<String, String>,
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
}

pub fn is_doris_profile(text: &str) -> bool {
    text.contains("Doris Version:")
        || (text.contains("MergedProfile:") && text.contains("_OPERATOR("))
}

/// Extract a statistic without treating comma separators inside counts as stat delimiters.
fn statistic<'a>(value: &'a str, stat: &str) -> Option<&'a str> {
    let value = value.trim();
    if value.is_empty() || matches!(value, "N/A" | "NULL" | "-1") {
        return None;
    }
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
    Some(tail[..end].trim())
}
fn scalar<'a>(value: &'a str, stat: &str) -> Option<&'a str> {
    if ["avg ", "max ", "min ", "sum "]
        .iter()
        .any(|prefix| value.trim().starts_with(prefix))
    {
        statistic(value, stat)
    } else if !value.trim().is_empty() && value.trim() != "N/A" {
        Some(value.trim())
    } else {
        None
    }
}
fn duration(value: &str) -> Option<u64> {
    if value.trim().starts_with('-') {
        return None;
    }
    ValueParser::parse_duration(value)
        .ok()
        .and_then(|value| u64::try_from(value.as_nanos()).ok())
}
fn number(value: &str) -> Option<u64> {
    if let Some(cap) = RAW_NUMBER.captures(value) {
        return cap[1].replace(',', "").parse().ok();
    }
    let cap = NUMBER.captures(value)?;
    let value: f64 = cap[1].replace(',', "").parse().ok()?;
    let scale = match &cap[2] {
        "K" => 1e3,
        "M" => 1e6,
        "G" => 1e9,
        "T" => 1e12,
        _ => 1.0,
    };
    (value.is_finite() && value >= 0.0 && value * scale <= u64::MAX as f64)
        .then_some((value * scale) as u64)
}
fn time_metric(op: &RuntimeOperator, key: &str, stat: &str) -> Option<u64> {
    op.metrics
        .get(key)
        .and_then(|value| scalar(value, stat))
        .and_then(duration)
}
fn rows_metric(op: &RuntimeOperator, key: &str, stat: &str) -> Option<u64> {
    op.metrics
        .get(key)
        .and_then(|value| scalar(value, stat))
        .and_then(number)
}
fn bytes_metric(op: &RuntimeOperator, key: &str, stat: &str) -> Option<u64> {
    op.metrics
        .get(key)
        .and_then(|value| scalar(value, stat))
        .and_then(|value| ValueParser::parse_bytes(value).ok())
}
fn display_ns(value: u64) -> String {
    if value >= 1_000_000_000 {
        format!("{:.3}s", value as f64 / 1e9)
    } else if value >= 1_000_000 {
        format!("{:.3}ms", value as f64 / 1e6)
    } else if value >= 1_000 {
        format!("{:.3}us", value as f64 / 1e3)
    } else {
        format!("{value}ns")
    }
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
            if let Some((_, table)) = header.split_once("table_name=") {
                let table = table
                    .split(")(id=")
                    .next()
                    .unwrap_or(table)
                    .trim_end_matches(')');
                metrics.insert("Table".to_string(), table.to_string());
            }
            result.push(RuntimeOperator {
                fragment: fragment.clone(),
                pipeline: pipeline.clone(),
                name: cap[1].to_string(),
                plan_id: ID.captures(header).and_then(|cap| cap[1].parse().ok()),
                destination: DEST.captures(header).and_then(|cap| cap[1].parse().ok()),
                instances,
                metrics,
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
                if key.contains("Time") {
                    let times: Vec<_> =
                        samples.iter().filter_map(|value| duration(value)).collect();
                    if !times.is_empty() {
                        let avg = (times.iter().map(|n| *n as u128).sum::<u128>()
                            / times.len() as u128) as u64;
                        merged.metrics.insert(
                            key,
                            format!(
                                "avg {}, max {}, min {}",
                                display_ns(avg),
                                display_ns(*times.iter().max().unwrap()),
                                display_ns(*times.iter().min().unwrap())
                            ),
                        );
                    }
                } else if key.contains("Rows") || key.contains("Memory") || key.contains("Bytes") {
                    let is_bytes = key.contains("Memory") || key.contains("Bytes");
                    let values: Vec<_> = samples
                        .iter()
                        .filter_map(|value| {
                            if is_bytes {
                                ValueParser::parse_bytes(value).ok()
                            } else {
                                number(value)
                            }
                        })
                        .collect();
                    if !values.is_empty() {
                        let sum = values
                            .iter()
                            .fold(0u64, |sum, value| sum.saturating_add(*value));
                        let unit = if is_bytes { " B" } else { "" };
                        merged.metrics.insert(
                            key,
                            format!(
                                "sum {sum}{unit}, avg {}{unit}, max {}{unit}, min {}{unit}",
                                sum / values.len() as u64,
                                values.iter().max().unwrap(),
                                values.iter().min().unwrap()
                            ),
                        );
                    }
                }
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
    let total: u128 = operators
        .iter()
        .filter_map(|op| time_metric(op, "ExecTime", "avg"))
        .map(u128::from)
        .sum();
    let mut nodes: Vec<_> = operators
        .iter()
        .map(|op| {
            let exec = time_metric(op, "ExecTime", "avg");
            let percentage = exec
                .filter(|_| total > 0)
                .map(|time| time as f64 / total as f64 * 100.0);
            let mut unique_metrics = op.metrics.clone();
            if let Some(time) = exec {
                unique_metrics.insert("OperatorTotalTime".into(), display_ns(time));
            }
            unique_metrics.insert(
                "TimingBasis".into(),
                "Average ExecTime per task; percentage of sum of operator averages".into(),
            );
            unique_metrics.insert("InstanceCount".into(), op.instances.to_string());
            ExecutionTreeNode {
                id: op.key(),
                operator_name: op.name.trim_end_matches("_OPERATOR").to_string(),
                node_type: node_type(&op.name),
                plan_node_id: op.plan_id,
                parent_plan_node_id: None,
                metrics: OperatorMetrics {
                    operator_total_time: exec,
                    operator_total_time_raw: exec.map(display_ns),
                    operator_total_time_min: time_metric(op, "ExecTime", "min"),
                    operator_total_time_max: time_metric(op, "ExecTime", "max"),
                    push_row_num: rows_metric(op, "InputRows", "sum"),
                    pull_row_num: rows_metric(op, "RowsProduced", "sum"),
                    memory_usage: bytes_metric(op, "MemoryUsagePeak", "max"),
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
                rows: rows_metric(op, "RowsProduced", "sum")
                    .or_else(|| rows_metric(op, "InputRows", "sum")),
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
    let root = roots
        .iter()
        .find(|i| nodes[**i].node_type == NodeType::ResultSink)
        .copied()
        .unwrap_or(roots[0]);
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

fn diagnose(
    operators: &[RuntimeOperator],
    tree: &ExecutionTree,
    lang: Lang,
) -> HashMap<String, Vec<DiagnosticResult>> {
    let mut result = HashMap::new();
    for (op, node) in operators.iter().zip(&tree.nodes) {
        let mut diagnostics = Vec::new();
        let mut add = |id: &str,
                       name: &str,
                       severity: &str,
                       message: String,
                       reason: String,
                       suggestions: Vec<String>| {
            diagnostics.push(DiagnosticResult {
                rule_id: id.into(),
                rule_name: name.into(),
                severity: severity.into(),
                node_path: node.id.clone(),
                plan_node_id: node.plan_node_id,
                message,
                reason,
                suggestions,
                parameter_suggestions: Vec::new(),
            });
        };
        let exec = time_metric(op, "ExecTime", "avg");
        if let (Some(exec), Some(pct)) = (exec, node.time_percentage) {
            if pct >= 30.0 {
                add("DORIS_TIME_SHARE", text(lang,"Dominant operator","主要耗时算子"), "Info",
                    format!("{}: {:.1}% {}",node.operator_name,pct,text(lang,"of measured operator execution time","的已测量算子执行时间")),
                    format!("ExecTime avg = {}; share = avg ExecTime / sum of operator averages. This identifies concentration of work, not proof that this query is slow.",display_ns(exec)),
                    vec![text(lang,"Inspect this operator's phase counters and input volume before changing query or cluster settings.","先检查此算子的阶段计数器和输入量，再调整查询或集群设置。").into()]);
            }
            if exec >= 1_000_000 && op.instances > 1 {
                if let Some(max) = time_metric(op, "ExecTime", "max")
                    .filter(|max| *max as f64 / exec as f64 >= 2.0)
                {
                    add("DORIS_EXEC_SKEW",text(lang,"Uneven execution across tasks","任务执行不均衡"),"Warning",
                        text(lang,"Some tasks take at least twice the average execution time.","部分任务执行时间至少为平均值的两倍。").into(),
                        format!("ExecTime max={}, avg={}, max/avg={:.2}, instances={}",display_ns(max),display_ns(exec),max as f64/exec as f64,op.instances),
                        vec![text(lang,"Compare per-task rows and execution time in DetailProfile; check distribution keys, skewed join keys and backend load.","在 DetailProfile 中比较各任务行数和执行时间；检查分布键、Join 热点键及 BE 负载。").into()]);
                }
            }
        }
        if op.instances > 1 {
            if let (Some(avg), Some(max), Some(sum)) = (
                rows_metric(op, "RowsProduced", "avg"),
                rows_metric(op, "RowsProduced", "max"),
                rows_metric(op, "RowsProduced", "sum"),
            ) {
                if avg > 0 && max as f64 / avg as f64 >= 2.0 && sum >= 1000 {
                    add("DORIS_ROW_SKEW",text(lang,"Uneven output distribution","输出数据倾斜"),"Warning",text(lang,"Output rows are concentrated in a subset of tasks.","输出行集中在少数任务中。").into(),format!("RowsProduced sum={sum}, avg={avg}, max={max}; max/avg={:.2}",max as f64/avg as f64),vec![text(lang,"Inspect the per-backend row distribution and join/grouping keys; verify statistics before changing distribution strategy.","检查各 BE 行分布及 Join/分组键；调整分布策略前确认统计信息。").into()]);
                }
            }
        }
        if op.name.contains("SCAN") {
            if let (Some(init), Some(exec)) = (time_metric(op, "InitConnectorTime", "avg"), exec) {
                if init >= 5_000_000 && init as f64 >= exec as f64 * 0.5 {
                    add("DORIS_CONNECTOR_INIT",text(lang,"Connector initialization dominates the scan","连接器初始化占据扫描时间"),"Info",text(lang,"Connector setup is the largest measured scan phase.","连接器初始化是扫描中耗时最大的已测量阶段。").into(),format!("InitConnectorTime={}, ExecTime avg={}; setup/ExecTime={:.1}%",display_ns(init),display_ns(exec),init as f64/exec as f64*100.0),vec![text(lang,"Check external connector connection reuse, driver loading and connection latency. Compare warm and cold runs before changing the SQL.","检查外部连接器连接复用、驱动加载及连接延迟；修改 SQL 前比较冷、热执行。").into()]);
                }
            }
            let wait_key = format!("WaitForDependency[{}_DEPENDENCY]Time", op.name);
            if let (Some(wait), Some(exec)) = (time_metric(op, &wait_key, "avg"), exec) {
                if wait >= 5_000_000 && wait as f64 >= exec as f64 * 0.5 {
                    add("DORIS_SCAN_WAIT",text(lang,"Scan dependency wait","扫描依赖等待"),"Info",text(lang,"The scan spends substantial time waiting for its scanner dependency.","扫描算子有较多时间在等待 Scanner 依赖。").into(),format!("{wait_key} avg={}, ExecTime avg={}. Wait time is separate and must not be added to operator execution time.",display_ns(wait),display_ns(exec)),vec![text(lang,"Inspect scanner connector/IO counters and worker scheduling in DetailProfile; this wait alone does not prove a storage or network fault.","检查 DetailProfile 中连接器、IO 及调度计数器；仅凭等待时间不能确认存储或网络故障。").into()]);
                }
            }
            if let (Some(read), Some(output)) = (
                rows_metric(op, "RowsRead", "sum")
                    .or_else(|| rows_metric(op, "RawRowsRead", "sum")),
                node.rows,
            ) {
                if read >= 100_000 && output as f64 / (read as f64) < 0.01 {
                    add("DORIS_SCAN_VOLUME",text(lang,"Large scan with low output","扫描量大而输出少"),"Warning",text(lang,"The scan reads far more rows than it produces.","扫描读取的行数远多于输出行数。").into(),format!("RowsRead={read}, RowsProduced={output}; output/read={:.3}%",output as f64/read as f64*100.0),vec![text(lang,"Check predicate pushdown, partition pruning and selected columns. Confirm whether this reduction is expected for the query.","检查谓词下推、分区裁剪和所选列；确认该查询是否预期需要这种过滤比例。").into()]);
                }
            }
        }
        if op.name.contains("JOIN") && !op.name.contains("SINK") {
            if let (Some(probe), Some(output)) = (rows_metric(op, "ProbeRows", "sum"), node.rows) {
                if probe > 0 && output >= 10_000 && output as f64 / probe as f64 >= 10.0 {
                    add("DORIS_JOIN_EXPANSION",text(lang,"Join row amplification","Join 行数放大"),"Warning",text(lang,"The join produces at least ten rows per probe row.","Join 每个 Probe 行至少产生十个输出行。").into(),format!("ProbeRows={probe}, RowsProduced={output}, output/probe={:.2}",output as f64/probe as f64),vec![text(lang,"Verify join predicates and key uniqueness; check whether a many-to-many join is intended, and consider pre-aggregation or earlier filters.","检查 Join 条件和键唯一性；确认多对多 Join 是否符合预期，并考虑预聚合或提前过滤。").into()]);
                }
            }
        }
        for (key, value) in &op.metrics {
            if key.to_ascii_lowercase().contains("spill")
                && key.to_ascii_lowercase().contains("bytes")
            {
                if let Some(bytes) = scalar(value, "sum")
                    .or_else(|| scalar(value, "max"))
                    .and_then(|v| ValueParser::parse_bytes(v).ok())
                    .filter(|n| *n > 0)
                {
                    add("DORIS_SPILL",text(lang,"Operator spilled to disk","算子发生落盘"),"Warning",text(lang,"This operator reports nonzero spill bytes.","此算子报告了非零落盘字节数。").into(),format!("{key}={bytes} bytes"),vec![text(lang,"Reduce rows or columns before this operator; inspect sort/join cardinality and available memory before increasing query memory limits.","减少进入算子的行数或列数；提高查询内存限制前检查排序/Join 基数和可用内存。").into()]);
                    break;
                }
            }
        }
        if !diagnostics.is_empty() {
            result.insert(node.id.clone(), diagnostics);
        }
    }
    result
}

pub fn analyze(
    profile_text: &str,
    context: &AnalysisContext,
) -> Result<ProfileAnalysisResponse, String> {
    if profile_text.len() > 16 * 1024 * 1024 {
        return Err("Profile exceeds the 16 MiB analysis limit".into());
    }
    let mut warnings = Vec::new();
    let merged = profile_text
        .split_once("MergedProfile:")
        .map(|(_, tail)| tail.split("DetailProfile").next().unwrap_or(tail));
    let mut operators = merged.map(parse_operators).unwrap_or_default();
    if operators.is_empty() {
        let detail = profile_text
            .split_once("DetailProfile")
            .map(|(_, tail)| tail.split("Appendix:").next().unwrap_or(tail))
            .unwrap_or(profile_text);
        operators = merge_detail(parse_operators(detail));
        warnings.push("MergedProfile was unavailable; operator averages were computed from the available detailed tasks. Missing tasks cannot be reconstructed.".into());
    }
    if operators.is_empty() {
        return Err("No supported Doris pipeline runtime operators were found. Enable profiling for the original query and retrieve a complete MergedProfile or DetailProfile.".into());
    }
    if operators.len() > 4096 {
        return Err("Profile exceeds the 4096 operator analysis limit".into());
    }
    if merged.is_some() {
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
                        op.metrics
                            .entry(key.clone())
                            .or_insert_with(|| value.clone());
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
    let mut tree = build_tree(&operators, &mut warnings);
    let operator_diagnostics = diagnose(&operators, &tree, context.lang);
    for node in &mut tree.nodes {
        if let Some(diagnostics) = operator_diagnostics.get(&node.id) {
            node.has_diagnostic = true;
            node.diagnostic_ids = diagnostics.iter().map(|d| d.rule_id.clone()).collect();
        }
    }
    tree.root = tree
        .nodes
        .iter()
        .find(|n| n.id == tree.root.id)
        .unwrap()
        .clone();
    let diagnostics: Vec<_> = tree
        .nodes
        .iter()
        .flat_map(|n| {
            operator_diagnostics
                .get(&n.id)
                .into_iter()
                .flatten()
                .cloned()
        })
        .collect();
    let mut ranked: Vec<_> = tree
        .nodes
        .iter()
        .filter(|n| n.metrics.operator_total_time.is_some())
        .collect();
    ranked.sort_by_key(|node| std::cmp::Reverse(node.metrics.operator_total_time));
    let root_cause_paths: Vec<_> = ranked
        .iter()
        .filter_map(|node| {
            let all = operator_diagnostics.get(&node.id)?;
            let main = all
                .iter()
                .find(|d| d.rule_id != "DORIS_TIME_SHARE")
                .unwrap_or(&all[0]);
            let mut suggestions: Vec<_> = all
                .iter()
                .flat_map(|d| d.suggestions.iter().cloned())
                .collect();
            suggestions.sort();
            suggestions.dedup();
            Some(RootCausePath {
                node_ids: downstream_path(&tree, &node.id),
                message: main.message.clone(),
                evidence: all.iter().map(|d| d.reason.clone()).collect(),
                suggestions,
            })
        })
        .take(5)
        .collect();
    let mut summary = ProfileSummary::default();
    let header = profile_text
        .split("MergedProfile:")
        .next()
        .unwrap_or(profile_text);
    for line in header.lines() {
        let entry = line.trim().trim_start_matches("- ");
        let Some((key, value)) = entry.split_once(": ") else { continue };
        match key {
            "Profile ID" => summary.query_id = value.into(),
            "Start Time" => summary.start_time = value.into(),
            "End Time" => summary.end_time = value.into(),
            "Total" => {
                summary.total_time = value.into();
                summary.total_time_ms = duration(value).map(|n| n as f64 / 1e6);
            },
            "Task State" => summary.query_state = value.into(),
            "User" => summary.user = Some(value.into()),
            "Default Db" => summary.default_db = Some(value.into()),
            "Doris Version" => summary.starrocks_version = value.into(),
            "Sql Statement" => summary.sql_statement = value.into(),
            "Plan Time" => {
                summary.planner_total_time = Some(value.into());
                summary.planner_total_time_ms = duration(value).map(|n| n as f64 / 1e6);
            },
            _ => {},
        }
    }
    let in_progress = matches!(
        summary.query_state.to_ascii_uppercase().as_str(),
        "RUNNING" | "PENDING" | "QUEUED"
    );
    if in_progress {
        summary.is_profile_complete = Some(false);
        summary.profile_completeness_warning = Some(text(context.lang,
            "Query is still running. Operator counters and diagnostic candidates are provisional.",
            "查询仍在运行。算子计数器和候选诊断为临时数据。"
        ).into());
    }
    summary.query_execution_wall_time_ms = summary.total_time_ms;
    summary.query_execution_wall_time = Some(summary.total_time.clone());
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
    let timed = tree
        .nodes
        .iter()
        .filter(|n| n.metrics.operator_total_time.is_some())
        .count();
    if timed < tree.nodes.len() {
        warnings.push(format!(
            "ExecTime is missing for {} operators; their time and percentage remain unknown.",
            tree.nodes.len() - timed
        ));
    }
    let conclusion = format!(
        "{} {} {} / {} {}. {}",
        text(context.lang, "Doris runtime profile:", "Doris 运行时 Profile："),
        tree.nodes.len(),
        text(context.lang, "operators", "个算子"),
        timed,
        text(context.lang, "with measured ExecTime", "个具有 ExecTime"),
        text(
            context.lang,
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
        .map(|d| HotSpot {
            node_path: d.node_path.clone(),
            severity: if d.severity == "Warning" {
                HotSeverity::Moderate
            } else {
                HotSeverity::Mild
            },
            issue_type: d.rule_id.clone(),
            description: d.message.clone(),
            suggestions: d.suggestions.clone(),
        })
        .collect();
    let mut fragments: Vec<Fragment> = Vec::new();
    for op in &operators {
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
    Ok(ProfileAnalysisResponse {
        engine:"doris".into(),metric_basis:"Operator share = average ExecTime / sum of operator average ExecTime. Parallel execution and dependency waits are not additive wall-clock time; missing counters remain unknown.".into(),
        topology_warnings:warnings,root_cause_paths,operator_diagnostics,
        hotspots,conclusion,suggestions,performance_score:100.0-(diagnostics.iter().filter(|d|d.severity=="Warning").count() as f64*5.0).min(100.0),
        execution_tree:Some(tree),summary:Some(summary),aggregated_diagnostics:aggregate_diagnostics(&diagnostics,context.lang),diagnostics,node_diagnostics:HashMap::new(),profile_content:Some(profile_text.into()),fragments,
    })
}

#[cfg(test)]
mod tests;
