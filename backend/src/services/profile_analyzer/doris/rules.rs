//! Doris operator rules.
//!
//! Every counter read here is either observed in a committed fixture
//! (docs/profile/doris-counter-catalog.md) or documented by Doris and cited inline. Rules that
//! depend on counters or header formats not yet seen in a real profile are listed in
//! `EXPERIMENTAL` and only run when `DorisOptions::experimental_rules` is enabled.
use super::RuntimeOperator;
use super::findings::{Confidence, Finding, Severity};
use super::metric::{Stat, display_bytes, display_ns};
use crate::services::profile_analyzer::models::{ExecutionTree, SqlSnippet};
use crate::utils::i18n::Lang;
use std::collections::HashMap;

/// Rules whose counters, operator names or header formats are not yet confirmed by a fixture.
pub const EXPERIMENTAL: &[&str] = &[
    // No Doris doc lists a spill profile counter; matches any "*spill*bytes*" counter.
    "DORIS_SPILL",
    // "(PARTITIONED)" join op text has not been captured yet (only BROADCAST is observed).
    "DORIS_SHUFFLE_SMALL_BUILD",
    // NESTED_LOOP_JOIN_* operator names have not been captured yet.
    "DORIS_NESTED_LOOP_JOIN",
    // Aggregation output text for DISTINCT has not been captured yet.
    "DORIS_COUNT_DISTINCT",
    // OLAP scan counters are documented for 2.1/3.x only; semantics unverified on 4.x.
    "DORIS_OLAP_INDEX_FILTER",
    "DORIS_OLAP_IO",
    "DORIS_OLAP_SEGMENTS",
    "DORIS_OLAP_PAGE_CACHE",
];

/// Findings that inform but do not indicate a problem; excluded from the score.
pub const ADVISORY: &[&str] = &["DORIS_TIME_SHARE", "DORIS_PROFILE_LEVEL"];

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

pub struct RuleInput<'a> {
    pub operators: &'a [RuntimeOperator],
    pub tree: &'a ExecutionTree,
    pub total_query_ns: Option<f64>,
    pub exec_mem_limit: Option<f64>,
    pub lang: Lang,
}

fn text<'a>(lang: Lang, en: &'a str, zh: &'a str) -> &'a str {
    if lang == Lang::Zh { zh } else { en }
}

impl RuleInput<'_> {
    fn query_share(&self, ns: f64) -> Option<f64> {
        self.total_query_ns.filter(|t| *t > 0.0).map(|t| ns / t)
    }

    /// The paired operator with the same fragment and plan id (e.g. join probe for a build sink).
    fn paired(&self, op: &RuntimeOperator, name: &str) -> Option<&RuntimeOperator> {
        let mut matches = self.operators.iter().filter(|other| {
            other.name == name && other.fragment == op.fragment && other.plan_id == op.plan_id
        });
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    /// Rows produced by the operators feeding `index` in the execution tree.
    fn input_rows(&self, index: usize) -> Option<f64> {
        let ids: HashMap<_, _> = self
            .tree
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (node.id.as_str(), i))
            .collect();
        let children = &self.tree.nodes[index].children;
        if children.is_empty() {
            return None;
        }
        children
            .iter()
            .map(|id| {
                ids.get(id.as_str())
                    .and_then(|i| self.operators[*i].count("RowsProduced", Stat::Sum))
            })
            .sum()
    }

    /// Any operator with the same fragment and plan id that has a `limit` plan info line.
    fn has_limit(&self, op: &RuntimeOperator) -> bool {
        self.operators.iter().any(|other| {
            other.fragment == op.fragment
                && other.plan_id == op.plan_id
                && other
                    .info("limit")
                    .is_some_and(|limit| limit.trim() != "-1")
        })
    }
}

