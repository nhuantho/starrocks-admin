//! Trigger, no-trigger and missing-counter tests for every Doris rule.
//!
//! Profiles here are synthetic but use only counter names and formats from
//! docs/profile/doris-counter-catalog.md (observed in 4.1.3 or cited Doris docs).
use super::*;

fn header(total: &str, extra: &str) -> String {
    format!(
        "Summary:\n   - Profile ID: t\n   - Total: {total}\n   - Task State: OK\nExecution Summary:\n{extra}   - Doris Version: doris-4.1.3-rc02\n"
    )
}

/// One fragment, one pipeline; `ops` are operator blocks listed consumer first.
fn profile(total: &str, instances: usize, ops: &str) -> String {
    format!(
        "{}MergedProfile:\n  Fragments:\n    Fragment 0:\n      Pipeline 0(instance_num={instances}):\n{ops}",
        header(total, "")
    )
}

fn op(name: &str, id: i32, counters: &[&str]) -> String {
    let mut block = format!("        {name}(id={id}):\n          CommonCounters:\n");
    for counter in counters {
        block.push_str(&format!("             - {counter}\n"));
    }
    block
}

fn analyze_full(text: &str, variables: Option<&[(&str, &str)]>) -> ProfileAnalysisResponse {
    let context = AnalysisContext {
        lang: Lang::En,
        cluster_variables: variables.map(|vars| {
            vars.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        }),
    };
    analyze_with(text, &context, &DorisOptions { experimental_rules: true }).unwrap()
}

fn find(response: &ProfileAnalysisResponse, rule: &str) -> Option<DiagnosticResult> {
    response
        .diagnostics
        .iter()
        .find(|d| d.rule_id == rule)
        .cloned()
}

fn fires(text: &str, rule: &str) -> Option<DiagnosticResult> {
    find(&analyze_full(text, None), rule)
}

fn result_sink() -> String {
    op("RESULT_SINK_OPERATOR", 9, &["ExecTime: avg 1ms, max 1ms, min 1ms"])
}

// ---------------------------------------------------------------- generic operator rules

#[test]
fn exec_skew_warning_critical_and_missing() {
    let skewed = |max: &str| {
        profile(
            "10sec",
            4,
            &(result_sink()
                + &*op(
                    "HASH_JOIN_OPERATOR",
                    3,
                    &[&format!("ExecTime: avg 100ms, max {max}, min 1ms")],
                )),
        )
    };
    assert_eq!(fires(&skewed("300ms"), "DORIS_EXEC_SKEW").unwrap().severity, "Warning");
    assert_eq!(
        fires(&skewed("1500ms"), "DORIS_EXEC_SKEW")
            .unwrap()
            .severity,
        "Critical"
    );
    assert!(fires(&skewed("150ms"), "DORIS_EXEC_SKEW").is_none());
    let missing = profile(
        "10sec",
        4,
        &(result_sink()
            + &*op("HASH_JOIN_OPERATOR", 3, &["RowsProduced: sum 5, avg 1, max 2, min 1"])),
    );
    assert!(fires(&missing, "DORIS_EXEC_SKEW").is_none());
}

#[test]
fn row_skew_trigger_no_trigger_missing() {
    let rows = |max: u32| {
        profile(
            "1sec",
            4,
            &(result_sink()
                + &*op(
                    "LOCAL_EXCHANGE_OPERATOR",
                    2,
                    &[
                        "ExecTime: avg 1ms, max 1ms, min 1ms",
                        &format!("RowsProduced: sum 4000, avg 1000, max {max}, min 0"),
                    ],
                )),
        )
    };
    assert!(fires(&rows(3000), "DORIS_ROW_SKEW").is_some());
    assert!(fires(&rows(1500), "DORIS_ROW_SKEW").is_none());
    let missing = profile(
        "1sec",
        4,
        &(result_sink() + &*op("LOCAL_EXCHANGE_OPERATOR", 2, &["RowsProduced: sum 4000"])),
    );
    assert!(fires(&missing, "DORIS_ROW_SKEW").is_none());
}

