//! Query-level diagnostic rules (Q001-Q009)
//!
//! Rules that evaluate the entire query profile.

use super::{
    Lang, ParameterSuggestion, ParameterType, RuleSeverity, format_bytes, format_duration_ms,
    get_parameter_metadata, parse_duration_ms,
};
use crate::services::profile_analyzer::models::*;

/// Known default values for common StarRocks session parameters
fn get_parameter_default(name: &str) -> Option<&'static str> {
    match name {
        // DataCache related
        "enable_scan_datacache" => Some("true"),
        "enable_populate_datacache" => Some("true"),

        // Query optimization
        "enable_query_cache" => Some("false"),
        "enable_adaptive_sink_dop" => Some("false"),
        "enable_runtime_adaptive_dop" => Some("false"),
        "enable_spill" => Some("false"),

        // Parallelism
        "parallel_fragment_exec_instance_num" => Some("1"),
        "pipeline_dop" => Some("0"),
        "io_tasks_per_scan_operator" => Some("4"),

        _ => None,
    }
}

// Note: suggest_parameter_if_needed is replaced by QueryRuleContext::suggest_parameter

/// Query-level rule context containing all information needed for evaluation
pub struct QueryRuleContext<'a> {
    pub profile: &'a Profile,
    /// Live cluster variables (actual current values from cluster)
    pub cluster_variables: Option<&'a std::collections::HashMap<String, String>>,
    /// Language for user-facing diagnostic text
    pub lang: Lang,
}

impl<'a> QueryRuleContext<'a> {
    #[allow(dead_code)]
    pub fn new(profile: &'a Profile) -> Self {
        Self { profile, cluster_variables: None, lang: Lang::default() }
    }

    pub fn with_cluster_variables(
        profile: &'a Profile,
        cluster_variables: Option<&'a std::collections::HashMap<String, String>>,
        lang: Lang,
    ) -> Self {
        Self { profile, cluster_variables, lang }
    }

    /// Get current value of a parameter
    /// Priority: cluster_variables > non_default_variables > default
    pub fn get_variable_value(&self, name: &str) -> Option<String> {
        // First check live cluster variables (most accurate)
        if let Some(vars) = self.cluster_variables
            && let Some(value) = vars.get(name)
        {
            return Some(value.clone());
        }
        // Then check profile's non-default variables
        if let Some(info) = self.profile.summary.non_default_variables.get(name) {
            return Some(info.actual_value_str());
        }
        // Finally use known default
        get_parameter_default(name).map(|s| s.to_string())
    }

    /// Get current value as i64
    #[allow(dead_code)]
    pub fn get_variable_i64(&self, name: &str) -> Option<i64> {
        self.get_variable_value(name).and_then(|v| v.parse().ok())
    }

    /// Get current value as bool
    #[allow(dead_code)]
    pub fn get_variable_bool(&self, name: &str) -> Option<bool> {
        self.get_variable_value(name)
            .map(|v| v.eq_ignore_ascii_case("true"))
    }

