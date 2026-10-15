//! OlapTableSink operator rules (I001-I003) for data import scenarios

use super::*;

/// I001: Import data skew
pub struct I001ImportDataSkew;

impl DiagnosticRule for I001ImportDataSkew {
    fn id(&self) -> &str {
        "I001"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("导入数据倾斜", "Import data skew")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        let name = node.operator_name.to_uppercase();
        name.contains("SINK") && (name.contains("OLAP") || name.contains("TABLE"))
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let max_chunks = context.get_metric("__MAX_OF_PushChunkNum")?;
        let min_chunks = context.get_metric("__MIN_OF_PushChunkNum").unwrap_or(0.0);
        if min_chunks == 0.0 {
            return None;
        }
        let ratio = max_chunks / min_chunks;
        if ratio > 3.0 {
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
                    Lang::Zh => {
                        format!("导入存在数据倾斜，PushChunkNum max/min 比率为 {:.2}", ratio)
                    },
                    Lang::En => {
                        format!("Import data is skewed; PushChunkNum max/min ratio is {:.2}", ratio)
                    },
                },
                reason: context
                    .lang
                    .pick(
                        "请参考 StarRocks 官方文档了解更多信息。",
                        "See the StarRocks documentation for more information.",
                    )
                    .to_string(),
                suggestions: vec![
                    context
                        .lang
                        .pick(
                            "检查上游算子是否存在数据倾斜",
                            "Check upstream operators for data skew",
                        )
                        .to_string(),
                    context
                        .lang
                        .pick("优化分桶键选择", "Optimize the Bucket key selection")
                        .to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// I002: Import RPC latency high
pub struct I002ImportRPCLatency;

impl DiagnosticRule for I002ImportRPCLatency {
    fn id(&self) -> &str {
        "I002"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("导入 RPC 延迟高", "High import RPC latency")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        let name = node.operator_name.to_uppercase();
        name.contains("SINK") && (name.contains("OLAP") || name.contains("TABLE"))
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let client_time = context.get_metric("RpcClientSideTime")?;
        let server_time = context.get_metric("RpcServerSideTime").unwrap_or(1.0);
        if server_time == 0.0 {
            return None;
        }
        let ratio = client_time / server_time;
        if ratio > 2.0 && client_time > 1_000_000_000.0 {
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
                    Lang::Zh => {
                        format!("导入 RPC 客户端耗时是服务端的 {:.1} 倍，网络传输可能是瓶颈", ratio)
                    },
                    Lang::En => format!(
                        "Import RPC client-side time is {:.1}x the server-side time; network transfer may be the bottleneck",
                        ratio
                    ),
                },
                reason: context
                    .lang
                    .pick(
                        "请参考 StarRocks 官方文档了解更多信息。",
                        "See the StarRocks documentation for more information.",
                    )
                    .to_string(),
                suggestions: vec![
                    context
                        .lang
                        .pick(
                            "启用数据压缩减少网络传输量",
                            "Enable data compression to reduce network transfer volume",
                        )
                        .to_string(),
                    context
                        .lang
                        .pick("检查网络带宽和延迟", "Check network bandwidth and latency")
                        .to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// I003: Import filtered rows too many
pub struct I003ImportFilteredRows;

impl DiagnosticRule for I003ImportFilteredRows {
    fn id(&self) -> &str {
        "I003"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("导入过滤行数过多", "Too many rows filtered during import")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        let name = node.operator_name.to_uppercase();
        name.contains("SINK") && (name.contains("OLAP") || name.contains("TABLE"))
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let filtered = context.get_metric("RowsFiltered").unwrap_or(0.0);
        let read = context
            .get_metric("RowsRead")
            .or_else(|| context.node.metrics.push_row_num.map(|v| v as f64))?;
        if read == 0.0 {
            return None;
        }
        let ratio = filtered / read;
        if ratio > 0.1 && filtered > 1000.0 {
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
                    Lang::Zh => format!(
                        "导入过滤了 {:.0} 行 ({:.1}%)，可能存在数据质量问题",
                        filtered,
                        ratio * 100.0
                    ),
                    Lang::En => format!(
                        "Import filtered {:.0} rows ({:.1}%); there may be data quality issues",
                        filtered,
                        ratio * 100.0
                    ),
                },
                reason: context
                    .lang
                    .pick(
                        "请参考 StarRocks 官方文档了解更多信息。",
                        "See the StarRocks documentation for more information.",
                    )
                    .to_string(),
                suggestions: vec![
                    context
                        .lang
                        .pick(
                            "检查数据格式是否符合表结构",
                            "Check that the data format matches the table schema",
                        )
                        .to_string(),
                    context
                        .lang
                        .pick(
                            "检查是否有空值或类型不匹配",
                            "Check for null values or type mismatches",
                        )
                        .to_string(),
                    context
                        .lang
                        .pick(
                            "查看 BE 日志获取详细过滤原因",
                            "Check BE logs for detailed filtering reasons",
                        )
                        .to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

pub fn get_rules() -> Vec<Box<dyn DiagnosticRule>> {
    vec![
        Box::new(I001ImportDataSkew),
        Box::new(I002ImportRPCLatency),
        Box::new(I003ImportFilteredRows),
    ]
}