#[test]
fn operator_memory_absolute_relative_and_missing() {
    let mem = |peak: &str| {
        profile(
            "1sec",
            1,
            &(result_sink()
                + &*op(
                    "AGGREGATION_SINK_OPERATOR",
                    4,
                    &[
                        "ExecTime: avg 1ms, max 1ms, min 1ms",
                        &format!("MemoryUsagePeak: sum {peak}, avg {peak}, max {peak}, min {peak}"),
                    ],
                )),
        )
    };
    // Without exec_mem_limit only an absolute 4 GiB threshold applies.
    assert_eq!(
        fires(&mem("5.00 GB"), "DORIS_OPERATOR_MEMORY")
            .unwrap()
            .severity,
        "Warning"
    );
    assert!(fires(&mem("1.00 GB"), "DORIS_OPERATOR_MEMORY").is_none());
    let vars = [("exec_mem_limit", "2147483648"), ("enable_spill", "false")];
    let critical =
        find(&analyze_full(&mem("1.80 GB"), Some(&vars)), "DORIS_OPERATOR_MEMORY").unwrap();
    assert_eq!(critical.severity, "Critical");
    let names: Vec<_> = critical
        .parameter_suggestions
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(names, ["exec_mem_limit", "enable_spill"]);
    assert!(
        critical
            .parameter_suggestions
            .iter()
            .all(|p| p.current.is_some() && p.scope.as_deref() == Some("session"))
    );
    let missing = profile(
        "1sec",
        1,
        &(result_sink()
            + &*op("AGGREGATION_SINK_OPERATOR", 4, &["ExecTime: avg 1ms, max 1ms, min 1ms"])),
    );
    assert!(fires(&missing, "DORIS_OPERATOR_MEMORY").is_none());
}

#[test]
fn peak_memory_alias_from_older_releases_is_read() {
    // 2.1/3.x docs name the counter PeakMemoryUsage; the version adapter maps it.
    let text = profile(
        "1sec",
        1,
        &(result_sink()
            + &*op(
                "SORT_SINK_OPERATOR",
                4,
                &[
                    "ExecTime: avg 1ms, max 1ms, min 1ms",
                    "PeakMemoryUsage: sum 6.00 GB, avg 6.00 GB, max 6.00 GB, min 6.00 GB",
                ],
            )),
    );
    assert!(fires(&text, "DORIS_OPERATOR_MEMORY").is_some());
}

#[test]
fn spill_is_experimental() {
    let text = profile(
        "1sec",
        1,
        &(result_sink()
            + &*op(
                "SORT_SINK_OPERATOR",
                4,
                &[
                    "ExecTime: avg 1ms, max 1ms, min 1ms",
                    "SpillWriteBytes: sum 1.00 GB, avg 1.00 GB, max 1.00 GB, min 1.00 GB",
                ],
            )),
    );
    assert!(fires(&text, "DORIS_SPILL").is_some());
    let default =
        analyze_with(&text, &AnalysisContext::default(), &DorisOptions::default()).unwrap();
    assert!(find(&default, "DORIS_SPILL").is_none());
    let zero = text.replace("1.00 GB", "0.00 B");
    assert!(fires(&zero, "DORIS_SPILL").is_none());
}

// ---------------------------------------------------------------- scans

fn scan(counters: &[&str]) -> String {
    profile("1sec", 1, &(result_sink() + &*op("JDBC_SCAN_OPERATOR", 0, counters)))
}

#[test]
fn connector_init_scan_wait_and_volume() {
    let text = scan(&[
        "ExecTime: avg 40ms, max 40ms, min 40ms",
        "InitConnectorTime: 30ms",
        "WaitForDependency[JDBC_SCAN_OPERATOR_DEPENDENCY]Time: avg 30ms, max 30ms, min 30ms",
        "RowsRead: 1000000",
        "RowsProduced: sum 10, avg 10, max 10, min 10",
    ]);
    for rule in ["DORIS_CONNECTOR_INIT", "DORIS_SCAN_WAIT", "DORIS_SCAN_VOLUME"] {
        assert!(fires(&text, rule).is_some(), "{rule}");
    }
    let quiet = scan(&[
        "ExecTime: avg 40ms, max 40ms, min 40ms",
        "InitConnectorTime: 1ms",
        "WaitForDependency[JDBC_SCAN_OPERATOR_DEPENDENCY]Time: avg 1ms, max 1ms, min 1ms",
        "RowsRead: 1000",
        "RowsProduced: sum 900, avg 900, max 900, min 900",
    ]);
    for rule in ["DORIS_CONNECTOR_INIT", "DORIS_SCAN_WAIT", "DORIS_SCAN_VOLUME"] {
        assert!(fires(&quiet, rule).is_none(), "{rule}");
    }
    let missing = scan(&["RowsProduced: sum 10, avg 10, max 10, min 10"]);
    for rule in ["DORIS_CONNECTOR_INIT", "DORIS_SCAN_WAIT", "DORIS_SCAN_VOLUME"] {
        assert!(fires(&missing, rule).is_none(), "{rule}");
    }
}