    /// Create a smart parameter suggestion
    /// Returns None if current value already meets recommendation
    pub fn suggest_parameter(&self, name: &str) -> Option<ParameterSuggestion> {
        let cluster_info = self.profile.get_cluster_info();
        let current_str = self.get_variable_value(name);
        let current_i64 = current_str.as_ref().and_then(|v| v.parse::<i64>().ok());
        let current_bool = current_str.as_ref().map(|v| v.eq_ignore_ascii_case("true"));

        let (recommended, reason) = match name {
            "query_timeout" => {
                let current = current_i64.unwrap_or(300);
                if current >= 600 {
                    return None;
                }
                (
                    "600".to_string(),
                    self.lang
                        .pick(
                            "延长超时时间以支持复杂查询",
                            "Longer timeout to support complex queries",
                        )
                        .to_string(),
                )
            },

            "query_mem_limit" => {
                let current = current_i64.unwrap_or(0);
                let total_bytes = cluster_info.total_scan_bytes;
                let recommended = if total_bytes > 0 {
                    (total_bytes * 2).clamp(4 * 1024 * 1024 * 1024, 32 * 1024 * 1024 * 1024) as i64
                } else {
                    8 * 1024 * 1024 * 1024
                };
                if current >= recommended {
                    return None;
                }
                let gb = recommended / (1024 * 1024 * 1024);
                (
                    recommended.to_string(),
                    match self.lang {
                        Lang::Zh => format!("根据数据量推荐 {}GB 内存限制", gb),
                        Lang::En => {
                            format!("{}GB memory limit recommended for this data volume", gb)
                        },
                    },
                )
            },

            "enable_spill" => {
                if current_bool.unwrap_or(false) {
                    return None;
                }
                (
                    "true".to_string(),
                    self.lang
                        .pick("启用后可避免大查询 OOM", "Prevents OOM for large queries")
                        .to_string(),
                )
            },

            "pipeline_profile_level" => {
                let current = current_i64.unwrap_or(1);
                if current <= 1 {
                    return None;
                }
                (
                    "1".to_string(),
                    self.lang
                        .pick(
                            "降低 Profile 级别减少收集开销",
                            "A lower Profile level reduces collection overhead",
                        )
                        .to_string(),
                )
            },

            "pipeline_dop" => {
                let current = current_i64.unwrap_or(0);
                if current == 0 {
                    return None;
                }
                (
                    "0".to_string(),
                    self.lang
                        .pick("推荐使用自动模式", "Automatic mode is recommended")
                        .to_string(),
                )
            },

            "enable_scan_datacache" => {
                if current_bool.unwrap_or(true) {
                    return None;
                }
                (
                    "true".to_string(),
                    self.lang
                        .pick(
                            "启用 DataCache 提升存算分离性能",
                            "DataCache improves shared-data performance",
                        )
                        .to_string(),
                )
            },

            _ => return None,
        };

        let metadata = get_parameter_metadata(name, self.lang);
        let command = format!("SET {} = {};", name, recommended);
        Some(ParameterSuggestion {
            name: name.to_string(),
            param_type: ParameterType::Session,
            current: current_str, // Always has value from get_variable_value
            recommended,
            command,
            description: metadata.description,
            impact: format!("{} ({})", metadata.impact, reason),
        })
    }
}

/// Query-level rule trait
pub trait QueryRule: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self, lang: Lang) -> &str;
    /// Evaluate the rule with full context including cluster variables
    fn evaluate(&self, ctx: &QueryRuleContext) -> Option<QueryDiagnostic>;
}

/// Query-level diagnostic result
#[derive(Debug, Clone)]
pub struct QueryDiagnostic {
    pub rule_id: String,
    pub rule_name: String,
    pub severity: RuleSeverity,
    pub message: String,
    pub reason: String,
    pub suggestions: Vec<String>,
    pub parameter_suggestions: Vec<ParameterSuggestion>,
}

/// Q001: Query execution time too long
/// Condition: TotalTime > 60s
pub struct Q001LongRunning;

impl QueryRule for Q001LongRunning {
    fn id(&self) -> &str {
        "Q001"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("查询执行时间过长", "Query execution time too long")
    }

    fn evaluate(&self, ctx: &QueryRuleContext) -> Option<QueryDiagnostic> {
        let total_time_ms = ctx
            .profile
            .summary
            .total_time_ms
            .or_else(|| parse_duration_ms(&ctx.profile.summary.total_time))?;

        if total_time_ms > 60_000.0 {
            Some(QueryDiagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(ctx.lang).to_string(),
                severity: RuleSeverity::Warning,
                message: match ctx.lang {
                    Lang::Zh => format!(
                        "查询执行时间 {}，超过 60 秒阈值",
                        format_duration_ms(total_time_ms)
                    ),
                    Lang::En => format!(
                        "Query execution time {} exceeds the 60s threshold",
                        format_duration_ms(total_time_ms)
                    ),
                },
                reason: ctx
                    .lang
                    .pick(
                        "请参考 StarRocks 官方文档了解更多信息。",
                        "See the StarRocks documentation for more information.",
                    )
                    .to_string(),
                suggestions: vec![
                    ctx.lang
                        .pick("检查是否存在性能瓶颈算子", "Check for bottleneck operators")
                        .to_string(),
                    ctx.lang
                        .pick("考虑优化查询计划", "Consider optimizing the query plan")
                        .to_string(),
                    ctx.lang
                        .pick("检查是否存在数据倾斜", "Check for data skew")
                        .to_string(),
                ],
                parameter_suggestions: {
                    let mut suggestions = Vec::new();
                    if let Some(s) = ctx.suggest_parameter("query_timeout") {
                        suggestions.push(s);
                    }
                    if let Some(s) = ctx.suggest_parameter("query_mem_limit") {
                        suggestions.push(s);
                    }
                    suggestions
                },
            })
        } else {
            None
        }
    }
}