pub fn operator_findings(input: &RuleInput, index: usize) -> Vec<Finding> {
    let op = &input.operators[index];
    let node = &input.tree.nodes[index];
    let lang = input.lang;
    let mut out = Vec::new();
    let exec = op.time("ExecTime", Stat::Avg);
    let share = node.time_percentage.map(|pct| pct / 100.0);
    let mut push = |mut finding: Finding, counters: &[&str]| {
        finding.confidence = op.confidence(counters);
        out.push(finding);
    };

    if let (Some(exec), Some(share)) = (exec, share) {
        if share >= 0.3 {
            push(Finding::query("DORIS_TIME_SHARE", text(lang, "Dominant operator", "主要耗时算子"), Severity::Info,
                format!("{}: {:.1}% {}", node.operator_name, share * 100.0, text(lang, "of measured operator execution time", "的已测量算子执行时间")),
                format!("ExecTime avg = {}; share = avg ExecTime / sum of operator averages. This identifies concentration of work, not proof that this query is slow.", display_ns(exec)),
                vec![text(lang, "Inspect this operator's phase counters and input volume before changing query or cluster settings.", "先检查此算子的阶段计数器和输入量，再调整查询或集群设置。").into()],
                Some(share), vec!["ExecTime".into()]), &["ExecTime"]);
        }
        if exec >= 1e6
            && op.instances > 1
            && let Some(max) = op
                .time("ExecTime", Stat::Max)
                .filter(|max| max / exec >= 2.0)
        {
            let ratio = max / exec;
            let severity =
                if ratio >= 10.0 && share >= 0.5 { Severity::Critical } else { Severity::Warning };
            push(Finding::query("DORIS_EXEC_SKEW", text(lang, "Uneven execution across tasks", "任务执行不均衡"), severity,
                    text(lang, "Some tasks take at least twice the average execution time.", "部分任务执行时间至少为平均值的两倍。").into(),
                    format!("ExecTime max={}, avg={}, max/avg={ratio:.2}, instances={}", display_ns(max), display_ns(exec), op.instances),
                    vec![text(lang, "Compare per-task rows and execution time in DetailProfile; check distribution keys, skewed join keys and backend load.", "在 DetailProfile 中比较各任务行数和执行时间；检查分布键、Join 热点键及 BE 负载。").into()],
                    Some(share), vec!["ExecTime".into()]), &["ExecTime"]);
        }
    }

    if op.instances > 1
        && let (Some(avg), Some(max), Some(sum)) = (
            op.count("RowsProduced", Stat::Avg),
            op.count("RowsProduced", Stat::Max),
            op.count("RowsProduced", Stat::Sum),
        )
        && avg > 0.0
        && max / avg >= 2.0
        && sum >= 1000.0
    {
        push(Finding::query("DORIS_ROW_SKEW", text(lang, "Uneven output distribution", "输出数据倾斜"), Severity::Warning,
                    text(lang, "Output rows are concentrated in a subset of tasks.", "输出行集中在少数任务中。").into(),
                    format!("RowsProduced sum={sum}, avg={avg}, max={max}; max/avg={:.2}", max / avg),
                    vec![text(lang, "Inspect the per-backend row distribution and join/grouping keys; verify statistics before changing distribution strategy.", "检查各 BE 行分布及 Join/分组键；调整分布策略前确认统计信息。").into()],
                    share, vec!["RowsProduced".into()]), &["RowsProduced"]);
    }

    if op.name.contains("SCAN") {
        scan_rules(input, op, exec, share, &mut push);
    }
    if op.name == "HASH_JOIN_OPERATOR"
        || op.name.contains("NESTED_LOOP_JOIN") && !op.name.contains("SINK")
    {
        join_probe_rules(input, op, node.rows.map(|r| r as f64), share, &mut push);
    }
    if op.name == "HASH_JOIN_SINK_OPERATOR" {
        join_build_rules(input, op, share, &mut push);
    }
    if op.name == "STREAMING_AGGREGATION_OPERATOR"
        && let (Some(input_rows), Some(output)) =
            (input.input_rows(index), op.count("RowsProduced", Stat::Sum))
        && input_rows >= 1e6
        && output / input_rows >= 0.9
    {
        push(Finding::query("DORIS_STREAMING_AGG_INEFFECTIVE", text(lang, "Pre-aggregation barely reduces rows", "预聚合几乎没有减少行数"), Severity::Info,
                    text(lang, "Streaming aggregation passes almost every input row through.", "流式聚合几乎原样输出全部输入行。").into(),
                    format!("upstream RowsProduced={input_rows}, RowsProduced={output}, output/input={:.2}", output / input_rows),
                    vec![text(lang, "The grouping keys are close to unique per task; this phase mostly adds hashing cost. Check whether the GROUP BY keys are expected to be high-cardinality.", "分组键在各任务内接近唯一；该阶段主要增加哈希开销。确认 GROUP BY 键是否预期为高基数。").into()],
                    share, vec!["RowsProduced".into()]), &["RowsProduced"]);
    }
    if op.name == "AGGREGATION_OPERATOR" {
        if let Some(size) = op
            .count("HashTableSize", Stat::Sum)
            .filter(|size| *size >= 1e7)
        {
            push(Finding::query("DORIS_AGG_HIGH_CARDINALITY", text(lang, "High-cardinality aggregation", "高基数聚合"), Severity::Info,
                text(lang, "The aggregation hash table holds many distinct groups.", "聚合哈希表包含大量不同分组。").into(),
                format!("HashTableSize sum={size}"),
                vec![text(lang, "Filter earlier or aggregate on fewer/lower-cardinality keys; check whether the result needs every group.", "尽早过滤或使用更少/更低基数的分组键；确认结果是否需要所有分组。").into()],
                share, vec!["HashTableSize".into()]), &["HashTableSize"]);
        }
        if op
            .info("output")
            .is_some_and(|output| output.to_ascii_lowercase().contains("distinct"))
            && let Some(memory) = op
                .bytes("MemoryUsageHashTable", Stat::Max)
                .filter(|m| *m >= 256.0 * 1024.0 * 1024.0)
        {
            push(Finding::query("DORIS_COUNT_DISTINCT", text(lang, "Heavy DISTINCT aggregation", "DISTINCT 聚合开销大"), Severity::Info,
                    text(lang, "A DISTINCT aggregation keeps a large hash table.", "DISTINCT 聚合维持了较大的哈希表。").into(),
                    format!("output={}, MemoryUsageHashTable max={}", op.info("output").unwrap_or_default(), display_bytes(memory)),
                    vec![text(lang, "If an approximate answer is acceptable consider APPROX_COUNT_DISTINCT or a bitmap/HLL column.", "若可接受近似结果，考虑 APPROX_COUNT_DISTINCT 或 Bitmap/HLL 列。").into()],
                    share, vec!["MemoryUsageHashTable".into()]), &["MemoryUsageHashTable"]);
        }
    }
    if op.name == "SORT_SINK_OPERATOR"
        && let Some(rows) = op.count("InputRows", Stat::Sum).filter(|rows| *rows >= 1e7)
        && !input.has_limit(op)
    {
        push(Finding::query("DORIS_LARGE_SORT", text(lang, "Large sort without LIMIT", "无 LIMIT 的大排序"), Severity::Warning,
                    text(lang, "The query sorts many rows and keeps all of them.", "查询对大量行排序并保留全部结果。").into(),
                    format!("InputRows sum={rows}; no limit on plan node {:?}", op.plan_id),
                    vec![text(lang, "Add LIMIT if only the top rows are needed, or sort a pre-aggregated/filtered result.", "若只需前若干行请添加 LIMIT，或对预聚合/过滤后的结果排序。").into()],
                    share, vec!["InputRows".into()]), &["InputRows"]);
    }
    if op.name == "DATA_STREAM_SINK_OPERATOR" {
        for key in ["WaitForRpcBufferQueue", "WaitForBroadcastBuffer"] {
            if let Some(wait) = op.time(key, Stat::Avg).filter(|w| *w >= 100e6)
                && input.query_share(wait).is_some_and(|s| s >= 0.3)
            {
                push(Finding::query("DORIS_SINK_BACKPRESSURE", text(lang, "Exchange sender is blocked", "Exchange 发送端阻塞"), Severity::Info,
                        text(lang, "The sender waits for RPC buffers, so receivers or the network are slower than this fragment.", "发送端在等待 RPC 缓冲，接收端或网络慢于本 Fragment。").into(),
                        format!("{key} avg={}. Wait time is separate from ExecTime.", display_ns(wait)),
                        vec![text(lang, "Look at the receiving fragment's dominant operator and at network throughput between backends.", "查看接收端 Fragment 的主要耗时算子以及 BE 间网络吞吐。").into()],
                        input.query_share(wait), vec![key.into()]), &[key]);
                break;
            }
        }
        if let Some(bytes) = op
            .bytes("BytesSent", Stat::Sum)
            .filter(|b| *b >= 10.0 * GIB)
        {
            push(Finding::query("DORIS_EXCHANGE_VOLUME", text(lang, "Large network shuffle", "网络 Shuffle 数据量大"), Severity::Info,
                text(lang, "This exchange sends a large volume of data between backends.", "该 Exchange 在 BE 间发送了大量数据。").into(),
                format!("BytesSent sum={}", display_bytes(bytes)),
                vec![text(lang, "Project fewer columns and filter before the exchange; colocate or bucket-shuffle joins avoid re-shuffling.", "在 Exchange 前减少列并过滤；Colocate 或 Bucket Shuffle Join 可避免重新 Shuffle。").into()],
                share, vec!["BytesSent".into()]), &["BytesSent"]);
        }
    }

    if let Some(peak) = op.bytes("MemoryUsagePeak", Stat::Max) {
        let limit_share = input
            .exec_mem_limit
            .filter(|l| *l > 0.0)
            .map(|limit| peak / limit);
        let severity = match limit_share {
            Some(s) if s >= 0.8 => Some(Severity::Critical),
            Some(s) if s >= 0.5 => Some(Severity::Warning),
            None if peak >= 4.0 * GIB => Some(Severity::Warning),
            _ => None,
        };
        if let Some(severity) = severity {
            let limit_text = input
                .exec_mem_limit
                .map(|l| {
                    format!(
                        ", exec_mem_limit={} ({:.0}%)",
                        display_bytes(l),
                        limit_share.unwrap_or(0.0) * 100.0
                    )
                })
                .unwrap_or_default();
            let advice = if op.name.contains("JOIN") {
                text(
                    lang,
                    "Make the smaller input the build (right) side, filter it earlier, and check join statistics.",
                    "让较小的输入作为 Build（右）侧，提前过滤，并检查 Join 统计信息。",
                )
            } else if op.name.contains("AGGREGATION") {
                text(
                    lang,
                    "Reduce grouping cardinality or filter before aggregating.",
                    "降低分组基数或在聚合前过滤。",
                )
            } else if op.name.contains("SORT") {
                text(
                    lang,
                    "Add LIMIT or sort fewer columns/rows.",
                    "添加 LIMIT 或减少排序的行/列。",
                )
            } else {
                text(
                    lang,
                    "Reduce rows and columns reaching this operator.",
                    "减少进入该算子的行和列。",
                )
            };
            let mut finding = Finding::query(
                "DORIS_OPERATOR_MEMORY",
                text(lang, "High operator memory", "算子内存占用高"),
                severity,
                format!(
                    "{} {} {}",
                    node.operator_name,
                    text(lang, "peaked at", "峰值内存"),
                    display_bytes(peak)
                ),
                format!("MemoryUsagePeak max={}{limit_text}", display_bytes(peak)),
                vec![advice.into()],
                limit_share.or(share),
                vec!["MemoryUsagePeak".into()],
            );
            finding.related_variable = Some("exec_mem_limit".into());
            push(finding, &["MemoryUsagePeak"]);
        }
    }

    for (key, value) in &op.metrics {
        let lower = key.to_ascii_lowercase();
        if lower.contains("spill")
            && lower.contains("bytes")
            && let Some(bytes) = op
                .bytes(key, Stat::Sum)
                .or_else(|| op.bytes(key, Stat::Max))
                .filter(|b| *b > 0.0)
        {
            let mut finding = Finding::query("DORIS_SPILL", text(lang, "Operator spilled to disk", "算子发生落盘"), Severity::Warning,
                    text(lang, "This operator reports nonzero spill bytes.", "此算子报告了非零落盘字节数。").into(),
                    format!("{key}={} ({value})", display_bytes(bytes)),
                    vec![text(lang, "Reduce rows or columns before this operator; inspect sort/join cardinality and available memory before increasing query memory limits.", "减少进入算子的行数或列数；提高查询内存限制前检查排序/Join 基数和可用内存。").into()],
                    share, vec![key.clone()]);
            finding.related_variable = Some("exec_mem_limit".into());
            push(finding, &[key.as_str()]);
            break;
        }
    }
    out
}