#[test]
fn scan_volume_uses_documented_raw_rows_read_when_rows_read_is_absent() {
    let text = scan(&[
        "ExecTime: avg 1ms, max 1ms, min 1ms",
        "RawRowsRead: sum 500000, avg 500000, max 500000, min 500000",
        "RowsProduced: sum 1, avg 1, max 1, min 1",
    ]);
    assert!(
        fires(&text, "DORIS_SCAN_VOLUME")
            .unwrap()
            .reason
            .contains("RawRowsRead")
    );
}

#[test]
fn runtime_filter_and_scanner_queue_waits() {
    let text = scan(&[
        "ExecTime: avg 1ms, max 1ms, min 1ms",
        "WaitForRuntimeFilter: 400ms",
        "ScannerWorkerWaitTime: 300ms",
    ]);
    let response = analyze_full(&text, None);
    assert!(find(&response, "DORIS_RUNTIME_FILTER_WAIT").is_some());
    assert!(find(&response, "DORIS_SCANNER_QUEUE").is_some());
    let small = scan(&[
        "ExecTime: avg 1ms, max 1ms, min 1ms",
        "WaitForRuntimeFilter: 20ms",
        "ScannerWorkerWaitTime: 50ms",
    ]);
    assert!(find(&analyze_full(&small, None), "DORIS_RUNTIME_FILTER_WAIT").is_none());
    assert!(find(&analyze_full(&small, None), "DORIS_SCANNER_QUEUE").is_none());
    // Without Total the share is unknown, so no claim is made.
    let no_total = text.replace("   - Total: 1sec\n", "");
    assert!(find(&analyze_full(&no_total, None), "DORIS_RUNTIME_FILTER_WAIT").is_none());
}

#[test]
fn olap_rules_are_experimental_and_need_their_counters() {
    let olap = |counters: &[&str]| {
        profile("1sec", 1, &(result_sink() + &*op("OLAP_SCAN_OPERATOR", 0, counters)))
    };
    let text = olap(&[
        "ExecTime: avg 100ms, max 100ms, min 100ms",
        "RawRowsRead: sum 5000000, avg 5000000, max 5000000, min 5000000",
        "RowsKeyRangeFiltered: sum 0, avg 0, max 0, min 0",
        "RowsProduced: sum 100, avg 100, max 100, min 100",
        "IOTimer: avg 80ms, max 80ms, min 80ms",
        "NumSegmentTotal: sum 4000, avg 4000, max 4000, min 4000",
        "TabletNum: sum 10, avg 10, max 10, min 10",
        "CachedPagesNum: sum 10, avg 10, max 10, min 10",
        "TotalPagesNum: sum 50000, avg 50000, max 50000, min 50000",
    ]);
    let rules = [
        "DORIS_OLAP_INDEX_FILTER",
        "DORIS_OLAP_IO",
        "DORIS_OLAP_SEGMENTS",
        "DORIS_OLAP_PAGE_CACHE",
    ];
    for rule in rules {
        assert!(fires(&text, rule).is_some(), "{rule}");
    }
    let default =
        analyze_with(&text, &AnalysisContext::default(), &DorisOptions::default()).unwrap();
    assert!(rules.iter().all(|rule| find(&default, rule).is_none()));
    let missing = olap(&["ExecTime: avg 100ms, max 100ms, min 100ms"]);
    assert!(rules.iter().all(|rule| fires(&missing, rule).is_none()));
}

// ---------------------------------------------------------------- joins

fn join(probe: &[&str], build: &[&str], join_op: &str) -> String {
    let probe_block = op("HASH_JOIN_OPERATOR", 3, probe).replace(
        "          CommonCounters:\n",
        &format!(
            "           - PlanInfo\n              - join op: {join_op}\n          CommonCounters:\n"
        ),
    );
    format!(
        "{}MergedProfile:\n  Fragments:\n    Fragment 0:\n      Pipeline 0(instance_num=8):\n{}{}      Pipeline 1(instance_num=8):\n{}",
        header("10sec", ""),
        result_sink(),
        probe_block,
        op("HASH_JOIN_SINK_OPERATOR", 3, build)
    )
}

