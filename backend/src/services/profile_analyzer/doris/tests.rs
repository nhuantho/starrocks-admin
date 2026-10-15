use super::*;

const LIVE: &str = include_str!("../../../../tests/fixtures/profiles/doris-4.1.3-jdbc.txt");
fn parsed(text: &str) -> ProfileAnalysisResponse {
    analyze(text, &AnalysisContext { lang: Lang::En, ..Default::default() }).unwrap()
}

#[test]
fn real_413_profile_has_four_operators_and_three_true_edges() {
    let response = parsed(LIVE);
    let tree = response.execution_tree.unwrap();
    assert_eq!(tree.nodes.len(), 4); // Merged + Detail must not become eight nodes.
    assert_eq!(tree.nodes.iter().map(|n| n.children.len()).sum::<usize>(), 3);
    assert_eq!(tree.root.operator_name, "RESULT_SINK");
    let scan = tree
        .nodes
        .iter()
        .find(|n| n.operator_name == "JDBC_SCAN")
        .unwrap();
    assert_eq!(scan.metrics.operator_total_time, Some(34_680_000));
    assert_eq!(scan.rows, Some(1));
    assert!(scan.time_percentage.unwrap() > 99.0);
    assert_eq!(
        tree.nodes
            .iter()
            .filter_map(|n| n.metrics.operator_total_time)
            .sum::<u64>(),
        34_886_492
    );
    assert!(
        response.operator_diagnostics[&scan.id]
            .iter()
            .any(|d| d.rule_id == "DORIS_CONNECTOR_INIT")
    );
    let path = &response.root_cause_paths[0];
    assert_eq!(path.node_ids.len(), 4);
    assert_eq!(path.node_ids[0], scan.id);
    assert_eq!(path.node_ids.last(), Some(&tree.root.id));
    assert!(path.evidence.iter().any(|e| e.contains("25.294ms")));
}

#[test]
fn parses_statistic_units_counts_and_zero_without_inventing_values() {
    assert_eq!(duration("1min2sec3ms"), Some(62_003_000_000));
    assert_eq!(duration("12.25us"), Some(12_250));
    assert_eq!(duration("N/A"), None);
    assert_eq!(duration("0ns"), Some(0));
    assert_eq!(number("150.0M (150000000)"), Some(150_000_000));
    assert_eq!(number("1.5K"), Some(1500));
    assert_eq!(statistic("sum 1,000, avg 500, max 800, min 200", "sum"), Some("1,000"));
    assert_eq!(statistic("avg 12ms, max 50ms, min 0ns", "max"), Some("50ms"));
}

const BRANCH: &str = r#"Summary:
 - Profile ID: test-join
 - Total: 2sec
 - Task State: OK
Execution Summary:
 - Doris Version: 4.1.3
MergedProfile:
  Fragments:
    Fragment 0:
      Pipeline 0(instance_num=2):
        RESULT_SINK_OPERATOR(id=9):
          CommonCounters:
            - ExecTime: avg 1ms, max 1ms, min 1ms
        HASH_JOIN_OPERATOR(nereids_id=10)(id=3):
          CommonCounters:
            - ExecTime: avg 900ms, max 1900ms, min 1ms
            - RowsProduced: sum 200000, avg 100000, max 200000, min 0
          CustomCounters:
            - ProbeRows: sum 10000, avg 5000, max 10000, min 0
        OLAP_SCAN_OPERATOR(id=2):
          CommonCounters:
            - ExecTime: avg 10ms, max 10ms, min 10ms
            - RowsProduced: sum 10000, avg 5000, max 5000, min 5000
      Pipeline 1(instance_num=2):
        HASH_JOIN_SINK_OPERATOR(nereids_id=10)(id=3):
          CommonCounters:
            - ExecTime: avg 5ms, max 5ms, min 5ms
            - InputRows: sum 10000, avg 5000, max 5000, min 5000
        EXCHANGE_OPERATOR(id=1):
          CommonCounters:
            - ExecTime: avg 1ms, max 1ms, min 1ms
    Fragment 1:
      Pipeline 0(instance_num=2):
        DATA_STREAM_SINK_OPERATOR(dest_id=1):
          CommonCounters:
            - ExecTime: avg 1ms, max 1ms, min 1ms
        OLAP_SCAN_OPERATOR(id=2):
          CommonCounters:
            - ExecTime: avg 2ms, max 2ms, min 2ms
            - RowsProduced: sum 10000, avg 5000, max 5000, min 5000
"#;