fn scan_rules(
    input: &RuleInput,
    op: &RuntimeOperator,
    exec: Option<f64>,
    share: Option<f64>,
    push: &mut impl FnMut(Finding, &[&str]),
) {
    let lang = input.lang;
    if let (Some(init), Some(exec)) = (op.time("InitConnectorTime", Stat::Avg), exec)
        && init >= 5e6
        && init >= exec * 0.5
    {
        push(Finding::query("DORIS_CONNECTOR_INIT", text(lang, "Connector initialization dominates the scan", "连接器初始化占据扫描时间"), Severity::Info,
                text(lang, "Connector setup is the largest measured scan phase.", "连接器初始化是扫描中耗时最大的已测量阶段。").into(),
                format!("InitConnectorTime={}, ExecTime avg={}; setup/ExecTime={:.1}%", display_ns(init), display_ns(exec), init / exec * 100.0),
                vec![text(lang, "Check external connector connection reuse, driver loading and connection latency. Compare warm and cold runs before changing the SQL.", "检查外部连接器连接复用、驱动加载及连接延迟；修改 SQL 前比较冷、热执行。").into()],
                share, vec!["InitConnectorTime".into(), "ExecTime".into()]), &["InitConnectorTime", "ExecTime"]);
    }
    if let (Some((key, wait)), Some(exec)) = (op.dependency_wait(), exec)
        && wait >= 5e6
        && wait >= exec * 0.5
    {
        push(Finding::query("DORIS_SCAN_WAIT", text(lang, "Scan dependency wait", "扫描依赖等待"), Severity::Info,
                text(lang, "The scan spends substantial time waiting for its scanner dependency.", "扫描算子有较多时间在等待 Scanner 依赖。").into(),
                format!("{key} avg={}, ExecTime avg={}. Wait time is separate and must not be added to operator execution time.", display_ns(wait), display_ns(exec)),
                vec![text(lang, "Inspect scanner connector/IO counters and worker scheduling in DetailProfile; this wait alone does not prove a storage or network fault.", "检查 DetailProfile 中连接器、IO 及调度计数器；仅凭等待时间不能确认存储或网络故障。").into()],
                share, vec![key.clone()]), &[key.as_str()]);
    }
    // RowsRead: observed (4.1.3 JDBC detail) and documented for 2.1/3.x. RawRowsRead: documented,
    // https://doris.apache.org/docs/2.1/query-acceleration/optimization-technology-principle/topn-optimization/
    let read_key =
        if op.count("RowsRead", Stat::Sum).is_some() { "RowsRead" } else { "RawRowsRead" };
    if let (Some(read), Some(output)) =
        (op.count(read_key, Stat::Sum), op.count("RowsProduced", Stat::Sum))
        && read >= 1e5
        && output / read < 0.01
    {
        push(Finding::query("DORIS_SCAN_VOLUME", text(lang, "Large scan with low output", "扫描量大而输出少"), Severity::Warning,
                text(lang, "The scan reads far more rows than it produces.", "扫描读取的行数远多于输出行数。").into(),
                format!("{read_key}={read}, RowsProduced={output}; output/read={:.3}%", output / read * 100.0),
                vec![text(lang, "Check predicate pushdown, partition pruning and selected columns. Confirm whether this reduction is expected for the query.", "检查谓词下推、分区裁剪和所选列；确认该查询是否预期需要这种过滤比例。").into()],
                share, vec![read_key.into(), "RowsProduced".into()]), &[read_key, "RowsProduced"]);
    }
    // WaitForRuntimeFilter: observed (4.1.3 JDBC detail); runtime_filter_wait_time_ms is documented
    // at https://doris.apache.org/docs/2.1/query-acceleration/tuning/tuning-execution/adjustment-of-runtimefilter-wait-time/
    if let Some(wait) = op
        .time("WaitForRuntimeFilter", Stat::Max)
        .filter(|w| *w >= 100e6)
        && input.query_share(wait).is_some_and(|s| s >= 0.2)
    {
        let mut finding = Finding::query("DORIS_RUNTIME_FILTER_WAIT", text(lang, "Scan waited for runtime filters", "扫描等待 Runtime Filter"), Severity::Info,
                text(lang, "The scan delayed its start waiting for runtime filters from a join build side.", "扫描在等待 Join Build 侧的 Runtime Filter 时推迟启动。").into(),
                format!("WaitForRuntimeFilter max={}", display_ns(wait)),
                vec![text(lang, "If the build side is slow the filter may arrive too late to help; compare the build branch time with runtime_filter_wait_time_ms.", "若 Build 侧较慢，Filter 可能来得太晚；将 Build 分支耗时与 runtime_filter_wait_time_ms 对比。").into()],
                input.query_share(wait), vec!["WaitForRuntimeFilter".into()]);
        finding.related_variable = Some("runtime_filter_wait_time_ms".into());
        push(finding, &["WaitForRuntimeFilter"]);
    }
    // ScannerWorkerWaitTime: observed (4.1.3 JDBC detail) and documented for 2.1/3.x.
    if let Some(wait) = op
        .time("ScannerWorkerWaitTime", Stat::Max)
        .filter(|w| *w >= 100e6)
        && input.query_share(wait).is_some_and(|s| s >= 0.2)
    {
        push(Finding::query("DORIS_SCANNER_QUEUE", text(lang, "Scanners waited for worker threads", "Scanner 等待工作线程"), Severity::Info,
                text(lang, "Scanner tasks queued for the scanner thread pool.", "Scanner 任务在 Scanner 线程池中排队。").into(),
                format!("ScannerWorkerWaitTime max={}", display_ns(wait)),
                vec![text(lang, "The backend's scan threads were busy; check concurrent scans and workload group limits on that backend.", "BE 的扫描线程繁忙；检查该 BE 上的并发扫描和 Workload Group 限制。").into()],
                input.query_share(wait), vec!["ScannerWorkerWaitTime".into()]), &["ScannerWorkerWaitTime"]);
    }
    if op.name != "OLAP_SCAN_OPERATOR" {
        return;
    }
    // OLAP counters below: https://doris.apache.org/docs/2.1/query-acceleration/optimization-technology-principle/topn-optimization/
    const FILTERS: &[&str] = &[
        "RowsKeyRangeFiltered",
        "RowsStatsFiltered",
        "RowsBloomFilterFiltered",
        "RowsInvertedIndexFiltered",
        "RowsBitmapIndexFiltered",
        "RowsShortCircuitPredFiltered",
        "RowsVectorPredFiltered",
        "RowsConditionsFiltered",
        "RowsDelFiltered",
    ];
    if let (Some(raw), Some(output)) =
        (op.count("RawRowsRead", Stat::Sum), op.count("RowsProduced", Stat::Sum))
    {
        let filtered: Vec<_> = FILTERS
            .iter()
            .filter_map(|key| op.count(key, Stat::Sum).map(|v| format!("{key}={v}")))
            .collect();
        if raw >= 1e6 && output / raw < 0.01 {
            push(Finding::query("DORIS_OLAP_INDEX_FILTER", text(lang, "Indexes prune little before rows are read", "索引在读取前裁剪较少"), Severity::Warning,
                text(lang, "Most rows are read from storage and discarded afterwards.", "大部分行从存储读出后才被丢弃。").into(),
                format!("RawRowsRead={raw}, RowsProduced={output}; {}", filtered.join(", ")),
                vec![text(lang, "Put selective predicates on the key prefix, partition or bucket columns, or add a bloom filter / inverted index on the filtered column.", "将选择性谓词放在前缀键、分区或分桶列上，或为过滤列添加 BloomFilter/倒排索引。").into()],
                share, vec!["RawRowsRead".into()]), &["RawRowsRead"]);
        }
    }
    if let (Some(io), Some(exec)) = (op.time("IOTimer", Stat::Avg), exec)
        && io >= 10e6
        && io >= exec * 0.5
    {
        push(Finding::query("DORIS_OLAP_IO", text(lang, "Scan is storage-IO bound", "扫描受存储 IO 限制"), Severity::Info,
                text(lang, "Most scan time is spent in storage IO.", "扫描时间主要用于存储 IO。").into(),
                format!("IOTimer avg={}, ExecTime avg={}", display_ns(io), display_ns(exec)),
                vec![text(lang, "Read fewer columns, improve pruning, or check disk/remote storage latency and cache hit ratio.", "减少读取列、改进裁剪，或检查磁盘/远端存储延迟及缓存命中率。").into()],
                share, vec!["IOTimer".into()]), &["IOTimer"]);
    }
    if let (Some(segments), Some(tablets)) =
        (op.count("NumSegmentTotal", Stat::Sum), op.count("TabletNum", Stat::Sum))
        && segments >= 1000.0
        && tablets > 0.0
        && segments / tablets >= 50.0
    {
        push(Finding::query("DORIS_OLAP_SEGMENTS", text(lang, "Many segments per tablet", "每个 Tablet 的 Segment 过多"), Severity::Warning,
                text(lang, "Tablets have many segments, which suggests compaction lag or many small loads.", "Tablet 的 Segment 较多，可能存在 Compaction 滞后或大量小批导入。").into(),
                format!("NumSegmentTotal={segments}, TabletNum={tablets}, segments/tablet={:.1}", segments / tablets),
                vec![text(lang, "Check compaction status for this table (SHOW TABLET / compaction score) and batch small loads.", "检查该表的 Compaction 状态（SHOW TABLET / compaction score），并合并小批导入。").into()],
                share, vec!["NumSegmentTotal".into(), "TabletNum".into()]), &["NumSegmentTotal", "TabletNum"]);
    }
    if let (Some(cached), Some(total)) =
        (op.count("CachedPagesNum", Stat::Sum), op.count("TotalPagesNum", Stat::Sum))
        && total >= 1e4
        && cached / total < 0.1
    {
        push(Finding::query("DORIS_OLAP_PAGE_CACHE", text(lang, "Low page cache hit ratio", "Page Cache 命中率低"), Severity::Info,
                text(lang, "Few data pages were served from the page cache.", "很少的数据页命中 Page Cache。").into(),
                format!("CachedPagesNum={cached}, TotalPagesNum={total}, hit={:.1}%", cached / total * 100.0),
                vec![text(lang, "Expected on a cold run; if repeated runs stay cold, check the BE page cache size.", "冷启动时属正常；若重复执行仍未命中，检查 BE Page Cache 大小。").into()],
                share, vec!["CachedPagesNum".into(), "TotalPagesNum".into()]), &["CachedPagesNum", "TotalPagesNum"]);
    }
}