#[test]
fn join_expansion_warning_critical_and_missing() {
    let probe = |out: u64| {
        join(
            &[
                "ExecTime: avg 1ms, max 1ms, min 1ms",
                &format!("RowsProduced: sum {out}, avg 1, max 1, min 1"),
                "ProbeRows: sum 100000, avg 1, max 1, min 1",
            ],
            &["ExecTime: avg 1ms, max 1ms, min 1ms"],
            "INNER JOIN(PARTITIONED)[]",
        )
    };
    assert_eq!(
        fires(&probe(2_000_000), "DORIS_JOIN_EXPANSION")
            .unwrap()
            .severity,
        "Warning"
    );
    assert_eq!(
        fires(&probe(20_000_000), "DORIS_JOIN_EXPANSION")
            .unwrap()
            .severity,
        "Critical"
    );
    assert!(fires(&probe(200_000), "DORIS_JOIN_EXPANSION").is_none());
    let missing = join(
        &["ExecTime: avg 1ms, max 1ms, min 1ms", "RowsProduced: sum 2000000, avg 1, max 1, min 1"],
        &[],
        "INNER JOIN(PARTITIONED)[]",
    );
    assert!(fires(&missing, "DORIS_JOIN_EXPANSION").is_none());
}

#[test]
fn build_larger_than_probe_and_broadcast_size() {
    let text = join(
        &["ExecTime: avg 1ms, max 1ms, min 1ms", "ProbeRows: sum 1000, avg 125, max 200, min 0"],
        &[
            "ExecTime: avg 1ms, max 1ms, min 1ms",
            "InputRows: sum 20000000, avg 20000000, max 20000000, min 0",
            "MemoryUsagePeak: sum 2.00 GB, avg 2.00 GB, max 2.00 GB, min 0.00",
        ],
        "LEFT OUTER JOIN(BROADCAST)[]",
    );
    let response = analyze_full(&text, None);
    assert!(find(&response, "DORIS_JOIN_BUILD_LARGER").is_some());
    let broadcast = find(&response, "DORIS_BROADCAST_LARGE").unwrap();
    assert!(broadcast.sql_snippets[0].sql.contains("[shuffle]"));
    let small = join(
        &["ExecTime: avg 1ms, max 1ms, min 1ms", "ProbeRows: sum 50000000, avg 1, max 1, min 1"],
        &["ExecTime: avg 1ms, max 1ms, min 1ms", "InputRows: sum 100, avg 100, max 100, min 0"],
        "LEFT OUTER JOIN(BROADCAST)[]",
    );
    let response = analyze_full(&small, None);
    assert!(find(&response, "DORIS_JOIN_BUILD_LARGER").is_none());
    assert!(find(&response, "DORIS_BROADCAST_LARGE").is_none());
    // Missing InputRows: no claim about build size.
    let missing = join(
        &["ExecTime: avg 1ms, max 1ms, min 1ms", "ProbeRows: sum 1, avg 1, max 1, min 1"],
        &["ExecTime: avg 1ms, max 1ms, min 1ms"],
        "INNER JOIN(BROADCAST)[]",
    );
    assert!(find(&analyze_full(&missing, None), "DORIS_JOIN_BUILD_LARGER").is_none());
}

#[test]
fn shuffle_of_tiny_build_is_experimental() {
    let text = join(
        &["ExecTime: avg 1ms, max 1ms, min 1ms", "ProbeRows: sum 50000000, avg 1, max 1, min 1"],
        &["ExecTime: avg 1ms, max 1ms, min 1ms", "InputRows: sum 100, avg 12, max 20, min 0"],
        "INNER JOIN(PARTITIONED)[]",
    );
    assert!(fires(&text, "DORIS_SHUFFLE_SMALL_BUILD").is_some());
    assert!(
        find(
            &analyze_with(&text, &AnalysisContext::default(), &DorisOptions::default()).unwrap(),
            "DORIS_SHUFFLE_SMALL_BUILD"
        )
        .is_none()
    );
    let broadcast = text.replace("(PARTITIONED)", "(BROADCAST)");
    assert!(fires(&broadcast, "DORIS_SHUFFLE_SMALL_BUILD").is_none());
}

#[test]
fn probe_waiting_for_build_side() {
    let text = join(
        &[
            "ExecTime: avg 1ms, max 1ms, min 1ms",
            "WaitForDependency[HASH_JOIN_PROBE_DEPENDENCY]Time: avg 8sec, max 8sec, min 8sec",
        ],
        &["ExecTime: avg 1ms, max 1ms, min 1ms"],
        "INNER JOIN(BROADCAST)[]",
    );
    assert!(fires(&text, "DORIS_JOIN_BUILD_WAIT").is_some());
    assert!(
        fires(
            &text.replace("avg 8sec, max 8sec, min 8sec", "avg 1sec, max 1sec, min 1sec"),
            "DORIS_JOIN_BUILD_WAIT"
        )
        .is_none()
    );
}