#[test]
fn joins_keep_both_branches_exchange_links_and_duplicate_plan_ids_separate() {
    let response = parsed(BRANCH);
    let tree = response.execution_tree.unwrap();
    assert_eq!(tree.nodes.len(), 7);
    assert_eq!(tree.nodes.iter().map(|n| n.children.len()).sum::<usize>(), 6);
    let join = tree
        .nodes
        .iter()
        .find(|n| n.operator_name == "HASH_JOIN")
        .unwrap();
    assert_eq!(join.children.len(), 2);
    assert_ne!(tree.nodes[2].id, tree.nodes[6].id);
    let diagnostics = &response.operator_diagnostics[&join.id];
    assert!(
        diagnostics
            .iter()
            .any(|d| d.rule_id == "DORIS_JOIN_EXPANSION")
    );
    assert!(diagnostics.iter().any(|d| d.rule_id == "DORIS_EXEC_SKEW"));
    assert!(diagnostics.iter().any(|d| d.rule_id == "DORIS_ROW_SKEW"));
    let build = tree
        .nodes
        .iter()
        .find(|n| n.operator_name == "HASH_JOIN_SINK")
        .unwrap();
    assert!(!response.operator_diagnostics.contains_key(&build.id));
}

#[test]
fn detail_only_averages_task_times_and_sums_rows_not_profiles() {
    let fixture = r#"DetailProfile(test):
  Fragment 0:
    Pipeline 0(host=be1):
      PipelineTask(index=0):
        RESULT_SINK_OPERATOR(id=0):
          - ExecTime: 10ms
          - InputRows: 100
      PipelineTask(index=1):
        RESULT_SINK_OPERATOR(id=0):
          - ExecTime: 30ms
          - InputRows: 300
"#;
    let response = parsed(fixture);
    let tree = response.execution_tree.unwrap();
    assert_eq!(tree.nodes.len(), 1);
    assert_eq!(tree.root.metrics.operator_total_time, Some(20_000_000));
    assert_eq!(tree.root.metrics.operator_total_time_max, Some(30_000_000));
    assert_eq!(tree.root.rows, Some(400));
    assert!(!response.topology_warnings.is_empty());
}

#[test]
fn missing_time_is_unknown_and_zero_is_measured_zero() {
    let fixture = r#"MergedProfile:
 Fragment 0:
  Pipeline 0(instance_num=1):
   RESULT_SINK_OPERATOR(id=0):
    CommonCounters:
     - ExecTime: 0ns
   JDBC_SCAN_OPERATOR(id=1):
    CommonCounters:
     - RowsProduced: sum 0, avg 0, max 0, min 0
"#;
    let response = parsed(fixture);
    let tree = response.execution_tree.unwrap();
    assert_eq!(tree.nodes[0].metrics.operator_total_time, Some(0));
    assert_eq!(tree.nodes[1].metrics.operator_total_time, None);
    assert_eq!(tree.nodes[1].time_percentage, None);
    assert!(
        response
            .topology_warnings
            .iter()
            .any(|w| w.contains("ExecTime is missing"))
    );
    assert!(response.diagnostics.is_empty());
}

#[test]
fn ambiguous_exchange_does_not_fabricate_a_cross_fragment_edge() {
    let response = parsed(&BRANCH.replace("OLAP_SCAN_OPERATOR(id=2):", "EXCHANGE_OPERATOR(id=1):"));
    assert!(
        response
            .topology_warnings
            .iter()
            .any(|w| w.contains("matching receivers"))
    );
}

#[test]
fn spills_generate_measured_actionable_recommendations() {
    let fixture = BRANCH.replace(
        "- ProbeRows:",
        "- SpillWriteBytes: sum 2.5 MB, avg 1.25 MB, max 2 MB, min 0 B\n            - ProbeRows:",
    );
    let response = parsed(&fixture);
    let diagnostic = response
        .diagnostics
        .iter()
        .find(|d| d.rule_id == "DORIS_SPILL")
        .unwrap();
    assert!(diagnostic.reason.contains("2621440"));
    assert!(!diagnostic.suggestions.is_empty());
    assert!(diagnostic.parameter_suggestions.is_empty()); // Do not suggest StarRocks settings for Doris.
}

#[test]
fn repeated_or_unsupported_input_fails_clearly() {
    assert!(analyze("EXPLAIN SELECT 1", &Default::default()).is_err());
    let repeated=BRANCH.replace("        HASH_JOIN_OPERATOR", "        RESULT_SINK_OPERATOR(id=9):\n         - ExecTime: 10ms\n        HASH_JOIN_OPERATOR");
    assert!(analyze(&repeated, &Default::default()).is_err());
}

