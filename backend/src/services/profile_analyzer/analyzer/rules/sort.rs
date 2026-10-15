//! Sort/Merge operator diagnostic rules (T001-T005, W001)
//!
//! Rules for SORT and ANALYTIC (Window) operators.

use super::*;

/// T001: Sort rows too large
/// Condition: InputRowNum > 10M without LIMIT
pub struct T001SortRowsTooLarge;

impl DiagnosticRule for T001SortRowsTooLarge {
    fn id(&self) -> &str {
        "T001"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("排序行数过多", "Too many rows to sort")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SORT")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let input_rows = context.node.metrics.push_row_num? as f64;

        if input_rows > 10_000_000.0 {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Warning,
                node_path: format!("{} (plan_node_id={})", 
                    context.node.operator_name,
                    context.node.plan_node_id.unwrap_or(-1)),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
                    Lang::Zh => format!("排序行数过多 ({:.0} 行)，可能导致性能问题", input_rows),
                    Lang::En => format!("Too many rows to sort ({:.0} rows), which may cause performance issues", input_rows),
                },
                reason: context.lang.pick("排序数据量过大，消耗大量 CPU 和内存资源。考虑添加过滤条件减少排序数据量或使用 TopN 优化。", "The sort data volume is too large and consumes significant CPU and memory. Consider adding filters to reduce the data to sort, or use TopN optimization.").to_string(),
                suggestions: vec![
                    context.lang.pick("添加 LIMIT 限制结果集大小", "Add a LIMIT to restrict the result set size").to_string(),
                    context.lang.pick("检查是否可以使用 Top-N 优化", "Check whether Top-N optimization can be used").to_string(),
                    context.lang.pick("考虑使用物化视图预排序", "Consider pre-sorting with a materialized view").to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// T002: Sort spill detected
/// Condition: SpillBytes > 0
pub struct T002SortSpill;

impl DiagnosticRule for T002SortSpill {
    fn id(&self) -> &str {
        "T002"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("排序发生落盘", "Sort Spill to disk")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SORT")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let spill_bytes = context
            .get_metric("SpillBytes")
            .or_else(|| context.get_metric("OperatorSpillBytes"))?;

        if spill_bytes > 0.0 {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Warning,
                node_path: format!(
                    "{} (plan_node_id={})",
                    context.node.operator_name,
                    context.node.plan_node_id.unwrap_or(-1)
                ),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
                    Lang::Zh => format!("Sort 发生磁盘溢写，溢写数据量 {}", format_bytes(spill_bytes as u64)),
                    Lang::En => format!("Sort spilled to disk, spilled data: {}", format_bytes(spill_bytes as u64)),
                },
                reason: context.lang.pick("排序数据量超出内存限制，触发磁盘溢写。Spill 会显著降低排序性能。", "The sort data exceeds the memory limit and spills to disk. Spill significantly degrades sort performance.")
                    .to_string(),
                suggestions: vec![
                    context.lang.pick("增加内存限制以避免 Spill", "Increase the memory limit to avoid Spill").to_string(),
                    context.lang.pick("添加 LIMIT 减少排序数据量", "Add a LIMIT to reduce the data to sort").to_string(),
                    context.lang.pick("检查是否可以优化查询减少排序数据", "Check whether the query can be optimized to sort less data").to_string(),
                ],
                parameter_suggestions: {
                    let mut suggestions = Vec::new();
                    if let Some(s) = context.suggest_parameter_smart("query_mem_limit") {
                        suggestions.push(s);
                    }
                    if let Some(s) = context.suggest_parameter_smart("enable_spill") {
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

/// T003: Sort memory too high
/// Condition: OperatorPeakMemoryUsage > 1GB
pub struct T003SortMemoryHigh;

impl DiagnosticRule for T003SortMemoryHigh {
    fn id(&self) -> &str {
        "T003"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("排序内存过高", "Sort memory too high")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SORT")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let memory = context
            .get_metric("OperatorPeakMemoryUsage")
            .or_else(|| context.get_memory_usage().map(|v| v as f64))?;

        const ONE_GB: f64 = 1024.0 * 1024.0 * 1024.0;

        if memory > ONE_GB {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Warning,
                node_path: format!(
                    "{} (plan_node_id={})",
                    context.node.operator_name,
                    context.node.plan_node_id.unwrap_or(-1)
                ),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
                    Lang::Zh => format!("排序内存使用 {}", format_bytes(memory as u64)),
                    Lang::En => format!("Sort memory usage: {}", format_bytes(memory as u64)),
                },
                reason: context.lang.pick("排序占用内存过高，可能导致内存压力或影响其他算子。", "Sort uses too much memory, which may cause memory pressure or affect other operators.").to_string(),
                suggestions: vec![
                    context.lang.pick("添加 LIMIT 减少排序数据量", "Add a LIMIT to reduce the data to sort").to_string(),
                    context.lang.pick("启用 Spill 功能避免 OOM", "Enable Spill to avoid OOM").to_string(),
                    context.lang.pick("考虑分批处理", "Consider processing in batches").to_string(),
                ],
                parameter_suggestions: {
                    let mut suggestions = Vec::new();
                    if let Some(s) = context.suggest_parameter_smart("enable_spill") {
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

/// W001: Window function memory too high
/// Condition: Memory > 500MB for ANALYTIC operator
pub struct W001WindowMemoryHigh;

impl DiagnosticRule for W001WindowMemoryHigh {
    fn id(&self) -> &str {
        "W001"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("窗口函数内存过高", "Window function memory too high")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        let name = node.operator_name.to_uppercase();
        name.contains("ANALYTIC") || name.contains("WINDOW")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let memory = context.get_memory_usage()?;
        const THRESHOLD: u64 = 500 * 1024 * 1024; // 500MB

        if memory > THRESHOLD {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Warning,
                node_path: format!(
                    "{} (plan_node_id={})",
                    context.node.operator_name,
                    context.node.plan_node_id.unwrap_or(-1)
                ),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
                    Lang::Zh => format!("窗口函数内存使用 {}", format_bytes(memory)),
                    Lang::En => format!("Window function memory usage: {}", format_bytes(memory)),
                },
                reason: context.lang.pick("窗口函数占用内存过高，可能是窗口分区过大或窗口函数状态过大。", "The window function uses too much memory, possibly due to oversized window partitions or large window function states.").to_string(),
                suggestions: vec![
                    context.lang.pick("检查 PARTITION BY 基数是否过高", "Check whether the PARTITION BY cardinality is too high").to_string(),
                    context.lang.pick("考虑减少窗口大小", "Consider reducing the window size").to_string(),
                    context.lang.pick("检查是否可以使用聚合函数替代", "Check whether an aggregate function can be used instead").to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// T004: Sort merging time too long
pub struct T004SortMergingTimeLong;

impl DiagnosticRule for T004SortMergingTimeLong {
    fn id(&self) -> &str {
        "T004"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("Sort 合并时间过长", "Sort merge time too long")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SORT")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let merging_time = context.get_metric("MergingTime")?;
        let op_time = context.get_operator_time_ms()? * 1_000_000.0;
        if op_time == 0.0 {
            return None;
        }
        let ratio = merging_time / op_time;
        if ratio > 0.3 {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Info,
                node_path: format!(
                    "{} (plan_node_id={})",
                    context.node.operator_name,
                    context.node.plan_node_id.unwrap_or(-1)
                ),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
                    Lang::Zh => format!("Sort 合并阶段占比过高 ({:.1}%)", ratio * 100.0),
                    Lang::En => format!("Sort merge phase takes too large a share ({:.1}%)", ratio * 100.0),
                },
                reason: context.lang.pick("多路归并排序时间过长，可能是归并路数过多或单路数据量大。", "Multi-way merge sort takes too long, possibly due to too many merge streams or large per-stream data volume.").to_string(),
                suggestions: vec![
                    context.lang.pick("检查并行度设置是否合理", "Check whether the parallelism settings are appropriate").to_string(),
                    context.lang.pick("考虑减少分区数量", "Consider reducing the number of partitions").to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// T005: Merge waiting for upstream too long
pub struct T005MergeWaitingLong;

impl DiagnosticRule for T005MergeWaitingLong {
    fn id(&self) -> &str {
        "T005"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("Merge 等待上游过长", "Merge waiting on upstream too long")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("MERGE")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let pending_time = context.get_metric("6-PendingStageTime")?;
        let overall_time = context.get_metric("OverallStageTime").unwrap_or(1.0);
        if overall_time == 0.0 {
            return None;
        }
        let ratio = pending_time / overall_time;
        if ratio > 0.3 {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Info,
                node_path: format!(
                    "{} (plan_node_id={})",
                    context.node.operator_name,
                    context.node.plan_node_id.unwrap_or(-1)
                ),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
                    Lang::Zh => format!("Merge 等待上游时间占比 {:.1}%", ratio * 100.0),
                    Lang::En => format!("Merge waits on upstream for {:.1}% of the time", ratio * 100.0),
                },
                reason: context.lang.pick("Merge 算子等待上游数据时间过长，上游算子可能存在性能瓶颈。", "The Merge operator waits too long for upstream data; an upstream operator may be a performance bottleneck.").to_string(),
                suggestions: vec![
                    context.lang.pick("首先优化生产者 operator", "Optimize the producer operator first").to_string(),
                    context.lang.pick("扩大管道缓冲区", "Enlarge the Pipeline buffer").to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// Get all sort rules
pub fn get_rules() -> Vec<Box<dyn DiagnosticRule>> {
    vec![
        Box::new(T001SortRowsTooLarge),
        Box::new(T002SortSpill),
        Box::new(T003SortMemoryHigh),
        Box::new(T004SortMergingTimeLong),
        Box::new(T005MergeWaitingLong),
        Box::new(W001WindowMemoryHigh),
    ]
}