fn join_probe_rules(
    input: &RuleInput,
    op: &RuntimeOperator,
    output: Option<f64>,
    share: Option<f64>,
    push: &mut impl FnMut(Finding, &[&str]),
) {
    let lang = input.lang;
    if op.name.contains("NESTED_LOOP_JOIN") {
        push(Finding::query("DORIS_NESTED_LOOP_JOIN", text(lang, "Nested loop join", "嵌套循环 Join"), Severity::Warning,
            text(lang, "This join has no hash-joinable equality condition.", "该 Join 没有可哈希的等值条件。").into(),
            format!("{} RowsProduced={}", op.name, output.unwrap_or_default()),
            vec![text(lang, "Check the join condition: add an equality predicate or rewrite range/OR conditions if possible.", "检查 Join 条件：尽量添加等值谓词或改写范围/OR 条件。").into()],
            share, vec!["RowsProduced".into()]), &["RowsProduced"]);
        return;
    }
    if let (Some(probe), Some(output)) = (op.count("ProbeRows", Stat::Sum), output)
        && probe > 0.0
        && output >= 1e4
        && output / probe >= 10.0
    {
        let ratio = output / probe;
        let severity =
            if ratio >= 100.0 && output >= 1e7 { Severity::Critical } else { Severity::Warning };
        push(Finding::query("DORIS_JOIN_EXPANSION", text(lang, "Join row amplification", "Join 行数放大"), severity,
                text(lang, "The join produces at least ten rows per probe row.", "Join 每个 Probe 行至少产生十个输出行。").into(),
                format!("ProbeRows={probe}, RowsProduced={output}, output/probe={ratio:.2}"),
                vec![text(lang, "Verify join predicates and key uniqueness; check whether a many-to-many join is intended, and consider pre-aggregation or earlier filters.", "检查 Join 条件和键唯一性；确认多对多 Join 是否符合预期，并考虑预聚合或提前过滤。").into()],
                share.or(Some((ratio / 1000.0).min(1.0))), vec!["ProbeRows".into(), "RowsProduced".into()]), &["ProbeRows", "RowsProduced"]);
    }
    // The probe waits for its build side before it can start.
    if let Some((key, wait)) = op
        .dependency_wait()
        .filter(|(key, _)| key.contains("HASH_JOIN_PROBE"))
        && wait >= 100e6
        && input.query_share(wait).is_some_and(|s| s >= 0.5)
    {
        push(Finding::query("DORIS_JOIN_BUILD_WAIT", text(lang, "Probe waits for the build side", "Probe 等待 Build 侧"), Severity::Info,
                text(lang, "Most of the query is spent before this join's hash table is ready.", "查询大部分时间花在该 Join 哈希表就绪之前。").into(),
                format!("{key} avg={}", display_ns(wait)),
                vec![text(lang, "Look at the build (right) branch of this join: its scan and filters decide when probing can start.", "查看该 Join 的 Build（右）分支：其扫描和过滤决定 Probe 何时开始。").into()],
                input.query_share(wait), vec![key.clone()]), &[key.as_str()]);
    }
}