/// Q002: Query memory too high
/// Condition: QueryPeakMemory > 10GB
pub struct Q002HighMemory;

impl QueryRule for Q002HighMemory {
    fn id(&self) -> &str {
        "Q002"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("查询内存使用过高", "Query memory usage too high")
    }

    fn evaluate(&self, ctx: &QueryRuleContext) -> Option<QueryDiagnostic> {
        let peak_memory = ctx.profile.summary.query_peak_memory?;
        const TEN_GB: u64 = 10 * 1024 * 1024 * 1024;

        if peak_memory > TEN_GB {
            Some(QueryDiagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(ctx.lang).to_string(),
                severity: RuleSeverity::Warning,
                message: match ctx.lang {
                    Lang::Zh => {
                        format!("查询峰值内存 {}，超过 10GB 阈值", format_bytes(peak_memory))
                    },
                    Lang::En => format!(
                        "Query peak memory {} exceeds the 10GB threshold",
                        format_bytes(peak_memory)
                    ),
                },
                reason: ctx
                    .lang
                    .pick(
                        "请参考 StarRocks 官方文档了解更多信息。",
                        "See the StarRocks documentation for more information.",
                    )
                    .to_string(),
                suggestions: vec![
                    ctx.lang
                        .pick("检查是否存在大表 Join", "Check for Joins on large tables")
                        .to_string(),
                    ctx.lang
                        .pick("考虑启用 Spill 功能", "Consider enabling Spill")
                        .to_string(),
                    ctx.lang
                        .pick(
                            "优化查询减少中间结果",
                            "Optimize the query to reduce intermediate results",
                        )
                        .to_string(),
                ],
                parameter_suggestions: {
                    let mut suggestions = Vec::new();
                    if let Some(s) = ctx.suggest_parameter("enable_spill") {
                        suggestions.push(s);
                    }
                    if let Some(s) = ctx.suggest_parameter("query_mem_limit") {
                        suggestions.push(s);
                    }
                    suggestions
                },
            })
        } else {
            None
        }
    }
}

/// Q003: Query spill detected
/// Condition: QuerySpillBytes > 0
pub struct Q003QuerySpill;

impl QueryRule for Q003QuerySpill {
    fn id(&self) -> &str {
        "Q003"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("查询发生落盘", "Query spilled to disk")
    }

    fn evaluate(&self, ctx: &QueryRuleContext) -> Option<QueryDiagnostic> {
        let spill_bytes_str = ctx.profile.summary.query_spill_bytes.as_ref()?;

        // Parse spill bytes (e.g., "1.5 GB", "0.000 B")
        let spill_bytes = parse_spill_bytes(spill_bytes_str)?;

        if spill_bytes > 0 {
            Some(QueryDiagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(ctx.lang).to_string(),
                severity: RuleSeverity::Info,
                message: match ctx.lang {
                    Lang::Zh => {
                        format!("查询发生磁盘溢写，溢写数据量 {}", format_bytes(spill_bytes))
                    },
                    Lang::En => format!(
                        "Query spilled to disk; spilled data size {}",
                        format_bytes(spill_bytes)
                    ),
                },
                reason: ctx
                    .lang
                    .pick(
                        "请参考 StarRocks 官方文档了解更多信息。",
                        "See the StarRocks documentation for more information.",
                    )
                    .to_string(),
                suggestions: vec![
                    ctx.lang
                        .pick(
                            "增加内存限制以减少 Spill",
                            "Increase the memory limit to reduce Spill",
                        )
                        .to_string(),
                    ctx.lang
                        .pick(
                            "优化查询减少中间结果",
                            "Optimize the query to reduce intermediate results",
                        )
                        .to_string(),
                    ctx.lang
                        .pick("检查 Spill 是否影响性能", "Check whether Spill affects performance")
                        .to_string(),
                ],
                parameter_suggestions: {
                    let mut suggestions = Vec::new();
                    if let Some(s) = ctx.suggest_parameter("query_mem_limit") {
                        suggestions.push(s);
                    }
                    suggestions
                },
            })
        } else {
            None
        }
    }
}

