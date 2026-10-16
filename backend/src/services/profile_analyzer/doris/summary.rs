//! `Summary`, `Execution Summary` and `ChangedSessionVariables` parsing and query-phase rules.
//!
//! Key names are those observed in the Doris 4.1.3 fixtures (docs/profile/doris-counter-catalog.md).
use super::findings::{Finding, Severity};
use super::metric::{Unit, display_ns, quantity};
use super::version::DorisVersion;
use crate::services::profile_analyzer::models::{PhaseBreakdown, PhaseSegment};
use crate::utils::i18n::Lang;
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct BackendRpc {
    pub phase: String,
    pub backend: String,
    pub work_ns: Option<f64>,
    pub fe_to_be_ns: Option<f64>,
    pub queue_ns: Option<f64>,
    pub be_to_fe_ns: Option<f64>,
}

impl BackendRpc {
    pub fn total_ns(&self) -> Option<f64> {
        let parts = [self.work_ns, self.fe_to_be_ns, self.queue_ns, self.be_to_fe_ns];
        parts
            .iter()
            .any(Option::is_some)
            .then(|| parts.iter().flatten().sum())
    }
}

#[derive(Clone, Debug, Default)]
pub struct DorisSummary {
    pub fields: HashMap<String, String>,
    pub sql: String,
    pub version: Option<DorisVersion>,
    pub version_raw: Option<String>,
    pub changed_variables: HashMap<String, String>,
    pub backend_rpc: Vec<BackendRpc>,
}

/// Nereids planner stages printed directly under `Plan Time` in 4.1.3.
const PLAN_STAGES: &[&str] = &[
    "Garbage Collect During Plan Time",
    "Nereids Preload External Metadata Time",
    "Nereids Lock Table Time",
    "Nereids Analysis Time",
    "Nereids Rewrite Time",
    "Nereids Collect Table Partition Time",
    "Nereids Pre Rewrite By Mv Time",
    "Nereids Optimize Time",
    "Nereids Translate Time",
    "Nereids Distribute Time",
];
/// Stages that measure external catalog metadata or split planning.
const EXTERNAL_METADATA: &[&str] = &[
    "Nereids Preload External Metadata Time",
    "Nereids Collect Table Partition Time",
    "Get Splits Time",
    "Get Partitions Time",
    "Get Partition Files Time",
    "Create Scan Range Time",
];

impl DorisSummary {
    pub fn parse(profile_text: &str) -> Self {
        let header = profile_text
            .split("MergedProfile:")
            .next()
            .unwrap_or(profile_text)
            .split("DetailProfile")
            .next()
            .unwrap_or(profile_text);
        let mut summary = Self::default();
        let mut in_sql = false;
        let mut variables_json = None::<String>;
        for line in header.lines() {
            let trimmed = line.trim();
            if trimmed == "ChangedSessionVariables:" {
                variables_json = Some(String::new());
                in_sql = false;
                continue;
            }
            if let Some(json) = variables_json.as_mut() {
                json.push_str(line);
                json.push('\n');
                continue;
            }
            if let Some(entry) = trimmed.strip_prefix("- ") {
                in_sql = false;
                let Some((key, value)) = entry.split_once(':') else { continue };
                let (key, value) = (key.trim(), value.trim());
                if key == "Sql Statement" {
                    summary.sql = value.to_string();
                    in_sql = true;
                }
                // The first value wins: nested keys never overwrite top-level ones.
                summary
                    .fields
                    .entry(key.to_string())
                    .or_insert_with(|| value.to_string());
            } else if in_sql && !trimmed.is_empty() && !trimmed.ends_with(':') {
                summary.sql.push('\n');
                summary.sql.push_str(line);
            } else if in_sql && trimmed.ends_with(':') && !line.starts_with(' ') {
                in_sql = false;
            }
        }
        if let Some(json) = variables_json
            .as_deref()
            .and_then(|json| json.find('[').map(|start| &json[start..]))
            .and_then(|json| json.rfind(']').map(|end| &json[..=end]))
            && let Ok(serde_json::Value::Array(items)) = serde_json::from_str(json)
        {
            for item in items {
                if let (Some(name), Some(value)) = (
                    item.get("VarName").and_then(|v| v.as_str()),
                    item.get("CurrentValue").and_then(|v| v.as_str()),
                ) {
                    summary
                        .changed_variables
                        .insert(name.to_string(), value.to_string());
                }
            }
        }
        summary.version_raw = summary.fields.get("Doris Version").cloned();
        summary.version = summary.version_raw.as_deref().and_then(DorisVersion::parse);
        if let Some(json) = summary.fields.get("Schedule Time Of BE").cloned() {
            summary.backend_rpc = parse_backend_rpc(&json);
        }
        summary
    }

