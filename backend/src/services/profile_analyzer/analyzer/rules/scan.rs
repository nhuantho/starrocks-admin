//! Scan operator diagnostic rules (S001-S014)
//!
//! Rules for OLAP_SCAN and CONNECTOR_SCAN operators.
//!
//! ## Rule List:
//! - S001: Data skew detection
//! - S002: IO skew detection
//! - S003: Poor filter effectiveness
//! - S004: Predicate not pushed down
//! - S005: IO thread pool saturation
//! - S006: Rowset fragmentation
//! - S007: Cold storage access (IO bound)
//! - S008: ZoneMap index not effective
//! - S009: Low cache hit rate (PageCache + DataCache for disaggregated storage)
//! - S010: Runtime Filter not effective
//! - S011: Accumulated soft deletes
//! - S012: Bitmap index not effective
//! - S013: Bloom filter index not effective
//! - S014: Colocate Join opportunity missed

use super::*;

/// S001: Data skew detection
/// Condition: max(RowsRead)/avg(RowsRead) > 2
pub struct S001DataSkew;

impl DiagnosticRule for S001DataSkew {
    fn id(&self) -> &str {
        "S001"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("Scan 数据倾斜", "Scan Data Skew")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SCAN")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        // Check for max/min rows read metrics
        let max_rows = context
            .get_metric("__MAX_OF_RowsRead")
            .or_else(|| context.get_metric("RowsRead"))?;
        let min_rows = context.get_metric("__MIN_OF_RowsRead").unwrap_or(0.0);

        if min_rows == 0.0 {
            return None;
        }

        let avg_rows = (max_rows + min_rows) / 2.0;
        let ratio = max_rows / avg_rows;

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
 Lang::Zh => format!("Scan 存在数据倾斜，max/avg 比率为 {:.2}", ratio),
 Lang::En => format!("Scan data skew detected, max/avg ratio is {:.2}", ratio),
 },
                reason: context.lang.pick("StarRocks 数据在各个存储节点分布不均，使得某些节点在读取数据时需要扫描更多的数据，导致查询延迟。通常是分桶键选择不当导致数据分布不均匀。", "Data is unevenly distributed across storage nodes, so some nodes must scan much more data than others, increasing query latency. This is usually caused by a poorly chosen bucketing key.").to_string(),
                suggestions: vec![
                    context.lang.pick("检查分桶键选择是否合理", "Check whether the bucketing key is appropriate").to_string(),
                    context.lang.pick("考虑重新分桶以均匀分布数据", "Consider re-bucketing to distribute data evenly").to_string(),
                    context.lang.pick("检查是否存在热点数据", "Check for hot-spot data").to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// S003: Poor filter effectiveness
/// Condition: RowsRead/RawRowsRead > 0.8 (less than 20% filtered)
pub struct S003PoorFilter;

impl DiagnosticRule for S003PoorFilter {
    fn id(&self) -> &str {
        "S003"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("过滤效果差", "Poor Filter Effectiveness")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SCAN")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let rows_read = context.get_metric("RowsRead")?;
        let raw_rows_read = context.get_metric("RawRowsRead")?;

        if raw_rows_read == 0.0 {
            return None;
        }

        let ratio = rows_read / raw_rows_read;

        // Only trigger if we're reading a significant amount of data
        if ratio > 0.8 && raw_rows_read > 100_000.0 {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Warning,
                node_path: format!("{} (plan_node_id={})", 
                    context.node.operator_name,
                    context.node.plan_node_id.unwrap_or(-1)),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
 Lang::Zh => format!("过滤效果差，仅过滤了 {:.1}% 的数据 (读取 {:.0} 行 / 原始 {:.0} 行)", (1.0 - ratio) * 100.0, rows_read, raw_rows_read),
 Lang::En => format!("Poor filtering: only {:.1}% of data filtered ({:.0} rows read / {:.0} raw rows)", (1.0 - ratio) * 100.0, rows_read, raw_rows_read),
 },
                reason: context.lang.pick("基于扫描原始数据量以及最终输出数据量判断，Scan 算子扫描数据量较大但输出给下游的数据量未显著减少。StarRocks 提供索引、谓词下推、Runtime Filter 等多种方式过滤数据。", "Comparing raw rows scanned with rows output, the Scan operator reads a large amount of data but does not significantly reduce what it passes downstream. StarRocks can filter data using indexes, predicate push-down, Runtime Filter, and more.").to_string(),
                suggestions: vec![
                    context.lang.pick("添加更精确的过滤条件", "Add more selective filter conditions").to_string(),
                    context.lang.pick("检查谓词是否可以下推到存储层", "Check whether predicates can be pushed down to the storage layer").to_string(),
                    context.lang.pick("考虑使用分区裁剪", "Consider using partition pruning").to_string(),
                    context.lang.pick("检查 ZoneMap 索引是否生效", "Check whether the ZoneMap index is effective").to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// S007: Cold storage access (IO bound)
/// Condition: IOTime/ScanTime > 0.8 && BytesRead > 1GB
pub struct S007ColdStorage;

impl DiagnosticRule for S007ColdStorage {
    fn id(&self) -> &str {
        "S007"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("冷存储访问", "Cold Storage Access")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SCAN")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let io_time = context
            .get_metric("IOTime")
            .or_else(|| context.get_metric("IOTaskExecTime"))?;
        let scan_time = context
            .get_metric("ScanTime")
            .or_else(|| context.get_operator_time_ms())?;

        if scan_time == 0.0 {
            return None;
        }

        let bytes_read = context.get_metric("BytesRead").unwrap_or(0.0);
        let ratio = io_time / scan_time;

        const ONE_GB: f64 = 1024.0 * 1024.0 * 1024.0;

        if ratio > 0.8 && bytes_read > ONE_GB {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Warning,
                node_path: format!("{} (plan_node_id={})", 
                    context.node.operator_name,
                    context.node.plan_node_id.unwrap_or(-1)),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
 Lang::Zh => format!("IO 时间占比 {:.1}%，读取数据量 {}，可能存在存储瓶颈", ratio * 100.0, format_bytes(bytes_read as u64)),
 Lang::En => format!("IO time accounts for {:.1}% with {} read; storage may be a bottleneck", ratio * 100.0, format_bytes(bytes_read as u64)),
 },
                reason: context.lang.pick("数据存储在冷存储（如对象存储）上，IO 延迟较高。冷存储的 IOPS 和吞吐量通常低于本地 SSD。", "Data resides on cold storage (such as object storage) with high IO latency. Cold storage usually has lower IOPS and throughput than local SSDs.").to_string(),
                suggestions: vec![
                    context.lang.pick("检查存储性能，考虑使用 SSD", "Check storage performance and consider using SSDs").to_string(),
                    context.lang.pick("增大 PageCache 缓存", "Increase the PageCache size").to_string(),
                    context.lang.pick("检查网络带宽（如果是远程存储）", "Check network bandwidth (for remote storage)").to_string(),
                ],
                parameter_suggestions: {
                    let mut suggestions = Vec::new();
                    if let Some(s) = context.suggest_parameter_smart("storage_page_cache_limit") {
                        suggestions.push(s);
                    }
                    if let Some(s) = context.suggest_parameter_smart("io_tasks_per_scan_operator") {
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

/// S009: Low cache hit rate (PageCache or DataCache)
/// Condition:
/// - PageCache: CachedPagesNum/ReadPagesNum < 0.3
/// - DataCache (disaggregated): CompressedBytesReadLocalDisk/(Local+Remote) < 0.7
pub struct S009LowCacheHit;

impl DiagnosticRule for S009LowCacheHit {
    fn id(&self) -> &str {
        "S009"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("缓存命中率低", "Low Cache Hit Rate")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SCAN")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        // First, check DataCache metrics (for disaggregated storage-compute clusters)
        let bytes_local = context
            .get_metric("CompressedBytesReadLocalDisk")
            .unwrap_or(0.0);
        let bytes_remote = context
            .get_metric("CompressedBytesReadRemote")
            .unwrap_or(0.0);
        let total_bytes = bytes_local + bytes_remote;

        const MIN_BYTES: f64 = 10.0 * 1024.0 * 1024.0; // 10MB

        // DataCache scenario: remote bytes > 0 indicates disaggregated storage
        if bytes_remote > 0.0 && total_bytes > MIN_BYTES {
            let hit_rate = bytes_local / total_bytes;

            // Trigger if hit rate < 70%
            if hit_rate < 0.7 {
                let miss_rate = (1.0 - hit_rate) * 100.0;
                return Some(Diagnostic {
                    rule_id: self.id().to_string(),
                    rule_name: self.name(context.lang).to_string(),
                    severity: if hit_rate < 0.3 { RuleSeverity::Error } else { RuleSeverity::Warning },
                    node_path: format!("{} (plan_node_id={})", 
                        context.node.operator_name,
                        context.node.plan_node_id.unwrap_or(-1)),
                    plan_node_id: context.node.plan_node_id,
                    message: match context.lang {
 Lang::Zh => format!("DataCache 命中率 {:.1}%，{:.1}% 数据从远程存储读取 (本地: {}, 远程: {})", hit_rate * 100.0,
 miss_rate,
 format_bytes(bytes_local as u64),
 format_bytes(bytes_remote as u64)),
 Lang::En => format!("DataCache hit rate is {:.1}%; {:.1}% of data read from remote storage (local: {}, remote: {})", hit_rate * 100.0,
 miss_rate,
 format_bytes(bytes_local as u64),
 format_bytes(bytes_remote as u64)),
 },
                    reason: context.lang.pick("存算分离架构下，DataCache 是提升查询性能的关键。当大量数据需要从远程存储（如 S3/OSS）读取时，网络延迟会显著影响查询性能。", "In shared-data deployments, DataCache is key to query performance. When large amounts of data must be read from remote storage (such as S3/OSS), network latency significantly degrades query performance.").to_string(),
                    suggestions: vec![
                        context.lang.pick("增大 DataCache 容量 (datacache_disk_size)", "Increase DataCache capacity (datacache_disk_size)").to_string(),
                        context.lang.pick("检查 DataCache 磁盘空间是否充足", "Check whether DataCache disk space is sufficient").to_string(),
                        context.lang.pick("对热点数据执行缓存预热 (CACHE SELECT)", "Warm up the cache for hot data (CACHE SELECT)").to_string(),
                        context.lang.pick("检查是否有其他查询竞争缓存资源", "Check whether other queries are competing for cache resources").to_string(),
                    ],
                    // Only suggest parameters that are not already set to recommended values
                    parameter_suggestions: [
                        context.suggest_parameter(
                            "enable_scan_datacache",
                            "true",
                            "SET enable_scan_datacache = true;"
                        ),
                        context.suggest_parameter(
                            "enable_populate_datacache",
                            "true",
                            "SET enable_populate_datacache = true;"
                        ),
                    ].into_iter().flatten().collect(),
                });
            }
        }

        // Also check IO count metrics as alternative for DataCache
        let io_local = context.get_metric("IOCountLocalDisk").unwrap_or(0.0);
        let io_remote = context.get_metric("IOCountRemote").unwrap_or(0.0);
        let total_io = io_local + io_remote;

        if io_remote > 0.0 && total_io > 100.0 {
            let hit_rate = io_local / total_io;

            if hit_rate < 0.7 {
                return Some(Diagnostic {
                    rule_id: self.id().to_string(),
                    rule_name: self.name(context.lang).to_string(),
                    severity: if hit_rate < 0.3 { RuleSeverity::Error } else { RuleSeverity::Warning },
                    node_path: format!("{} (plan_node_id={})", 
                        context.node.operator_name,
                        context.node.plan_node_id.unwrap_or(-1)),
                    plan_node_id: context.node.plan_node_id,
                    message: match context.lang {
 Lang::Zh => format!("DataCache IO 命中率 {:.1}%，{:.1}% IO 访问远程存储 (本地: {:.0}, 远程: {:.0})", hit_rate * 100.0,
 (1.0 - hit_rate) * 100.0,
 io_local,
 io_remote),
 Lang::En => format!("DataCache IO hit rate is {:.1}%; {:.1}% of IO accessed remote storage (local: {:.0}, remote: {:.0})", hit_rate * 100.0,
 (1.0 - hit_rate) * 100.0,
 io_local,
 io_remote),
 },
                    reason: context.lang.pick("存算分离架构下，DataCache 是提升查询性能的关键。当大量 IO 请求需要访问远程存储时，网络延迟会显著影响查询性能。", "In shared-data deployments, DataCache is key to query performance. When many IO requests must access remote storage, network latency significantly degrades query performance.").to_string(),
                    suggestions: vec![
                        context.lang.pick("增大 DataCache 容量 (datacache_disk_size)", "Increase DataCache capacity (datacache_disk_size)").to_string(),
                        context.lang.pick("对热点数据执行缓存预热 (CACHE SELECT)", "Warm up the cache for hot data (CACHE SELECT)").to_string(),
                    ],
                    // Only suggest if not already enabled
                    parameter_suggestions: context.suggest_parameter(
                        "enable_scan_datacache",
                        "true",
                        "SET enable_scan_datacache = true;"
                    ).into_iter().collect(),
                });
            }
        }

        // Fallback: check PageCache metrics (for shared-nothing clusters)
        let cached_pages = context.get_metric("CachedPagesNum");
        let read_pages = context.get_metric("ReadPagesNum");

        if let (Some(cached), Some(total)) = (cached_pages, read_pages)
            && total > 1000.0
        {
            let hit_rate = cached / total;

            if hit_rate < 0.3 {
                return Some(Diagnostic {
                        rule_id: self.id().to_string(),
                        rule_name: self.name(context.lang).to_string(),
                        severity: RuleSeverity::Info,
                        node_path: format!("{} (plan_node_id={})", 
                            context.node.operator_name,
                            context.node.plan_node_id.unwrap_or(-1)),
                        plan_node_id: context.node.plan_node_id,
                        message: match context.lang {
 Lang::Zh => format!("PageCache 命中率仅 {:.1}% ({:.0}/{:.0} pages)", hit_rate * 100.0, cached, total),
 Lang::En => format!("PageCache hit rate is only {:.1}% ({:.0}/{:.0} pages)", hit_rate * 100.0, cached, total),
 },
                        reason: context.lang.pick("PageCache 命中率低，大量数据需要从磁盘读取。可能是缓存容量不足或数据访问模式不适合缓存。", "The PageCache hit rate is low, so much of the data must be read from disk. The cache may be too small, or the access pattern may not suit caching.").to_string(),
                        suggestions: vec![
                            context.lang.pick("增大 PageCache 容量 (storage_page_cache_limit)", "Increase PageCache capacity (storage_page_cache_limit)").to_string(),
                            context.lang.pick("检查是否有其他查询竞争缓存", "Check whether other queries are competing for the cache").to_string(),
                        ],
                    parameter_suggestions: vec![],
                });
            }
        }

        None
    }
}

/// S010: Runtime Filter not effective on Scan
/// Condition: RuntimeFilterRows == 0 && RawRowsRead > 100k
pub struct S010RFNotEffective;

impl DiagnosticRule for S010RFNotEffective {
    fn id(&self) -> &str {
        "S010"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("Scan Runtime Filter 未生效", "Scan Runtime Filter Not Effective")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SCAN")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let rf_rows = context.get_metric("RuntimeFilterRows").unwrap_or(0.0);
        let raw_rows = context.get_metric("RawRowsRead").unwrap_or(0.0);

        if rf_rows == 0.0 && raw_rows > 100_000.0 {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Info,
                node_path: format!("{} (plan_node_id={})", 
                    context.node.operator_name,
                    context.node.plan_node_id.unwrap_or(-1)),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
 Lang::Zh => format!("Runtime Filter 未过滤任何行，扫描了 {:.0} 行", raw_rows),
 Lang::En => format!("Runtime Filter did not filter any rows; {:.0} rows scanned", raw_rows),
 },
                reason: context.lang.pick("Runtime Filter 未能有效过滤数据。可能是 Filter 构建失败、超时或 Filter 选择性差。", "The Runtime Filter did not filter data effectively. The filter may have failed to build, timed out, or has poor selectivity.").to_string(),
                suggestions: vec![
                    context.lang.pick("检查 Runtime Filter 是否被生成", "Check whether the Runtime Filter was generated").to_string(),
                    context.lang.pick("检查 Join 条件是否适合生成 RF", "Check whether the Join condition is suitable for generating a Runtime Filter").to_string(),
                    context.lang.pick("确认 enable_global_runtime_filter 已启用", "Make sure enable_global_runtime_filter is enabled").to_string(),
                ],
                parameter_suggestions: {
                    let mut suggestions = Vec::new();
                    if let Some(s) = context.suggest_parameter_smart("enable_global_runtime_filter") {
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

/// S011: Accumulated soft deletes
/// Condition: DelVecFilterRows/RawRowsRead > 0.3
pub struct S011SoftDeletes;

impl DiagnosticRule for S011SoftDeletes {
    fn id(&self) -> &str {
        "S011"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("累积软删除过多", "Excessive Soft Deletes")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        // Only applicable to OLAP_SCAN (internal tables), not CONNECTOR_SCAN (external tables)
        node.operator_name.to_uppercase().contains("OLAP_SCAN")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let del_vec_rows = context.get_metric("DelVecFilterRows")?;
        let raw_rows = context.get_metric("RawRowsRead")?;

        if raw_rows == 0.0 {
            return None;
        }

        let ratio = del_vec_rows / raw_rows;

        if ratio > 0.3 {
            // Extract table name from unique_metrics
            let table_name = context
                .node
                .unique_metrics
                .get("Table")
                .map(|s| s.as_str())
                .unwrap_or("unknown_table");

            // Build full table name with database
            // If table already has db prefix (db.table), use it directly
            // Otherwise, use default_db from profile summary
            let full_table_name = if table_name.contains('.') {
                table_name.to_string()
            } else if let Some(db) = context.default_db {
                if db.is_empty() {
                    table_name.to_string()
                } else {
                    format!("{}.{}", db, table_name)
                }
            } else {
                table_name.to_string()
            };

            // Generate Compaction command (correct syntax: ALTER TABLE <table> COMPACT)
            let compaction_cmd = format!("ALTER TABLE {} COMPACT;", full_table_name);

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
                        "表 {} 软删除行占比 {:.1}%，建议执行 Compaction",
                        full_table_name,
                        ratio * 100.0
                    ),
                    Lang::En => format!(
                        "Table {} has {:.1}% soft-deleted rows; Compaction is recommended",
                        full_table_name,
                        ratio * 100.0
                    ),
                },
                reason: match context.lang {
                    Lang::Zh => format!(
                        "表 {} 中存在大量软删除记录 ({:.0} 行)，扫描时需要过滤这些已删除的行，影响查询性能。建议执行 Compaction 清理删除标记。",
                        full_table_name, del_vec_rows
                    ),
                    Lang::En => format!(
                        "Table {} has many soft-deleted records ({:.0} rows) that must be filtered out during Scan, hurting query performance. Run Compaction to clean up the delete markers.",
                        full_table_name, del_vec_rows
                    ),
                },
                suggestions: vec![
                    match context.lang {
                        Lang::Zh => format!("执行 Compaction: {}", compaction_cmd),
                        Lang::En => format!("Run Compaction: {}", compaction_cmd),
                    },
                    context
                        .lang
                        .pick(
                            "检查 Compaction 状态: SHOW PROC '/compactions';",
                            "Check Compaction status: SHOW PROC '/compactions';",
                        )
                        .to_string(),
                    match context.lang {
                        Lang::Zh => {
                            format!("查看表 Tablet 状态: SHOW TABLET FROM {};", full_table_name)
                        },
                        Lang::En => format!(
                            "Check table Tablet status: SHOW TABLET FROM {};",
                            full_table_name
                        ),
                    },
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// S002: IO skew detection
pub struct S002IOSkew;

impl DiagnosticRule for S002IOSkew {
    fn id(&self) -> &str {
        "S002"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("Scan IO 倾斜", "Scan IO Skew")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SCAN")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let max_io = context.get_metric("__MAX_OF_IOTime")?;
        let min_io = context.get_metric("__MIN_OF_IOTime").unwrap_or(0.0);
        if min_io == 0.0 {
            return None;
        }
        let ratio = max_io / ((max_io + min_io) / 2.0);
        if ratio > 2.0 {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Warning,
                node_path: format!("{} (plan_node_id={})", context.node.operator_name, context.node.plan_node_id.unwrap_or(-1)),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
 Lang::Zh => format!("Scan IO 耗时存在倾斜，max/avg 比率为 {:.2}", ratio),
 Lang::En => format!("Scan IO time is skewed, max/avg ratio is {:.2}", ratio),
 },
                reason: context.lang.pick("Scan 算子多个实例在读取数据时，部分实例花费的时间显著大于其它实例。可能是节点 IO 使用率不均或数据在节点上分布不均。", "Some instances of the Scan operator take significantly longer to read data than others. Node IO utilization or data distribution across nodes may be uneven.").to_string(),
                suggestions: vec![context.lang.pick("检查节点 IO 使用率是否不均", "Check whether IO utilization is uneven across nodes").to_string(), context.lang.pick("检查存储设备是否存在性能问题", "Check storage devices for performance problems").to_string()],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// S004: Predicate not pushed down
pub struct S004PredicateNotPushed;

impl DiagnosticRule for S004PredicateNotPushed {
    fn id(&self) -> &str {
        "S004"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("谓词未下推", "Predicate Not Pushed Down")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SCAN")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let pushdown = context.get_metric("PushdownPredicates").unwrap_or(0.0);
        let pred_filter = context.get_metric("PredFilterRows").unwrap_or(0.0);
        let raw_rows = context.get_metric("RawRowsRead").unwrap_or(0.0);
        if pushdown == 0.0 && raw_rows > 10000.0 && pred_filter / raw_rows > 0.1 {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Warning,
                node_path: format!("{} (plan_node_id={})", context.node.operator_name, context.node.plan_node_id.unwrap_or(-1)),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
 Lang::Zh => format!("谓词未能下推到存储层，{:.0} 行 ({:.1}%) 在表达式层过滤", pred_filter, pred_filter / raw_rows * 100.0),
 Lang::En => format!("Predicates were not pushed down to storage; {:.0} rows ({:.1}%) filtered at the expression layer", pred_filter, pred_filter / raw_rows * 100.0),
 },
                reason: context.lang.pick("查询条件未能下推到存储层执行，导致需要在计算层过滤大量数据。可能是查询条件包含函数、类型不匹配或不支持下推的表达式。", "Query predicates were not pushed down to the storage layer, so a large amount of data must be filtered in the compute layer. The predicates may contain functions, type mismatches, or expressions that do not support push-down.").to_string(),
                suggestions: vec![context.lang.pick("将谓词重写为简单比较", "Rewrite predicates as simple comparisons").to_string(), context.lang.pick("添加 zonemap/Bloom 索引", "Add ZoneMap/Bloom filter indexes").to_string()],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// S005: IO thread pool saturation
pub struct S005IOThreadPoolSaturation;

impl DiagnosticRule for S005IOThreadPoolSaturation {
    fn id(&self) -> &str {
        "S005"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("IO 线程池饱和", "IO Thread Pool Saturated")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SCAN")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let wait_time = context.get_metric("IOTaskWaitTime").unwrap_or(0.0);
        let peak_tasks = context.get_metric("PeakIOTasks").unwrap_or(100.0);
        if wait_time > 1_000_000_000.0 && peak_tasks < 10.0 {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Warning,
                node_path: format!("{} (plan_node_id={})", context.node.operator_name, context.node.plan_node_id.unwrap_or(-1)),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
 Lang::Zh => format!("IO 线程池可能已饱和，等待时间 {:.1}s", wait_time / 1_000_000_000.0),
 Lang::En => format!("IO thread pool may be saturated; wait time {:.1}s", wait_time / 1_000_000_000.0),
 },
                reason: context.lang.pick("IO 线程池使用率过高，导致 IO 任务等待时间过长。可能是并发查询过多或存储性能不足。", "The IO thread pool is heavily utilized, causing long IO task wait times. There may be too many concurrent queries or insufficient storage performance.").to_string(),
                suggestions: vec![context.lang.pick("增加 BE 上的 max_io_threads 配置", "Increase the max_io_threads setting on BE").to_string()],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// S006: Rowset fragmentation
pub struct S006RowsetFragmentation;

impl DiagnosticRule for S006RowsetFragmentation {
    fn id(&self) -> &str {
        "S006"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("Rowset 碎片化", "Rowset Fragmentation")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SCAN")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let rowsets = context.get_metric("RowsetsReadCount").unwrap_or(0.0);
        let init_time = context.get_metric("SegmentInitTime").unwrap_or(0.0);
        if rowsets > 100.0 && init_time > 500_000_000.0 {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Warning,
                node_path: format!("{} (plan_node_id={})", context.node.operator_name, context.node.plan_node_id.unwrap_or(-1)),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
 Lang::Zh => format!("Rowset 数量过多 ({:.0})，初始化耗时 {:.1}ms", rowsets, init_time / 1_000_000.0),
 Lang::En => format!("Too many Rowsets ({:.0}); initialization took {:.1}ms", rowsets, init_time / 1_000_000.0),
 },
                reason: context.lang.pick("Rowset 数量过多导致 Segment 初始化时间过长。通常是频繁小批量导入或 Compaction 不及时导致。", "Too many Rowsets make Segment initialization slow. This is usually caused by frequent small loads or Compaction falling behind.").to_string(),
                suggestions: vec![context.lang.pick("触发手动 Compaction", "Trigger a manual Compaction").to_string(), context.lang.pick("批量合并小型导入任务", "Batch small load jobs together").to_string()],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// S008: ZoneMap index not effective
pub struct S008ZoneMapNotEffective;

impl DiagnosticRule for S008ZoneMapNotEffective {
    fn id(&self) -> &str {
        "S008"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("ZoneMap 索引未生效", "ZoneMap Index Not Effective")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SCAN")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let zonemap_rows = context.get_metric("ZoneMapIndexFilterRows").unwrap_or(0.0);
        let raw_rows = context.get_metric("RawRowsRead").unwrap_or(0.0);
        if zonemap_rows == 0.0 && raw_rows > 100000.0 {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Info,
                node_path: format!("{} (plan_node_id={})", context.node.operator_name, context.node.plan_node_id.unwrap_or(-1)),
                plan_node_id: context.node.plan_node_id,
                message: context.lang.pick("ZoneMap 索引未能过滤数据", "The ZoneMap index did not filter any data").to_string(),
                reason: context.lang.pick("ZoneMap 索引未能有效过滤数据。ZoneMap 基于排序键的 min/max 值过滤，需要查询条件包含排序键或前缀列。", "The ZoneMap index did not filter data effectively. ZoneMap filters on min/max values of the sort key, so predicates must include the sort key or its prefix columns.").to_string(),
                suggestions: vec![context.lang.pick("确保查询条件包含排序键或前缀列", "Make sure predicates include the sort key or its prefix columns").to_string()],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// S012: Bitmap index not effective
/// Condition: BitmapIndexFilterRows = 0 with low cardinality column filter
///
/// Reason: Bitmap索引适用于基数较低且大量重复的字段（如性别、状态）。
/// 如果查询条件包含这类字段但未命中Bitmap索引，可能是：
/// 1. 未创建Bitmap索引
/// 2. 查询条件不支持Bitmap索引（如范围查询）
/// 3. 优化器选择了其他索引
pub struct S012BitmapIndexNotEffective;

impl DiagnosticRule for S012BitmapIndexNotEffective {
    fn id(&self) -> &str {
        "S012"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("Bitmap 索引未生效", "Bitmap Index Not Effective")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SCAN")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let bitmap_rows = context.get_metric("BitmapIndexFilterRows").unwrap_or(0.0);
        let raw_rows = context.get_metric("RawRowsRead").unwrap_or(0.0);

        // Only trigger if scanning significant data and bitmap filter is 0
        if bitmap_rows == 0.0 && raw_rows > 100_000.0 {
            // Check if there's expression filter (suggesting there are filter conditions)
            let expr_filter = context.get_metric("ExprFilterRows").unwrap_or(0.0);
            if expr_filter > raw_rows * 0.1 {
                return Some(Diagnostic {
                    rule_id: self.id().to_string(),
                    rule_name: self.name(context.lang).to_string(),
                    severity: RuleSeverity::Info,
                    node_path: format!("{} (plan_node_id={})", 
                        context.node.operator_name,
                        context.node.plan_node_id.unwrap_or(-1)),
                    plan_node_id: context.node.plan_node_id,
                    message: match context.lang {
 Lang::Zh => format!("Bitmap 索引未过滤数据，表达式过滤了 {:.0} 行", expr_filter),
 Lang::En => format!("Bitmap index filtered no data; expressions filtered {:.0} rows", expr_filter),
 },
                    reason: context.lang.pick("Bitmap 索引适用于基数较低且大量重复的字段（如性别、状态）。如果查询条件包含这类字段但未命中索引，可能是未创建索引或查询条件不支持。", "Bitmap indexes suit low-cardinality columns with many repeated values (such as gender or status). If predicates use such columns but no index is hit, the index may not exist or the predicate may not be supported.").to_string(),
                suggestions: vec![
                        context.lang.pick("对低基数列（如状态、类型）创建 Bitmap 索引", "Create Bitmap indexes on low-cardinality columns (such as status or type)").to_string(),
                        context.lang.pick("确保查询条件使用等值匹配 (=, IN)", "Make sure predicates use equality matching (=, IN)").to_string(),
                        context.lang.pick("检查 Profile 中 BitmapIndexFilterRows 指标", "Check the BitmapIndexFilterRows metric in the Profile").to_string(),
                    ],
                    parameter_suggestions: vec![],
                });
            }
        }
        None
    }
}

/// S013: Bloom filter index not effective
/// Condition: BloomFilterFilterRows = 0 with high cardinality column filter
///
/// Reason: Bloom Filter索引适用于高基数列（如ID列）的等值查询。
/// 如果查询条件包含这类字段但未命中Bloom Filter索引，可能是：
/// 1. 未创建Bloom Filter索引
/// 2. 查询条件不是等值匹配（Bloom Filter仅支持 = 和 IN）
/// 3. 列类型不支持（TINYINT, FLOAT, DOUBLE, DECIMAL不支持）
pub struct S013BloomFilterNotEffective;

impl DiagnosticRule for S013BloomFilterNotEffective {
    fn id(&self) -> &str {
        "S013"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("Bloom Filter 索引未生效", "Bloom Filter Index Not Effective")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        node.operator_name.to_uppercase().contains("SCAN")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        let bloom_rows = context.get_metric("BloomFilterFilterRows").unwrap_or(0.0);
        let raw_rows = context.get_metric("RawRowsRead").unwrap_or(0.0);

        // Only trigger if scanning significant data and bloom filter is 0
        if bloom_rows == 0.0 && raw_rows > 100_000.0 {
            // Check if there's expression filter on potential ID columns
            let expr_filter = context.get_metric("ExprFilterRows").unwrap_or(0.0);
            if expr_filter > raw_rows * 0.5 {
                return Some(Diagnostic {
                    rule_id: self.id().to_string(),
                    rule_name: self.name(context.lang).to_string(),
                    severity: RuleSeverity::Info,
                    node_path: format!("{} (plan_node_id={})", 
                        context.node.operator_name,
                        context.node.plan_node_id.unwrap_or(-1)),
                    plan_node_id: context.node.plan_node_id,
                    message: match context.lang {
 Lang::Zh => format!("Bloom Filter 索引未过滤数据，表达式过滤了 {:.0} 行", expr_filter),
 Lang::En => format!("Bloom Filter index filtered no data; expressions filtered {:.0} rows", expr_filter),
 },
                    reason: context.lang.pick("Bloom Filter 索引适用于高基数列（如 ID 列）的等值查询。仅支持 = 和 IN 条件，且 TINYINT/FLOAT/DOUBLE/DECIMAL 类型不支持。", "Bloom Filter indexes suit equality queries on high-cardinality columns (such as ID columns). Only = and IN are supported, and TINYINT/FLOAT/DOUBLE/DECIMAL columns are not supported.").to_string(),
                suggestions: vec![
                        context.lang.pick("对高基数列（如 ID 列）创建 Bloom Filter 索引", "Create Bloom Filter indexes on high-cardinality columns (such as ID columns)").to_string(),
                        context.lang.pick("确保查询条件使用等值匹配 (=, IN)", "Make sure predicates use equality matching (=, IN)").to_string(),
                        context.lang.pick("注意: TINYINT/FLOAT/DOUBLE/DECIMAL 类型不支持 Bloom Filter", "Note: TINYINT/FLOAT/DOUBLE/DECIMAL columns do not support Bloom Filter").to_string(),
                        context.lang.pick("检查 Profile 中 BloomFilterFilterRows 指标", "Check the BloomFilterFilterRows metric in the Profile").to_string(),
                    ],
                    parameter_suggestions: vec![],
                });
            }
        }
        None
    }
}

/// S014: Colocate Join opportunity missed
/// Condition: Shuffle Join on tables that could be colocated
///
/// Reason: Colocate Join可以避免数据网络传输，显著提升Join性能。
/// 当两个表的分桶键相同且分桶数相同时，可以使用Colocate Join。
pub struct S014ColocateJoinOpportunity;

impl DiagnosticRule for S014ColocateJoinOpportunity {
    fn id(&self) -> &str {
        "S014"
    }
    fn name(&self, lang: Lang) -> &str {
        lang.pick("可优化为 Colocate Join", "Colocate Join Opportunity")
    }

    fn applicable_to(&self, node: &ExecutionTreeNode) -> bool {
        let name = node.operator_name.to_uppercase();
        name.contains("HASH") && name.contains("JOIN") && name.contains("SHUFFLE")
    }

    fn evaluate(&self, context: &RuleContext) -> Option<Diagnostic> {
        // Check if this is a shuffle join with significant network transfer
        let bytes_sent = context
            .get_metric("BytesSent")
            .or_else(|| context.get_metric("NetworkBytesSent"))
            .unwrap_or(0.0);

        const HUNDRED_MB: f64 = 100.0 * 1024.0 * 1024.0;

        if bytes_sent > HUNDRED_MB {
            Some(Diagnostic {
                rule_id: self.id().to_string(),
                rule_name: self.name(context.lang).to_string(),
                severity: RuleSeverity::Info,
                node_path: format!("{} (plan_node_id={})", 
                    context.node.operator_name,
                    context.node.plan_node_id.unwrap_or(-1)),
                plan_node_id: context.node.plan_node_id,
                message: match context.lang {
 Lang::Zh => format!("Shuffle Join 网络传输 {}，考虑使用 Colocate Join 优化", format_bytes(bytes_sent as u64)),
 Lang::En => format!("Shuffle Join transferred {} over the network; consider optimizing with Colocate Join", format_bytes(bytes_sent as u64)),
 },
                reason: context.lang.pick("Colocate Join 可以避免数据网络传输，显著提升 Join 性能。当两个表的分桶键相同且分桶数相同时，可以使用 Colocate Join。", "Colocate Join avoids transferring data over the network and significantly improves Join performance. It can be used when both tables have the same bucketing key and the same number of Buckets.").to_string(),
                suggestions: vec![
                    context.lang.pick("将频繁 Join 的表设置为同一 Colocation Group", "Put frequently joined tables in the same Colocation Group").to_string(),
                    context.lang.pick("确保两表的分桶键和分桶数相同", "Make sure both tables have the same bucketing key and number of Buckets").to_string(),
                    context.lang.pick("使用 SHOW COLOCATION GROUP 查看现有分组", "Use SHOW COLOCATION GROUP to view existing groups").to_string(),
                    context.lang.pick("Colocate Join 可避免数据网络传输，显著提升性能", "Colocate Join avoids network data transfer and significantly improves performance").to_string(),
                ],
                parameter_suggestions: vec![],
            })
        } else {
            None
        }
    }
}

/// Get all scan rules
pub fn get_rules() -> Vec<Box<dyn DiagnosticRule>> {
    vec![
        Box::new(S001DataSkew),
        Box::new(S002IOSkew),
        Box::new(S003PoorFilter),
        Box::new(S004PredicateNotPushed),
        Box::new(S005IOThreadPoolSaturation),
        Box::new(S006RowsetFragmentation),
        Box::new(S007ColdStorage),
        Box::new(S008ZoneMapNotEffective),
        Box::new(S009LowCacheHit),
        Box::new(S010RFNotEffective),
        Box::new(S011SoftDeletes),
        Box::new(S012BitmapIndexNotEffective),
        Box::new(S013BloomFilterNotEffective),
        Box::new(S014ColocateJoinOpportunity),
    ]
}