#[test]
fn nested_loop_join_is_experimental() {
    let text = profile(
        "1sec",
        1,
        &(result_sink()
            + &*op("NESTED_LOOP_JOIN_PROBE_OPERATOR", 3, &["ExecTime: avg 1ms, max 1ms, min 1ms"])),
    );
    assert!(fires(&text, "DORIS_NESTED_LOOP_JOIN").is_some());
    assert!(
        find(
            &analyze_with(&text, &AnalysisContext::default(), &DorisOptions::default()).unwrap(),
            "DORIS_NESTED_LOOP_JOIN"
        )
        .is_none()
    );
}

// ---------------------------------------------------------------- aggregation, sort, exchange

#[test]
fn streaming_aggregation_reduction_uses_upstream_rows() {
    let agg = |out: u64| {
        profile(
            "1sec",
            1,
            &(result_sink()
                + &*op(
                    "STREAMING_AGGREGATION_OPERATOR",
                    4,
                    &[
                        "ExecTime: avg 1ms, max 1ms, min 1ms",
                        &format!("RowsProduced: sum {out}, avg 1, max 1, min 1"),
                    ],
                )
                + &*op(
                    "JDBC_SCAN_OPERATOR",
                    0,
                    &[
                        "ExecTime: avg 1ms, max 1ms, min 1ms",
                        "RowsProduced: sum 2000000, avg 1, max 1, min 1",
                    ],
                )),
        )
    };
    assert!(fires(&agg(1_950_000), "DORIS_STREAMING_AGG_INEFFECTIVE").is_some());
    assert!(fires(&agg(10_000), "DORIS_STREAMING_AGG_INEFFECTIVE").is_none());
    let alone = profile(
        "1sec",
        1,
        &(result_sink()
            + &*op(
                "STREAMING_AGGREGATION_OPERATOR",
                4,
                &["RowsProduced: sum 1950000, avg 1, max 1, min 1"],
            )),
    );
    // No upstream operator, so the input is unknown and nothing is claimed.
    assert!(fires(&alone, "DORIS_STREAMING_AGG_INEFFECTIVE").is_none());
}

#[test]
fn aggregation_cardinality_and_distinct() {
    let agg = |size: u64, output: &str| {
        let block = op("AGGREGATION_OPERATOR", 5, &["ExecTime: avg 1ms, max 1ms, min 1ms", &format!("HashTableSize: sum {size}, avg 1, max 1, min 1"), "MemoryUsageHashTable: sum 512.00 MB, avg 512.00 MB, max 512.00 MB, min 0.00"])
            .replace("          CommonCounters:\n", &format!("           - PlanInfo\n              - output: {output}\n          CommonCounters:\n"));
        profile("1sec", 1, &(result_sink() + &*block))
    };
    assert!(fires(&agg(20_000_000, "count(id)"), "DORIS_AGG_HIGH_CARDINALITY").is_some());
    assert!(fires(&agg(1_000, "count(id)"), "DORIS_AGG_HIGH_CARDINALITY").is_none());
    assert!(fires(&agg(1_000, "multi_distinct_count(id)"), "DORIS_COUNT_DISTINCT").is_some());
    assert!(fires(&agg(1_000, "count(id)"), "DORIS_COUNT_DISTINCT").is_none());
}

#[test]
fn large_sort_respects_limit_on_same_plan_node() {
    let sort = profile(
        "1sec",
        1,
        &(result_sink()
            + &*op(
                "SORT_SINK_OPERATOR",
                7,
                &[
                    "ExecTime: avg 1ms, max 1ms, min 1ms",
                    "InputRows: sum 50000000, avg 1, max 1, min 1",
                ],
            )),
    );
    assert!(fires(&sort, "DORIS_LARGE_SORT").is_some());
    let limited = sort.replace(
        "        SORT_SINK_OPERATOR(id=7):\n",
        "        LOCAL_MERGE_SORT_SOURCE_OPERATOR(id=7):\n           - PlanInfo\n              - limit: 5\n          CommonCounters:\n             - ExecTime: avg 1ms, max 1ms, min 1ms\n        SORT_SINK_OPERATOR(id=7):\n",
    );
    assert!(fires(&limited, "DORIS_LARGE_SORT").is_none());
    assert!(
        fires(&sort.replace("InputRows: sum 50000000", "InputRows: sum 5000"), "DORIS_LARGE_SORT")
            .is_none()
    );
}