    pub fn field(&self, key: &str) -> Option<&str> {
        self.fields
            .get(key)
            .map(String::as_str)
            .filter(|value| !super::metric::is_unknown(value))
    }

    pub fn time_ns(&self, key: &str) -> Option<f64> {
        match quantity(self.field(key)?, None)? {
            (value, Unit::Duration) => Some(value),
            // "0" without a unit is printed for some empty timers.
            (0.0, Unit::Count) => Some(0.0),
            _ => None,
        }
    }

    pub fn total_ns(&self) -> Option<f64> {
        self.time_ns("Total")
    }

    pub fn task_state(&self) -> Option<&str> {
        self.field("Task State")
    }

    pub fn is_running(&self) -> bool {
        self.task_state().is_some_and(|state| {
            matches!(
                state.to_ascii_uppercase().as_str(),
                "RUNNING" | "PENDING" | "QUEUED" | "WAITING"
            )
        })
    }

    /// Only an explicit `profile_level` in ChangedSessionVariables is trusted here.
    pub fn profile_level(&self) -> Option<u8> {
        self.changed_variables
            .get("profile_level")
            .and_then(|value| value.trim().parse().ok())
    }

    pub fn phase_breakdown(&self, lang: Lang) -> Option<PhaseBreakdown> {
        let total = self.total_ns();
        let stacked = [
            ("parse", "Parse SQL Time", "Parse", "解析"),
            ("plan", "Plan Time", "Plan", "规划"),
            ("meta", "Get Meta Version Time", "Meta version", "元数据版本"),
            ("schedule", "Schedule Time", "Schedule", "调度"),
            ("execute_fetch", "Wait and Fetch Result Time", "Execute + fetch", "执行与取结果"),
        ];
        let segments: Vec<_> = stacked
            .iter()
            .filter_map(|(key, field, en, zh)| {
                self.time_ns(field).map(|ns| PhaseSegment {
                    key: (*key).into(),
                    label: text(lang, en, zh).into(),
                    time_ms: ns / 1e6,
                    raw: self.fields[*field].clone(),
                })
            })
            .collect();
        if segments.is_empty() {
            return None;
        }
        let details = [
            ("fetch", "Fetch Result Time", "Fetch result", "取结果"),
            ("write", "Write Result Time", "Write result to client", "写回客户端"),
        ]
        .iter()
        .filter_map(|(key, field, en, zh)| {
            self.time_ns(field).map(|ns| PhaseSegment {
                key: (*key).into(),
                label: text(lang, en, zh).into(),
                time_ms: ns / 1e6,
                raw: self.fields[*field].clone(),
            })
        })
        .collect();
        let sum: f64 = segments.iter().map(|segment| segment.time_ms).sum();
        let total_ms = total.map(|ns| ns / 1e6);
        // FE timers are rounded to whole milliseconds; allow that rounding before declaring overlap.
        let additive = total_ms.is_none_or(|total| sum <= total + segments.len() as f64);
        Some(PhaseBreakdown {
            total_ms,
            unaccounted_ms: total_ms
                .filter(|_| additive)
                .map(|total| (total - sum).max(0.0)),
            additive,
            segments,
            details,
            note: text(
                lang,
                "Measured FE phases from Execution Summary. 'Execute + fetch' includes waiting for BE execution, so it is not client time. Fetch/write values are shown separately because their nesting is not printed.",
                "来自 Execution Summary 的 FE 阶段测量值。“执行与取结果”包含等待 BE 执行，并非客户端耗时。取结果/写回因嵌套关系未打印而单独展示。",
            )
            .into(),
        })
    }

