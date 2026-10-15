//! Project and LocalExchange operator rules (P001, L001)

use super::*;

/// P001: Project expression compute time high
pub struct P001ProjectExprHigh;

impl DiagnosticRule for P001ProjectExprHigh {
    fn id(&self) -> &str {
        "P001"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("Project 表达式计算耗时高", "High Project expression compute time")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("PROJECT")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let expr_time = context.get_metric("ExprComputeTime")?;
        let op_time = context.get_operator_time_ms()? * 1_000_000.0; // Convert to ns
        if op_time == 0.0 {
            return None;
        }
        let ratio = expr_time / op_time;
        if ratio > 0.5 && expr_time > 100_000_000.0 {
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
                    Lang::Zh => format!("Project 表达式计算占比过高 ({:.1}%)", ratio * 100.0),
                    Lang::En => format!("Project expression compute time ratio too high ({:.1}%)", ratio * 100.0),
                },
                reason: context.lang.pick("Project 算子执行时间过长，可能是表达式计算复杂或数据量大。", "The Project operator takes too long, likely due to complex expressions or a large data volume.").to_string(),
                suggestions: vec![
                    context.lang.pick("简化 SELECT 中的复杂表达式", "Simplify complex expressions in SELECT").to_string(),
                    context.lang.pick("将复杂计算移到物化视图中预计算", "Precompute complex calculations in a materialized view").to_string(),
                    context.lang.pick("检查是否有不必要的类型转换", "Check for unnecessary type conversions").to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// L001: LocalExchange memory too high
pub struct L001LocalExchangeMemory;

impl DiagnosticRule for L001LocalExchangeMemory {
    fn id(&self) -> &str {
        "L001"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("LocalExchange 内存使用过高", "LocalExchange memory usage too high")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("LOCAL")
            && node.operator_name.to_uppercase().contains("EXCHANGE")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let memory = context
            .get_metric("LocalExchangePeakMemoryUsage")
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
                    Lang::Zh => format!("LocalExchange 内存使用 {}", format_bytes(memory as u64)),
                    Lang::En => format!("LocalExchange memory usage {}", format_bytes(memory as u64)),
                },
                reason: context.lang.pick("请参考 StarRocks 官方文档了解更多信息。", "See the StarRocks documentation for more information.").to_string(),
                suggestions: vec![
                    context.lang.pick("检查上下游算子的数据流是否平衡", "Check whether data flow between upstream and downstream operators is balanced").to_string(),
                    context.lang.pick("调整 pipeline_dop 参数", "Tune the pipeline_dop parameter").to_string(),
                ],
                parameter_suggestions: {
                    let mut suggestions = Vec::new();
                    if let Some(s) = context.suggest_parameter_smart("pipeline_dop") {
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

pub fn get_rules() -> Vec<Box<dyn DiagnosticRule>> {
    vec![Box::new(P001ProjectExprHigh), Box::new(L001LocalExchangeMemory)]
}