fn join_build_rules(
    input: &RuleInput,
    op: &RuntimeOperator,
    share: Option<f64>,
    push: &mut impl FnMut(Finding, &[&str]),
) {
    let lang = input.lang;
    let Some(probe) = input.paired(op, "HASH_JOIN_OPERATOR") else { return };
    let join_op = probe.info("join op").unwrap_or_default();
    let broadcast = join_op.contains("(BROADCAST)");
    // Broadcast: every instance sees the full build input (4.1.3 shows only one instance building a
    // shared hash table), so the per-instance max is the build size. Shuffle: the sum is.
    let build = op.count("InputRows", if broadcast { Stat::Max } else { Stat::Sum });
    let probe_rows = probe.count("ProbeRows", Stat::Sum);
    if let (Some(build), Some(probe_rows)) = (build, probe_rows)
        && build >= 1e6
        && build > probe_rows
    {
        push(Finding::query("DORIS_JOIN_BUILD_LARGER", text(lang, "Build side larger than probe side", "Build 侧大于 Probe 侧"), Severity::Warning,
                text(lang, "The hash table is built from the larger input.", "哈希表由较大的输入构建。").into(),
                format!("build InputRows={build}, ProbeRows={probe_rows}, join op={join_op}"),
                vec![text(lang, "The optimizer usually puts the smaller input on the build side; stale or missing statistics are a common cause. Run ANALYZE on the joined tables and compare the plan.", "优化器通常将较小输入放在 Build 侧；统计信息过期或缺失是常见原因。对相关表执行 ANALYZE 后比较计划。").into()],
                share, vec!["InputRows".into(), "ProbeRows".into()]), &["InputRows"]);
    }
    let memory = op.bytes("MemoryUsagePeak", Stat::Max);
    if broadcast && (build.is_some_and(|b| b >= 1e7) || memory.is_some_and(|m| m >= GIB)) {
        let mut finding = Finding::query("DORIS_BROADCAST_LARGE", text(lang, "Large broadcast join", "大表广播 Join"), Severity::Warning,
            text(lang, "A large build side is broadcast to every instance.", "较大的 Build 侧被广播到每个实例。").into(),
            format!("join op={join_op}, build InputRows max={}, MemoryUsagePeak max={}", build.unwrap_or_default(), memory.map(display_bytes).unwrap_or_else(|| "N/A".into())),
            vec![text(lang, "Check statistics first; if the plan is still a broadcast, try a shuffle hint and compare.", "先检查统计信息；若仍为广播，可尝试 shuffle hint 并比较。").into()],
            share, vec!["InputRows".into(), "MemoryUsagePeak".into()]);
        finding.sql_snippets.push(SqlSnippet {
            label: text(
                lang,
                "Join distribution hint (edit table names)",
                "Join 分布 Hint（请修改表名）",
            )
            .into(),
            sql: "SELECT ... FROM probe_table JOIN [shuffle] build_table ON ...".into(),
            scope: "statement".into(),
            warning: None,
        });
        push(finding, &["InputRows", "MemoryUsagePeak"]);
    }
    if join_op.contains("(PARTITIONED)")
        && let (Some(build), Some(probe_rows)) = (op.count("InputRows", Stat::Sum), probe_rows)
        && build <= 1e4
        && probe_rows >= 1e7
    {
        push(Finding::query("DORIS_SHUFFLE_SMALL_BUILD", text(lang, "Tiny build side is shuffled", "极小的 Build 侧被 Shuffle"), Severity::Info,
                    text(lang, "Both sides are shuffled although the build side is tiny.", "Build 侧很小，但两侧都被 Shuffle。").into(),
                    format!("join op={join_op}, build InputRows={build}, ProbeRows={probe_rows}"),
                    vec![text(lang, "A broadcast join would avoid shuffling the large probe side; check statistics, then try a broadcast hint.", "广播 Join 可避免 Shuffle 大的 Probe 侧；先检查统计信息，再尝试 broadcast hint。").into()],
                    share, vec!["InputRows".into(), "ProbeRows".into()]), &["InputRows"]);
    }
}

/// Lower confidence for running queries; used by the orchestrator.
pub fn cap_confidence(finding: &mut Finding, cap: Confidence) {
    finding.confidence = finding.confidence.min(cap);
}