    /// Query-level findings that do not belong to one operator.
    pub fn findings(&self, lang: Lang) -> Vec<Finding> {
        let mut findings = Vec::new();
        let total = self.total_ns();
        let share = |value: f64| total.filter(|t| *t > 0.0).map(|t| value / t);

        if let Some(plan) = self.time_ns("Plan Time") {
            if plan >= 100e6 && share(plan).is_some_and(|s| s >= 0.3) {
                let stages: Vec<_> = PLAN_STAGES
                    .iter()
                    .filter_map(|stage| self.time_ns(stage).map(|ns| (*stage, ns)))
                    .collect();
                let dominant = stages.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1));
                let dominant_text = dominant
                    .map(|(stage, ns)| format!("; largest stage {stage} = {}", display_ns(ns)))
                    .unwrap_or_default();
                let advice = match dominant.map(|(stage, _)| stage) {
                    Some("Nereids Lock Table Time") => text(
                        lang,
                        "Planning waited on table locks; check concurrent DDL, loads or schema changes on the same tables.",
                        "规划阶段在等待表锁；检查同一表上的并发 DDL、导入或 Schema Change。",
                    ),
                    Some("Nereids Pre Rewrite By Mv Time") => text(
                        lang,
                        "Materialized-view rewrite dominates planning; review how many MVs are candidates for these tables.",
                        "物化视图改写占据规划时间；检查这些表的候选物化视图数量。",
                    ),
                    Some(
                        "Nereids Preload External Metadata Time"
                        | "Nereids Collect Table Partition Time",
                    ) => text(
                        lang,
                        "External catalog metadata loading dominates planning; check catalog metadata cache settings and metastore latency.",
                        "外部 Catalog 元数据加载占据规划时间；检查 Catalog 元数据缓存设置和元数据服务延迟。",
                    ),
                    Some("Nereids Optimize Time" | "Nereids Rewrite Time") => text(
                        lang,
                        "Optimizer search dominates planning; very large join graphs, many OR/IN predicates or deep views increase it.",
                        "优化器搜索占据规划时间；超大 Join 图、大量 OR/IN 谓词或多层视图会增加该时间。",
                    ),
                    _ => text(
                        lang,
                        "Compare plan time across runs of the same SQL before changing anything.",
                        "调整前先比较同一 SQL 多次执行的规划时间。",
                    ),
                };
                findings.push(Finding::query(
                    "DORIS_PLAN_TIME",
                    text(lang, "Planning takes a large share of the query", "规划耗时占比高"),
                    Severity::Warning,
                    format!(
                        "{} {:.0}% {}",
                        text(lang, "Plan Time is", "Plan Time 占"),
                        share(plan).unwrap_or(0.0) * 100.0,
                        text(lang, "of Total", "的总耗时")
                    ),
                    format!(
                        "Plan Time = {}, Total = {}{dominant_text}",
                        display_ns(plan),
                        display_ns(total.unwrap_or(0.0))
                    ),
                    vec![advice.into()],
                    share(plan),
                    vec!["Plan Time".into()],
                ));
            }
            let external: f64 = EXTERNAL_METADATA
                .iter()
                .filter_map(|key| self.time_ns(key))
                .sum();
            if external >= 50e6 && external >= plan * 0.5 {
                findings.push(Finding::query(
                    "DORIS_EXTERNAL_METADATA",
                    text(lang, "External metadata lookup dominates planning", "外部元数据查询占据规划时间"),
                    Severity::Info,
                    text(lang, "Most planning time is spent loading external table metadata, partitions or splits.", "规划时间主要用于加载外部表元数据、分区或分片。").into(),
                    format!("external metadata/split timers = {}, Plan Time = {}", display_ns(external), display_ns(plan)),
                    vec![text(lang, "Check the catalog's metadata cache, partition count and file listing cost; compare a warm second run.", "检查 Catalog 元数据缓存、分区数量和文件列举开销；对比第二次热执行。").into()],
                    share(external),
                    EXTERNAL_METADATA.iter().map(|key| key.to_string()).collect(),
                ));
            }
        }

        if let Some(schedule) = self.time_ns("Schedule Time")
            && schedule >= 100e6
            && share(schedule).is_some_and(|s| s >= 0.3)
        {
            let phases = [
                "Fragment Assign Time",
                "Fragment Serialize Time",
                "Fragment RPC Phase1 Time",
                "Fragment RPC Phase2 Time",
            ]
            .iter()
            .filter_map(|key| {
                self.time_ns(key)
                    .map(|ns| format!("{key} = {}", display_ns(ns)))
            })
            .collect::<Vec<_>>()
            .join(", ");
            findings.push(Finding::query(
                    "DORIS_SCHEDULE_TIME",
                    text(lang, "Fragment scheduling takes a large share", "Fragment 调度耗时占比高"),
                    Severity::Warning,
                    format!("{} {:.0}% {}", text(lang, "Schedule Time is", "Schedule Time 占"), share(schedule).unwrap_or(0.0) * 100.0, text(lang, "of Total", "的总耗时")),
                    format!("Schedule Time = {}; {phases}", display_ns(schedule)),
                    vec![text(lang, "Check FE-to-BE RPC latency and BE load in Schedule Time Of BE; very many instances also increase serialize and RPC time.", "检查 Schedule Time Of BE 中的 FE 到 BE RPC 延迟和 BE 负载；实例数过多也会增加序列化和 RPC 时间。").into()],
                    share(schedule),
                    vec!["Schedule Time".into(), "Schedule Time Of BE".into()],
                ));
        }

        for phase in ["phase1", "phase2"] {
            let rpcs: Vec<_> = self
                .backend_rpc
                .iter()
                .filter(|rpc| rpc.phase == phase)
                .collect();
            if let Some(slow) = slowest_backend(&rpcs) {
                findings.push(Finding::query(
                    "DORIS_SLOW_BE_RPC",
                    text(lang, "One backend is slow to accept fragments", "单个 BE 接收 Fragment 较慢"),
                    Severity::Warning,
                    format!("{} {}", text(lang, "Slow fragment RPC on", "Fragment RPC 较慢的 BE:"), slow.0.backend),
                    format!("{phase}: {} total RPC {} vs median {}", slow.0.backend, display_ns(slow.1), display_ns(slow.2)),
                    vec![text(lang, "Check that backend's CPU, brpc thread pool, GC/memory pressure and network path.", "检查该 BE 的 CPU、brpc 线程池、内存压力和网络链路。").into()],
                    share(slow.1),
                    vec!["Schedule Time Of BE".into()],
                ));
            }
            if let Some(queued) = rpcs
                .iter()
                .filter_map(|rpc| rpc.queue_ns.map(|q| (rpc, q)))
                .filter(|(_, q)| *q >= 50e6)
                .max_by(|a, b| a.1.total_cmp(&b.1))
            {
                findings.push(Finding::query(
                    "DORIS_BE_RPC_QUEUE",
                    text(lang, "Fragment RPC queued on a backend", "Fragment RPC 在 BE 排队"),
                    Severity::Warning,
                    text(lang, "A backend's RPC work queue delayed fragment startup.", "BE 的 RPC 工作队列延迟了 Fragment 启动。").into(),
                    format!("{phase}: {} RPC Work Queue Time = {}", queued.0.backend, display_ns(queued.1)),
                    vec![text(lang, "The backend was busy with other RPC work; check concurrent queries and loads on it.", "该 BE 正忙于其他 RPC 工作；检查其上的并发查询和导入。").into()],
                    share(queued.1),
                    vec!["Schedule Time Of BE".into()],
                ));
            }
        }

        if let Some(write) = self.time_ns("Write Result Time")
            && write >= 100e6
            && share(write).is_some_and(|s| s >= 0.3)
        {
            findings.push(Finding::query(
                    "DORIS_RESULT_WRITE",
                    text(lang, "Sending results to the client is slow", "向客户端发送结果较慢"),
                    Severity::Info,
                    format!("{} {:.0}% {}", text(lang, "Write Result Time is", "Write Result Time 占"), share(write).unwrap_or(0.0) * 100.0, text(lang, "of Total", "的总耗时")),
                    format!("Write Result Time = {}, Total = {}", display_ns(write), display_ns(total.unwrap_or(0.0))),
                    vec![text(lang, "Large result sets or a slow client/network dominate; add LIMIT, select fewer columns, or export with SELECT INTO OUTFILE.", "结果集过大或客户端/网络较慢；可添加 LIMIT、减少列，或使用 SELECT INTO OUTFILE 导出。").into()],
                    share(write),
                    vec!["Write Result Time".into()],
                ));
        }

        if let Some(failure) = self.failure(lang) {
            findings.push(failure);
        }
        findings
    }

    /// Map a failed task state and its message to a known cause. Unknown messages are reported
    /// verbatim without a guessed cause.
    fn failure(&self, lang: Lang) -> Option<Finding> {
        let state = self.task_state()?;
        if matches!(
            state.to_ascii_uppercase().as_str(),
            "OK" | "EOF" | "FINISHED" | "RUNNING" | "PENDING" | "QUEUED" | "WAITING"
        ) {
            return None;
        }
        let message = self.field("System Message").unwrap_or_default();
        let haystack = format!("{state} {message}").to_ascii_lowercase();
        // Status names from be/src/common/status.h (MEM_LIMIT_EXCEEDED, TIMEOUT, CANCELLED).
        let cause = if haystack.contains("mem_limit_exceeded")
            || haystack.contains("memory limit exceeded")
        {
            Some((
                text(lang, "Memory limit exceeded", "超出内存限制"),
                text(
                    lang,
                    "Reduce the rows/columns reaching joins, aggregations and sorts, check the operator memory peaks below, then consider exec_mem_limit or spill (4.x).",
                    "减少进入 Join、聚合和排序的行/列，查看下方算子内存峰值，再考虑 exec_mem_limit 或落盘（4.x）。",
                ),
                "exec_mem_limit",
            ))
        } else if haystack.contains("timeout") || haystack.contains("timed out") {
            Some((
                text(lang, "Query timed out", "查询超时"),
                text(
                    lang,
                    "Find the dominant operators below first; raise query_timeout only after confirming the work is expected.",
                    "先定位下方的主要耗时算子；确认工作量符合预期后再提高 query_timeout。",
                ),
                "query_timeout",
            ))
        } else if haystack.contains("cancel") {
            Some((
                text(lang, "Query was cancelled", "查询被取消"),
                text(
                    lang,
                    "Cancellation can come from a user/KILL, a client disconnect or another fragment's failure; check the FE log for the first error.",
                    "取消可能来自用户/KILL、客户端断开或其他 Fragment 失败；检查 FE 日志中的首个错误。",
                ),
                "",
            ))
        } else {
            None
        };
        let evidence = if message.is_empty() {
            format!("Task State = {state}")
        } else {
            format!("Task State = {state}; System Message = {message}")
        };
        Some(match cause {
            Some((title, advice, variable)) => {
                let mut finding = Finding::query("DORIS_QUERY_FAILED", title, Severity::Critical, title.to_string(), evidence, vec![advice.into()], None, vec!["Task State".into(), "System Message".into()]);
                finding.related_variable = (!variable.is_empty()).then(|| variable.to_string());
                finding
            },
            None => Finding::query(
                "DORIS_QUERY_FAILED",
                text(lang, "Query did not finish successfully", "查询未成功完成"),
                Severity::Warning,
                text(lang, "The failure message is not recognized; no cause is inferred.", "无法识别失败信息，未推断原因。").into(),
                evidence,
                vec![text(lang, "Read the full error in the FE log or the client; diagnostics below may be partial.", "查看 FE 日志或客户端中的完整错误；下方诊断可能不完整。").into()],
                None,
                vec!["Task State".into()],
            ),
        })
    }
}