#[test]
fn automatic_engine_detection_preserves_starrocks_dispatch() {
    assert!(is_doris_profile(LIVE));
    let response = super::super::analyze_profile(LIVE).unwrap();
    assert_eq!(response.engine, "doris");
    let starrocks = include_str!("../../../../tests/fixtures/profiles/profile1.txt");
    assert!(!is_doris_profile(starrocks));
    let response = super::super::analyze_profile(starrocks).unwrap();
    assert_eq!(response.engine, "starrocks");
}

#[test]
fn json_response_is_finite_and_paths_reference_real_edges() {
    let response = parsed(BRANCH);
    serde_json::to_value(&response).unwrap();
    let tree = response.execution_tree.as_ref().unwrap();
    let index: HashMap<_, _> = tree.nodes.iter().map(|n| (&n.id, n)).collect();
    for path in response.root_cause_paths {
        for pair in path.node_ids.windows(2) {
            assert!(index[&pair[1]].children.contains(&pair[0]));
        }
    }
    let total: f64 = tree.nodes.iter().filter_map(|n| n.time_percentage).sum();
    assert!((total - 100.0).abs() < 0.001);
}

#[test]
fn per_scanner_arrays_remain_distributions_instead_of_summed_durations() {
    let response = parsed(
        "DetailProfile(test):\n Fragment 0:\n  Pipeline 0(host=be):\n   PipelineTask(index=0):\n    JDBC_SCAN_OPERATOR(id=1):\n     - ExecTime: 10ms\n     - PerScannerRunningTime: [1ms, 2ms, ]\n",
    );
    let node = &response.execution_tree.unwrap().nodes[0];
    assert_eq!(node.unique_metrics["PerScannerRunningTime"], "[1ms, 2ms, ]");
    assert_eq!(node.metrics.operator_total_time, Some(10_000_000));
}

#[test]
fn inconsistent_exchange_dependencies_cannot_create_a_cycle() {
    let response = parsed(
        "MergedProfile:\n Fragment 0:\n  Pipeline 0(instance_num=1):\n   DATA_STREAM_SINK_OPERATOR(dest_id=1):\n    - ExecTime: 1ms\n   EXCHANGE_OPERATOR(id=2):\n    - ExecTime: 1ms\n Fragment 1:\n  Pipeline 0(instance_num=1):\n   DATA_STREAM_SINK_OPERATOR(dest_id=2):\n    - ExecTime: 1ms\n   EXCHANGE_OPERATOR(id=1):\n    - ExecTime: 1ms\n",
    );
    assert!(
        response
            .topology_warnings
            .iter()
            .any(|warning| warning.contains("cycle"))
    );
    let tree = response.execution_tree.unwrap();
    assert_eq!(
        tree.nodes
            .iter()
            .map(|node| node.children.len())
            .sum::<usize>(),
        3
    );
}

#[test]
fn captured_413_join_connects_sort_materialization_and_both_join_branches_to_result() {
    let response =
        parsed(include_str!("../../../../tests/fixtures/profiles/doris-4.1.3-join-merged.txt"));
    let tree = response.execution_tree.unwrap();
    assert_eq!(tree.nodes.len(), 26);
    assert_eq!(
        tree.nodes
            .iter()
            .map(|node| node.children.len())
            .sum::<usize>(),
        25
    );
    assert!(response.topology_warnings.is_empty(), "{:?}", response.topology_warnings);
    let join = tree
        .nodes
        .iter()
        .find(|node| node.operator_name == "HASH_JOIN")
        .unwrap();
    assert_eq!(join.children.len(), 2);
    for path in response.root_cause_paths {
        assert_eq!(path.node_ids.last(), Some(&tree.root.id));
    }
}

#[test]
fn running_profile_is_provisional_with_unknown_wall_time_and_measured_operators() {
    let running = LIVE
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("- Task State:") {
                "   - Task State: RUNNING"
            } else if line.trim_start().starts_with("- Total:") {
                "   - Total: N/A"
            } else if line.trim_start().starts_with("- End Time:") {
                "   - End Time: N/A"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let response = parsed(&running);
    assert_eq!(response.summary.as_ref().unwrap().query_state, "RUNNING");
    assert_eq!(response.summary.as_ref().unwrap().is_profile_complete, Some(false));
    assert!(
        response
            .summary
            .as_ref()
            .unwrap()
            .profile_completeness_warning
            .is_some()
    );
    assert_eq!(response.summary.as_ref().unwrap().total_time_ms, None);
    assert_eq!(response.execution_tree.unwrap().nodes.len(), 4);
    let final_response = parsed(LIVE);
    assert_ne!(final_response.summary.as_ref().unwrap().is_profile_complete, Some(false));
}