/// Q005: Scan time dominates
/// Condition: ScanTime/TotalTime > 0.8
pub struct Q005ScanDominates;

impl QueryRule for Q005ScanDominates {
    fn id(&self) -> &str {
        "Q005"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("扫描时间占比过高", "Scan time ratio too high")
    }

    fn evaluate(&self, ctx: &QueryRuleContext) -> Option<QueryDiagnostic> {
        let scan_time_ms = ctx
            .profile
            .summary
            .query_cumulative_scan_time_ms
            .or_else(|| {
                ctx.profile
                    .summary
                    .query_cumulative_scan_time
                    .as_ref()
                    .and_then(|s| parse_duration_ms(s))
            })?;
        let total_time_ms = ctx
            .profile
            .summary
            .total_time_ms
            .or_else(|| parse_duration_ms(&ctx.profile.summary.total_time))?;

        if total_time_ms == 0.0 {
            return None;
        }

        let ratio = scan_time_ms / total_time_ms;

        if ratio > 0.8 {
            Some(QueryDiagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(ctx.lang).to_string(),
                severity: RuleSeverity::Warning,
                message: match ctx.lang {
                    Lang::Zh => format!("扫描时间占比 {:.1}%，查询瓶颈在数据扫描", ratio * 100.0),
                    Lang::En => format!(
                        "Scan time accounts for {:.1}%; the query bottleneck is data Scan",
                        ratio * 100.0
                    ),
                },
                reason: ctx
                    .lang
                    .pick(
                        "请参考 StarRocks 官方文档了解更多信息。",
                        "See the StarRocks documentation for more information.",
                    )
                    .to_string(),
                suggestions: vec![
                    ctx.lang
                        .pick(
                            "添加过滤条件减少扫描数据量",
                            "Add filter conditions to reduce the amount of data scanned",
                        )
                        .to_string(),
                    ctx.lang
                        .pick(
                            "检查分区裁剪是否生效",
                            "Check whether partition pruning takes effect",
                        )
                        .to_string(),
                    ctx.lang
                        .pick("考虑创建物化视图", "Consider creating a materialized view")
                        .to_string(),
                    ctx.lang
                        .pick("检查存储性能", "Check storage performance")
                        .to_string(),
                ],
                // Only suggest if not already enabled
                parameter_suggestions: ctx
                    .suggest_parameter("enable_scan_datacache")
                    .into_iter()
                    .collect(),
            })
        } else {
            None
        }
    }
}

/// Q006: Network time dominates
/// Condition: NetworkTime/TotalTime > 0.5
pub struct Q006NetworkDominates;

impl QueryRule for Q006NetworkDominates {
    fn id(&self) -> &str {
        "Q006"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("网络时间占比过高", "Network time ratio too high")
    }

