//! Fragment level diagnostic rules (F001-F003)

use super::*;

/// F001: Execution time skew across instances
pub struct F001ExecutionTimeSkew;

impl DiagnosticRule for F001ExecutionTimeSkew {
    fn id(&self) -> &str {
        "F001"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("实例执行时间倾斜", "Instance execution time skew")
    }

    fn applicable_to(&self, _node: &ExecutionTreeNode) -> bool {
        true
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let max_time = context.node.metrics.operator_total_time_max?;
        let avg_time = context.node.metrics.operator_total_time?;
        if avg_time == 0 {
            return None;
        }
        let ratio = max_time as f64 / avg_time as f64;
        if ratio > 2.0 {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Warning,
                node_path: format!("{} (plan_node_id={})", context.node.operator_name, context.node.plan_node_id.unwrap_or(-1)),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
                    Lang::Zh => format!("实例执行时间存在倾斜，max/avg 比率为 {:.2}", ratio),
                    Lang::En => format!("Instance execution time is skewed; max/avg ratio is {:.2}", ratio),
                },
                reason: context.lang.pick("Fragment 执行时间过长，是查询的主要瓶颈。需要分析 Fragment 内的算子找出具体问题。", "The Fragment takes too long to execute and is the main bottleneck of the query. Analyze the operators within the Fragment to find the specific issue.").to_string(),
                suggestions: vec![context.lang.pick("检查数据分布", "Check data distribution").to_string(), context.lang.pick("优化分桶策略", "Optimize the bucketing strategy").to_string()],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// F002: Memory allocation skew
pub struct F002MemorySkew;

impl DiagnosticRule for F002MemorySkew {
    fn id(&self) -> &str {
        "F002"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("实例内存分配不均", "Uneven instance memory allocation")
    }

    fn applicable_to(&self, _node: &ExecutionTreeNode) -> bool {
        true
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let max_mem = context.get_metric("__MAX_OF_OperatorPeakMemoryUsage")?;
        let min_mem = context
            .get_metric("__MIN_OF_OperatorPeakMemoryUsage")
            .unwrap_or(0.0);
        if min_mem == 0.0 {
            return None;
        }
        let ratio = max_mem / ((max_mem + min_mem) / 2.0);
        if ratio > 2.0 {
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
                    Lang::Zh => format!("实例内存分配不均，max/avg 比率为 {:.2}", ratio),
                    Lang::En => format!(
                        "Instance memory allocation is uneven; max/avg ratio is {:.2}",
                        ratio
                    ),
                },
                reason: context
                    .lang
                    .pick(
                        "Fragment 内存使用过高，可能导致查询失败。",
                        "Fragment memory usage is too high and may cause the query to fail.",
                    )
                    .to_string(),
                suggestions: vec![
                    context
                        .lang
                        .pick("检查数据倾斜", "Check for data skew")
                        .to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// F003: Fragment prepare time too long
pub struct F003PrepareTimeLong;

impl DiagnosticRule for F003PrepareTimeLong {
    fn id(&self) -> &str {
        "F003"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("Fragment 准备时间过长", "Long Fragment prepare time")
    }

    fn applicable_to(&self, _node: &ExecutionTreeNode) -> bool {
        true
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let prepare_time = context.get_metric("FragmentInstancePrepareTime")?;
        if prepare_time > 1_000_000_000.0 {
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
                    Lang::Zh => format!("Fragment 准备时间 {:.1}s", prepare_time / 1_000_000_000.0),
                    Lang::En => format!("Fragment prepare time is {:.1}s", prepare_time / 1_000_000_000.0),
                },
                reason: context.lang.pick("Fragment 各实例执行时间差异大，存在数据倾斜或资源不均。", "Execution times vary widely across Fragment instances, indicating data skew or uneven resources.").to_string(),
                suggestions: vec![context.lang.pick("检查元数据加载", "Check metadata loading").to_string()],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

pub fn get_rules() -> Vec<Box<dyn DiagnosticRule>> {
    vec![Box::new(F001ExecutionTimeSkew), Box::new(F002MemorySkew), Box::new(F003PrepareTimeLong)]
}
