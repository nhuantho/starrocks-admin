//! Exchange operator diagnostic rules (E001-E003)
//!
//! Rules for EXCHANGE_SINK and EXCHANGE_SOURCE operators.

use super::*;

/// E001: Network transfer too large
/// Condition: BytesSent > 1GB
pub struct E001NetworkTransferLarge;

impl DiagnosticRule for E001NetworkTransferLarge {
    fn id(&self) -> &str {
        "E001"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("网络传输数据量大", "Large network transfer")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("EXCHANGE")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let bytes_sent = context
            .get_metric("BytesSent")
            .or_else(|| context.get_metric("NetworkBytesSent"))?;

        const ONE_GB: f64 = 1024.0 * 1024.0 * 1024.0;

        if bytes_sent > ONE_GB {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Warning,
                node_path: format!("{} (plan_node_id={})", 
                    context.node.operator_name,
                    context.node.plan_node_id.unwrap_or(-1)),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
                    Lang::Zh => format!("网络传输数据量 {}，可能存在网络瓶颈", format_bytes(bytes_sent as u64)),
                    Lang::En => format!("Network transfer volume is {}; there may be a network bottleneck", format_bytes(bytes_sent as u64)),
                },
                reason: context.lang.pick("网络传输数据量过大，占用大量网络带宽和时间。可能是 Shuffle 数据量大或缺少有效的数据裁剪。", "The network transfer volume is too large, consuming significant bandwidth and time. This may be caused by a large Shuffle volume or a lack of effective data pruning.").to_string(),
                suggestions: vec![
                    context.lang.pick("检查是否可以减少 Shuffle 数据量", "Check whether the Shuffle data volume can be reduced").to_string(),
                    context.lang.pick("考虑使用 Colocate Join 避免 Shuffle", "Consider using Colocate Join to avoid Shuffle").to_string(),
                    context.lang.pick("检查网络带宽是否充足", "Check whether network bandwidth is sufficient").to_string(),
                ],
                parameter_suggestions: {
                    let mut suggestions = Vec::new();
                    // Use smart recommendation for parallel_fragment_exec_instance_num
                    if let Some(s) = context.suggest_parameter_smart("parallel_fragment_exec_instance_num") {
                        suggestions.push(s);
                    }
                    // Use smart recommendation for pipeline_dop
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

/// E002: Network time too high
/// Condition: NetworkTime/OperatorTime > 0.5
pub struct E002NetworkTimeHigh;

impl DiagnosticRule for E002NetworkTimeHigh {
    fn id(&self) -> &str {
        "E002"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("网络时间占比高", "High network time ratio")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("EXCHANGE")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let network_time = context
            .get_metric("NetworkTime")
            .or_else(|| context.get_metric("OverallThroughputSendTime"))?;
        let operator_time = context.get_operator_time_ms()?;

        if operator_time == 0.0 {
            return None;
        }

        let ratio = network_time / operator_time;

        if ratio > 0.5 {
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
                    Lang::Zh => format!("网络时间占比 {:.1}%，可能存在网络瓶颈", ratio * 100.0),
                    Lang::En => format!("Network time accounts for {:.1}%; there may be a network bottleneck", ratio * 100.0),
                },
                reason: context.lang.pick("网络传输时间占比过高，查询瓶颈在网络。可能是网络带宽不足或跨机房传输。", "Network transfer takes too large a share of time; the query is bottlenecked on the network. This may be due to insufficient bandwidth or cross-datacenter transfer.").to_string(),
                suggestions: vec![
                    context.lang.pick("检查网络带宽和延迟", "Check network bandwidth and latency").to_string(),
                    context.lang.pick("考虑使用 Colocate Join", "Consider using Colocate Join").to_string(),
                    context.lang.pick("减少跨节点数据传输", "Reduce cross-node data transfer").to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// E003: Data skew in shuffle
/// Condition: max(BytesSent)/avg > 2
pub struct E003ShuffleSkew;

impl DiagnosticRule for E003ShuffleSkew {
    fn id(&self) -> &str {
        "E003"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("Shuffle 数据倾斜", "Shuffle data skew")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("EXCHANGE")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let max_bytes = context.get_metric("__MAX_OF_BytesSent")?;
        let min_bytes = context.get_metric("__MIN_OF_BytesSent").unwrap_or(0.0);

        if min_bytes == 0.0 {
            return None;
        }

        let avg_bytes = (max_bytes + min_bytes) / 2.0;
        let ratio = max_bytes / avg_bytes;

        if ratio > 2.0 {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Warning,
                node_path: format!("{} (plan_node_id={})", 
                    context.node.operator_name,
                    context.node.plan_node_id.unwrap_or(-1)),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
                    Lang::Zh => format!("Shuffle 存在数据倾斜，max/avg 比率为 {:.2}", ratio),
                    Lang::En => format!("Shuffle data is skewed; max/avg ratio is {:.2}", ratio),
                },
                reason: context.lang.pick("Shuffle 数据在各节点间分布不均匀，部分节点接收更多数据。通常是 Shuffle 键存在热点值。", "Shuffle data is unevenly distributed across nodes, so some nodes receive more data. This is usually caused by hot values in the Shuffle key.").to_string(),
                suggestions: vec![
                    context.lang.pick("检查分区键选择是否合理", "Check whether the partition key is chosen appropriately").to_string(),
                    context.lang.pick("考虑使用 Skew Join 优化", "Consider using Skew Join optimization").to_string(),
                    context.lang.pick("检查是否存在热点数据", "Check for hot data").to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// Get all exchange rules
pub fn get_rules() -> Vec<Box<dyn DiagnosticRule>> {
    vec![
        Box::new(E001NetworkTransferLarge),
        Box::new(E002NetworkTimeHigh),
        Box::new(E003ShuffleSkew),
    ]
}