    fn evaluate(&self, ctx: &QueryRuleContext) -> Option<QueryDiagnostic> {
        let network_time_ms = ctx
            .profile
            .summary
            .query_cumulative_network_time_ms
            .or_else(|| {
                ctx.profile
                    .summary
                    .query_cumulative_network_time
                    .as_ref()
                    .and_then(|s| parse_duration_ms(s))
            })?;
        let total_time_ms = ctx
            .profile
            .summary
            .total_time_ms
            .or_else(|| parse_duration_ms(&ctx.profile.summary.total_time))?;

        if total_time_ms == 0.0 {
            return None;
        }

        let ratio = network_time_ms / total_time_ms;

        if ratio > 0.5 {
            Some(QueryDiagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(ctx.lang).to_string(),
                severity: RuleSeverity::Warning,
                message: match ctx.lang {
                    Lang::Zh => format!("网络时间占比 {:.1}%，查询瓶颈在网络传输", ratio * 100.0),
                    Lang::En => format!(
                        "Network time accounts for {:.1}%; the query bottleneck is network transfer",
                        ratio * 100.0
                    ),
                },
                reason: ctx
                    .lang
                    .pick(
                        "请参考 StarRocks 官方文档了解更多信息。",
                        "See the StarRocks documentation for more information.",
                    )
                    .to_string(),
                suggestions: vec![
                    ctx.lang
                        .pick(
                            "考虑使用 Colocate Join 减少 Shuffle",
                            "Consider using Colocate Join to reduce Shuffle",
                        )
                        .to_string(),
                    ctx.lang
                        .pick("检查网络带宽", "Check network bandwidth")
                        .to_string(),
                    ctx.lang
                        .pick("减少跨节点数据传输", "Reduce cross-node data transfer")
                        .to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// Q004: CPU utilization low
pub struct Q004LowCPU;

impl QueryRule for Q004LowCPU {
    fn id(&self) -> &str {
        "Q004"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("CPU 利用率低", "Low CPU utilization")
    }

    fn evaluate(&self, ctx: &QueryRuleContext) -> Option<QueryDiagnostic> {
        let cpu_time = ctx.profile.summary.query_cumulative_cpu_time_ms?;
        let wall_time = ctx.profile.summary.query_execution_wall_time_ms?;
        if wall_time == 0.0 {
            return None;
        }
        let ratio = cpu_time / wall_time;
        if ratio < 0.3 {
            Some(QueryDiagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(ctx.lang).to_string(),
                severity: super::RuleSeverity::Warning,
                message: match ctx.lang {
                    Lang::Zh => {
                        format!("CPU 利用率仅 {:.1}%，可能存在等待或 IO 瓶颈", ratio * 100.0)
                    },
                    Lang::En => format!(
                        "CPU utilization is only {:.1}%; there may be waits or an IO bottleneck",
                        ratio * 100.0
                    ),
                },
                reason: ctx
                    .lang
                    .pick(
                        "请参考 StarRocks 官方文档了解更多信息。",
                        "See the StarRocks documentation for more information.",
                    )
                    .to_string(),
                suggestions: vec![
                    ctx.lang
                        .pick("检查是否存在等待", "Check for waits")
                        .to_string(),
                    ctx.lang
                        .pick("增加并行度", "Increase parallelism")
                        .to_string(),
                ],
                parameter_suggestions: {
                    let mut suggestions = Vec::new();
                    if let Some(s) = ctx.suggest_parameter("pipeline_dop") {
                        suggestions.push(s);
                    }
                    suggestions
                },
            })
        } else {
            None
        }
    }
}

/// Q007: Profile collection slow
pub struct Q007ProfileCollectSlow;

impl QueryRule for Q007ProfileCollectSlow {
    fn id(&self) -> &str {
        "Q007"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("Profile 收集慢", "Slow Profile collection")
    }

    fn evaluate(&self, ctx: &QueryRuleContext) -> Option<QueryDiagnostic> {
        // Check CollectProfileTime from variables or execution metrics
        let collect_time = ctx
            .profile
            .execution
            .metrics
            .get("CollectProfileTime")
            .and_then(|v| v.parse::<f64>().ok())?;
        if collect_time > 100_000_000.0 {
            // 100ms in ns
            Some(QueryDiagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(ctx.lang).to_string(),
                severity: super::RuleSeverity::Info,
                message: match ctx.lang {
                    Lang::Zh => format!("Profile 收集时间 {:.1}ms", collect_time / 1_000_000.0),
                    Lang::En => {
                        format!("Profile collection time {:.1}ms", collect_time / 1_000_000.0)
                    },
                },
                reason: ctx
                    .lang
                    .pick(
                        "请参考 StarRocks 官方文档了解更多信息。",
                        "See the StarRocks documentation for more information.",
                    )
                    .to_string(),
                suggestions: vec![
                    ctx.lang
                        .pick("降低 pipeline_profile_level", "Lower pipeline_profile_level")
                        .to_string(),
                ],
                parameter_suggestions: {
                    let mut suggestions = Vec::new();
                    if let Some(s) = ctx.suggest_parameter("pipeline_profile_level") {
                        suggestions.push(s);
                    }
                    suggestions
                },
            })
        } else {
            None
        }
    }
}

/// Q008: Schedule time too long
pub struct Q008ScheduleTimeLong;

impl QueryRule for Q008ScheduleTimeLong {
    fn id(&self) -> &str {
        "Q008"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("调度时间过长", "Schedule time too long")
    }

    fn evaluate(&self, ctx: &QueryRuleContext) -> Option<QueryDiagnostic> {
        let schedule_time = ctx.profile.summary.query_peak_schedule_time_ms?;
        let wall_time = ctx.profile.summary.query_execution_wall_time_ms?;
        if wall_time == 0.0 {
            return None;
        }
        let ratio = schedule_time / wall_time;
        if ratio > 0.3 {
            Some(QueryDiagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(ctx.lang).to_string(),
                severity: super::RuleSeverity::Warning,
                message: match ctx.lang {
                    Lang::Zh => {
                        format!("调度时间占比 {:.1}%，Pipeline 调度可能存在瓶颈", ratio * 100.0)
                    },
                    Lang::En => format!(
                        "Schedule time accounts for {:.1}%; Pipeline scheduling may be a bottleneck",
                        ratio * 100.0
                    ),
                },
                reason: ctx
                    .lang
                    .pick(
                        "请参考 StarRocks 官方文档了解更多信息。",
                        "See the StarRocks documentation for more information.",
                    )
                    .to_string(),
                suggestions: vec![
                    ctx.lang
                        .pick("检查 Pipeline 调度瓶颈", "Check for Pipeline scheduling bottlenecks")
                        .to_string(),
                    ctx.lang
                        .pick("增加并行度", "Increase parallelism")
                        .to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// Q009: Result delivery slow
pub struct Q009ResultDeliverySlow;

impl QueryRule for Q009ResultDeliverySlow {
    fn id(&self) -> &str {
        "Q009"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("结果传输慢", "Slow result delivery")
    }

    fn evaluate(&self, ctx: &QueryRuleContext) -> Option<QueryDiagnostic> {
        // Check ResultDeliverTime from execution metrics
        let deliver_time = ctx
            .profile
            .execution
            .metrics
            .get("ResultDeliverTime")
            .and_then(|v| v.parse::<f64>().ok())?;
        let wall_time = ctx.profile.summary.query_execution_wall_time_ms? * 1_000_000.0; // to ns
        if wall_time == 0.0 {
            return None;
        }
        let ratio = deliver_time / wall_time;
        if ratio > 0.2 {
            Some(QueryDiagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(ctx.lang).to_string(),
                severity: super::RuleSeverity::Info,
                message: match ctx.lang {
                    Lang::Zh => format!("结果传输时间占比 {:.1}%", ratio * 100.0),
                    Lang::En => format!("Result delivery time accounts for {:.1}%", ratio * 100.0),
                },
                reason: ctx
                    .lang
                    .pick(
                        "请参考 StarRocks 官方文档了解更多信息。",
                        "See the StarRocks documentation for more information.",
                    )
                    .to_string(),
                suggestions: vec![
                    ctx.lang
                        .pick("检查网络带宽", "Check network bandwidth")
                        .to_string(),
                    ctx.lang
                        .pick("减少结果集大小", "Reduce the result set size")
                        .to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// Parse spill bytes string (e.g., "1.5 GB", "0.000 B")
fn parse_spill_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    let parts: Vec<&str> = s.split_whitespace().collect();

    if parts.len() != 2 {
        return None;
    }

    let value: f64 = parts[0].parse().ok()?;
    let unit = parts[1].to_uppercase();

    let multiplier = match unit.as_str() {
        "B" => 1u64,
        "KB" | "K" => 1024,
        "MB" | "M" => 1024 * 1024,
        "GB" | "G" => 1024 * 1024 * 1024,
        "TB" | "T" => 1024 * 1024 * 1024 * 1024,
        _ => return None,
    };

    Some((value * multiplier as f64) as u64)
}

/// Get all query-level rules
pub fn get_rules() -> Vec<Box<dyn QueryRule>> {
    vec![
        Box::new(Q001LongRunning),
        Box::new(Q002HighMemory),
        Box::new(Q003QuerySpill),
        Box::new(Q004LowCPU),
        Box::new(Q005ScanDominates),
        Box::new(Q006NetworkDominates),
        Box::new(Q007ProfileCollectSlow),
        Box::new(Q008ScheduleTimeLong),
        Box::new(Q009ResultDeliverySlow),
    ]
}