#[test]
fn exchange_backpressure_and_volume() {
    let sink = |wait: &str, bytes: &str| {
        profile(
            "1sec",
            1,
            &(result_sink()
                + &*op(
                    "DATA_STREAM_SINK_OPERATOR",
                    1,
                    &[
                        "ExecTime: avg 1ms, max 1ms, min 1ms",
                        &format!("WaitForRpcBufferQueue: avg {wait}, max {wait}, min {wait}"),
                        &format!("BytesSent: {bytes}"),
                    ],
                )),
        )
    };
    let response = analyze_full(&sink("500ms", "20.00 GB"), None);
    assert!(find(&response, "DORIS_SINK_BACKPRESSURE").is_some());
    assert!(find(&response, "DORIS_EXCHANGE_VOLUME").is_some());
    let quiet = analyze_full(&sink("1ms", "1.00 MB"), None);
    assert!(find(&quiet, "DORIS_SINK_BACKPRESSURE").is_none());
    assert!(find(&quiet, "DORIS_EXCHANGE_VOLUME").is_none());
}

// ---------------------------------------------------------------- query-level rules

#[test]
fn plan_schedule_and_result_write_phases() {
    let extra = "   - Parse SQL Time: 1ms\n   - Plan Time: 4sec\n     - Nereids Analysis Time: 100ms\n     - Nereids Optimize Time: 3sec\n   - Schedule Time: 3sec\n     - Fragment RPC Phase1 Time: 2sec\n   - Wait and Fetch Result Time: 2sec\n   - Write Result Time: 4sec\n";
    let text = format!(
        "{}MergedProfile:\n  Fragments:\n    Fragment 0:\n      Pipeline 0(instance_num=1):\n{}",
        header("10sec", extra),
        result_sink()
    );
    let response = analyze_full(&text, None);
    let plan = find(&response, "DORIS_PLAN_TIME").unwrap();
    assert!(plan.reason.contains("Nereids Optimize Time"));
    assert_eq!(plan.node_path, "query");
    assert!(find(&response, "DORIS_SCHEDULE_TIME").is_some());
    assert!(find(&response, "DORIS_RESULT_WRITE").is_some());
    let phases = response.phase_breakdown.unwrap();
    assert!(phases.additive);
    assert_eq!(
        phases
            .segments
            .iter()
            .map(|s| s.key.as_str())
            .collect::<Vec<_>>(),
        ["parse", "plan", "schedule", "execute_fetch"]
    );
    let fast = text
        .replace("Plan Time: 4sec", "Plan Time: 10ms")
        .replace("Schedule Time: 3sec", "Schedule Time: 5ms")
        .replace("Write Result Time: 4sec", "Write Result Time: 1ms");
    let response = analyze_full(&fast, None);
    for rule in ["DORIS_PLAN_TIME", "DORIS_SCHEDULE_TIME", "DORIS_RESULT_WRITE"] {
        assert!(find(&response, rule).is_none(), "{rule}");
    }
    // Unknown Total: shares cannot be computed and nothing is claimed.
    let response = analyze_full(&text.replace("   - Total: 10sec\n", ""), None);
    assert!(find(&response, "DORIS_PLAN_TIME").is_none());
    assert!(response.phase_breakdown.unwrap().total_ms.is_none());
}

#[test]
fn overlapping_phases_are_not_stacked() {
    let extra = "   - Plan Time: 8sec\n   - Wait and Fetch Result Time: 8sec\n";
    let text = format!(
        "{}MergedProfile:\n  Fragments:\n    Fragment 0:\n      Pipeline 0(instance_num=1):\n{}",
        header("10sec", extra),
        result_sink()
    );
    let phases = analyze_full(&text, None).phase_breakdown.unwrap();
    assert!(!phases.additive);
    assert!(phases.unaccounted_ms.is_none());
}

#[test]
fn external_metadata_dominating_plan() {
    let extra = "   - Plan Time: 400ms\n     - Nereids Preload External Metadata Time: 300ms\n";
    let text = format!(
        "{}MergedProfile:\n  Fragments:\n    Fragment 0:\n      Pipeline 0(instance_num=1):\n{}",
        header("1sec", extra),
        result_sink()
    );
    assert!(fires(&text, "DORIS_EXTERNAL_METADATA").is_some());
    assert!(
        fires(
            &text.replace("Metadata Time: 300ms", "Metadata Time: 1ms"),
            "DORIS_EXTERNAL_METADATA"
        )
        .is_none()
    );
}