/// A backend whose RPC total is at least 50 ms and three times the median of the others.
fn slowest_backend<'a>(rpcs: &[&'a BackendRpc]) -> Option<(&'a BackendRpc, f64, f64)> {
    if rpcs.len() < 2 {
        return None;
    }
    let mut totals: Vec<_> = rpcs
        .iter()
        .filter_map(|rpc| rpc.total_ns().map(|t| (*rpc, t)))
        .collect();
    if totals.len() < 2 {
        return None;
    }
    totals.sort_by(|a, b| a.1.total_cmp(&b.1));
    let (slow, slow_total) = *totals.last()?;
    let others: Vec<_> = totals[..totals.len() - 1].iter().map(|(_, t)| *t).collect();
    let median = others[others.len() / 2];
    (slow_total >= 50e6 && slow_total >= median.max(1e6) * 3.0)
        .then_some((slow, slow_total, median))
}

fn rpc_time(value: Option<&serde_json::Value>) -> Option<f64> {
    match quantity(value?.as_str()?, None)? {
        (ns, Unit::Duration) => Some(ns),
        _ => None,
    }
}

/// Parse `{"phase1":{"host: port":{"RPC Work Time":"4ms",...}}}`. Malformed JSON yields nothing.
pub fn parse_backend_rpc(json: &str) -> Vec<BackendRpc> {
    let Ok(serde_json::Value::Object(phases)) = serde_json::from_str::<serde_json::Value>(json)
    else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for (phase, backends) in phases {
        let Some(backends) = backends.as_object() else { continue };
        for (backend, timings) in backends {
            result.push(BackendRpc {
                phase: phase.clone(),
                backend: backend.replace(": ", ":"),
                work_ns: rpc_time(timings.get("RPC Work Time")),
                fe_to_be_ns: rpc_time(timings.get("RPC Latency From FE To BE")),
                queue_ns: rpc_time(timings.get("RPC Work Queue Time")),
                be_to_fe_ns: rpc_time(timings.get("RPC Latency From BE To FE")),
            });
        }
    }
    result
}

fn text<'a>(lang: Lang, en: &'a str, zh: &'a str) -> &'a str {
    if lang == Lang::Zh { zh } else { en }
}