#[test]
fn slow_backend_and_rpc_queue_from_schedule_json() {
    let json = r#"{"phase1":{"be1: 8060":{"RPC Work Time":"400ms","RPC Latency From FE To BE":"1ms","RPC Work Queue Time":"200ms","RPC Latency From BE To FE":"0ms"},"be2: 8060":{"RPC Work Time":"5ms","RPC Latency From FE To BE":"1ms","RPC Work Queue Time":"0ms","RPC Latency From BE To FE":"0ms"},"be3: 8060":{"RPC Work Time":"6ms","RPC Latency From FE To BE":"1ms","RPC Work Queue Time":"0ms","RPC Latency From BE To FE":"0ms"}}}"#;
    let text = format!(
        "{}MergedProfile:\n  Fragments:\n    Fragment 0:\n      Pipeline 0(instance_num=1):\n{}",
        header("1sec", &format!("   - Schedule Time Of BE: {json}\n")),
        result_sink()
    );
    let response = analyze_full(&text, None);
    assert!(
        find(&response, "DORIS_SLOW_BE_RPC")
            .unwrap()
            .reason
            .contains("be1:8060")
    );
    assert!(find(&response, "DORIS_BE_RPC_QUEUE").is_some());
    // The real single-backend fixture never flags a slow backend.
    let real = analyze_full(
        include_str!("../../../../tests/fixtures/profiles/doris-4.1.3-join-merged.txt"),
        None,
    );
    assert!(find(&real, "DORIS_SLOW_BE_RPC").is_none());
    // Malformed JSON is ignored, not a parse failure.
    let broken = text.replace(r#"{"phase1""#, r#"{"phase1"#);
    assert!(find(&analyze_full(&broken, None), "DORIS_SLOW_BE_RPC").is_none());
}

#[test]
fn failed_queries_map_known_causes_and_never_guess() {
    let failed = |state: &str, message: &str| {
        let text = profile("1sec", 1, &result_sink())
            .replace("Task State: OK", &format!("Task State: {state}"))
            .replace(
                "   - Doris Version",
                &format!("   - System Message: {message}\n   - Doris Version"),
            );
        analyze_full(&text, Some(&[("query_timeout", "300")]))
    };
    let timeout = find(&failed("ERR", "[TIMEOUT]query timeout"), "DORIS_QUERY_FAILED").unwrap();
    assert_eq!(timeout.severity, "Critical");
    assert_eq!(timeout.parameter_suggestions[0].recommended, "600");
    let memory =
        find(&failed("ERR", "MEM_LIMIT_EXCEEDED: exceeded"), "DORIS_QUERY_FAILED").unwrap();
    assert!(memory.rule_name.contains("Memory"));
    let unknown = find(&failed("ERR", "something odd"), "DORIS_QUERY_FAILED").unwrap();
    assert!(unknown.message.contains("not recognized"));
    assert!(unknown.parameter_suggestions.is_empty());
    assert!(find(&failed("OK", "N/A"), "DORIS_QUERY_FAILED").is_none());
}

#[test]
fn estimate_error_attaches_to_the_matching_scan() {
    let scan_block = op("JDBC_SCAN_OPERATOR", 0, &["ExecTime: avg 1ms, max 1ms, min 1ms"]).replace(
        "JDBC_SCAN_OPERATOR(id=0)",
        "JDBC_SCAN_OPERATOR(nereids_id=82. table_name=`db`.`t`)(id=0)",
    );
    let text = profile("1sec", 1, &(result_sink() + &*scan_block))
        + "\nAppendix:\n\nPhysicalPlan:\n+--PhysicalJdbcScan ( qualified=cat.db.t, stats=10 actualRows=5000000 )\n";
    let diagnostic = fires(&text, "DORIS_ESTIMATE_ERROR").unwrap();
    assert!(diagnostic.node_path.contains("JDBC_SCAN"));
    assert_eq!(diagnostic.sql_snippets[0].sql, "ANALYZE TABLE cat.db.t;");
    assert!(diagnostic.sql_snippets[0].warning.is_some());
    assert!(
        fires(&text.replace("actualRows=5000000", "actualRows=11"), "DORIS_ESTIMATE_ERROR")
            .is_none()
    );
    let other_table = text.replace("qualified=cat.db.t", "qualified=cat.db.other");
    assert_eq!(
        fires(&other_table, "DORIS_ESTIMATE_ERROR")
            .unwrap()
            .node_path,
        "query"
    );
}

// ---------------------------------------------------------------- confidence, ranking, score

#[test]
fn confidence_reflects_evidence_source_and_query_state() {
    let live = analyze_full(
        include_str!("../../../../tests/fixtures/profiles/doris-4.1.3-jdbc.txt"),
        None,
    );
    // InitConnectorTime exists only in DetailProfile.
    assert_eq!(
        find(&live, "DORIS_CONNECTOR_INIT")
            .unwrap()
            .confidence
            .as_deref(),
        Some("medium")
    );
    assert_eq!(
        find(&live, "DORIS_TIME_SHARE")
            .unwrap()
            .confidence
            .as_deref(),
        Some("high")
    );
    let running = include_str!("../../../../tests/fixtures/profiles/doris-4.1.3-jdbc.txt")
        .replace("Task State: OK", "Task State: RUNNING");
    let running = analyze_full(&running, None);
    assert_eq!(running.confidence.as_deref(), Some("low"));
    assert!(
        running
            .diagnostics
            .iter()
            .all(|d| d.confidence.as_deref() == Some("low"))
    );
}

#[test]
fn diagnostics_are_ranked_by_severity_then_impact() {
    let text = join(
        &[
            "ExecTime: avg 100ms, max 2000ms, min 1ms",
            "RowsProduced: sum 20000000, avg 1, max 1, min 1",
            "ProbeRows: sum 100000, avg 1, max 1, min 1",
        ],
        &["ExecTime: avg 1ms, max 1ms, min 1ms"],
        "INNER JOIN(PARTITIONED)[]",
    );
    let response = analyze_full(&text, None);
    let order: Vec<_> = response
        .diagnostics
        .iter()
        .map(|d| findings::severity_from_str(&d.severity))
        .collect();
    assert!(order.windows(2).all(|pair| pair[0] >= pair[1]));
    assert_eq!(response.diagnostics[0].severity, "Critical");
    // Two findings on one node produce one root-cause path.
    let paths = &response.root_cause_paths;
    let unique: std::collections::HashSet<_> = paths.iter().map(|p| p.node_ids.clone()).collect();
    assert_eq!(unique.len(), paths.len());
    assert!(response.performance_score < 100.0);
}

#[test]
fn advisory_findings_do_not_lower_the_score() {
    let real = analyze_full(
        include_str!("../../../../tests/fixtures/profiles/doris-4.1.3-join-merged.txt"),
        None,
    );
    let scored: Vec<_> = real
        .diagnostics
        .iter()
        .filter(|d| !rules::ADVISORY.contains(&d.rule_id.as_str()))
        .cloned()
        .collect();
    assert_eq!(real.performance_score, scoring::score(&scored));
    assert!(find(&real, "DORIS_PROFILE_LEVEL").is_some());
}

#[test]
fn profile_level_from_changed_session_variables() {
    let text = include_str!("../../../../tests/fixtures/profiles/doris-4.1.3-join-merged.txt").replace(
        "\"VarName\": \"enable_profile\",",
        "\"VarName\": \"profile_level\",\n    \"CurrentValue\": \"2\",\n    \"DefaultValue\": \"1\"\n  },\n  {\n    \"VarName\": \"enable_profile\",",
    );
    let response = analyze_full(&text, None);
    assert_eq!(response.profile_level, Some(2));
    assert!(find(&response, "DORIS_PROFILE_LEVEL").is_none());
    let response = analyze_full(&text, Some(&[("profile_level", "1")]));
    assert_eq!(response.profile_level, Some(2));
    assert!(find(&response, "DORIS_PROFILE_LEVEL").is_none());
}

#[test]
fn historical_profile_level_is_not_inferred_from_console_session() {
    let text = include_str!("../../../../tests/fixtures/profiles/doris-4.1.3-join-merged.txt");
    let response = analyze_full(text, Some(&[("profile_level", "2")]));
    assert_eq!(response.profile_level, None);
}

#[test]
fn captured_failure_keeps_summary_without_inventing_operators() {
    let text = include_str!("../../../../tests/fixtures/profiles/doris-4.1.3-failed-summary.txt");
    let response = analyze_full(text, None);
    assert!(response.execution_tree.is_none());
    assert!(response.operator_diagnostics.is_empty());
    assert!(response.root_cause_paths.is_empty());
    let summary = response.summary.as_ref().unwrap();
    assert_eq!(summary.query_state, "ERR");
    assert_eq!(summary.planner_total_time_ms, None);
    let failure = find(&response, "DORIS_QUERY_FAILED").unwrap();
    assert_eq!(failure.severity, "Warning");
    assert!(failure.message.contains("no cause is inferred"));
    assert!(failure.parameter_suggestions.is_empty());
    assert!(response.phase_breakdown.is_some());
    assert_eq!(response.profile_content.as_deref(), Some(text));
}
