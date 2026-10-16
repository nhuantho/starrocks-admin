//! Golden snapshots of real profiles, a no-panic mutation fuzzer and a size/time budget.
//!
//! Snapshots live in `tests/fixtures/profiles/snapshots/`. Regenerate after an intended change
//! with `UPDATE_DORIS_SNAPSHOTS=1 cargo test --offline --features rust-embed/debug-embed doris`
//! and review the diff.
use super::*;
use serde_json::{Value, json};

const FIXTURES: &[(&str, &str)] = &[
    ("doris-4.1.3-jdbc", include_str!("../../../../tests/fixtures/profiles/doris-4.1.3-jdbc.txt")),
    (
        "doris-4.1.3-join-merged",
        include_str!("../../../../tests/fixtures/profiles/doris-4.1.3-join-merged.txt"),
    ),
];

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// The stable, reviewable part of a response.
fn projection(response: &ProfileAnalysisResponse) -> Value {
    let tree = response.execution_tree.as_ref().unwrap();
    json!({
        "engine": response.engine,
        "engine_version": response.engine_version,
        "confidence": response.confidence,
        "profile_level": response.profile_level,
        "score": round(response.performance_score),
        "phases": response.phase_breakdown.as_ref().map(|p| json!({
            "total_ms": p.total_ms,
            "additive": p.additive,
            "unaccounted_ms": p.unaccounted_ms,
            "segments": p.segments.iter().map(|s| json!([s.key, s.time_ms])).collect::<Vec<_>>(),
            "details": p.details.iter().map(|s| json!([s.key, s.time_ms])).collect::<Vec<_>>(),
        })),
        "root": tree.root.id,
        "nodes": tree.nodes.iter().map(|n| json!({
            "id": n.id,
            "children": n.children,
            "pct": n.time_percentage.map(round),
            "exec_ns": n.metrics.operator_total_time,
            "rows": n.rows,
            "memory": n.metrics.memory_usage,
        })).collect::<Vec<_>>(),
        "diagnostics": response.diagnostics.iter().map(|d| json!({
            "rule": d.rule_id,
            "node": d.node_path,
            "severity": d.severity,
            "confidence": d.confidence,
            "impact": d.impact.map(round),
        })).collect::<Vec<_>>(),
        "root_cause_paths": response.root_cause_paths.iter().map(|p| &p.node_ids).collect::<Vec<_>>(),
        "warnings": response.topology_warnings,
    })
}

#[test]
fn real_profiles_match_golden_snapshots() {
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/profiles/snapshots");
    let update = std::env::var("UPDATE_DORIS_SNAPSHOTS").is_ok();
    for (name, text) in FIXTURES {
        let response = analyze_with(
            text,
            &AnalysisContext { lang: Lang::En, ..Default::default() },
            &DorisOptions::default(),
        )
        .unwrap();
        let actual = serde_json::to_string_pretty(&projection(&response)).unwrap() + "\n";
        let path = dir.join(format!("{name}.json"));
        if update || !path.exists() {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &actual).unwrap();
            continue;
        }
        let expected = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            expected, actual,
            "snapshot {name} changed; rerun with UPDATE_DORIS_SNAPSHOTS=1 and review"
        );
    }
}

/// Deterministic xorshift so failures reproduce without a fuzzing crate.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn mutate(text: &str, rng: &mut Rng) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for _ in 0..1 + rng.below(4) {
        if lines.is_empty() {
            break;
        }
        let i = rng.below(lines.len());
        match rng.below(8) {
            0 => {
                lines.truncate(i);
            },
            1 => {
                lines.remove(i);
            },
            2 => {
                let line = lines[i].clone();
                lines.insert(i, line);
            },
            3 => {
                let j = rng.below(lines.len());
                lines.swap(i, j);
            },
            4 => {
                // Cut a line at a char boundary.
                let line = &lines[i];
                let cut = line
                    .char_indices()
                    .nth(rng.below(line.chars().count() + 1))
                    .map_or(line.len(), |(b, _)| b);
                lines[i] = line[..cut].to_string();
            },
            5 => {
                let garbage = [
                    "avg , max , min",
                    "sum -5, avg x",
                    "[",
                    "{\"phase1\":",
                    ": : :",
                    "N/A",
                    "99999999999999999999999ms",
                    "\u{0}",
                    "Fragment 99:",
                    "Pipeline 7(instance_num=0):",
                    "HASH_JOIN_SINK_OPERATOR(id=3):",
                    "  - ExecTime: avg 1hour, max 0ns, min 9min",
                ];
                lines.insert(i, garbage[rng.below(garbage.len())].to_string());
            },
            6 => {
                lines[i] = lines[i].replace(['(', ')'], "");
            },
            _ => {
                lines[i] = lines[i].trim_start().to_string();
            },
        }
    }
    lines.join("\n")
}

#[test]
fn parser_never_panics_on_truncated_or_malformed_profiles() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let context = AnalysisContext { lang: Lang::En, ..Default::default() };
    let options = DorisOptions { experimental_rules: true };
    for (name, text) in FIXTURES {
        // Every prefix at a line boundary.
        let mut offset = 0;
        for line in text.split_inclusive('\n') {
            offset += line.len();
            let _ = analyze_with(&text[..offset], &context, &options);
        }
        for iteration in 0..400 {
            let mutated = mutate(text, &mut rng);
            let result = std::panic::catch_unwind(|| analyze_with(&mutated, &context, &options));
            assert!(result.is_ok(), "panic on {name} mutation {iteration}:\n{mutated}");
            if let Ok(Ok(response)) = result {
                // Output must stay serializable and finite.
                serde_json::to_string(&response).unwrap();
                let tree = response.execution_tree.as_ref().unwrap();
                assert!(
                    tree.nodes
                        .iter()
                        .filter_map(|n| n.time_percentage)
                        .all(f64::is_finite)
                );
                assert!(
                    response
                        .diagnostics
                        .iter()
                        .filter_map(|d| d.impact)
                        .all(f64::is_finite)
                );
                assert!(
                    response
                        .phase_breakdown
                        .iter()
                        .flat_map(|p| &p.segments)
                        .all(|s| s.time_ms.is_finite())
                );
                assert!((0.0..=100.0).contains(&response.performance_score));
            }
        }
    }
}

#[test]
fn sixteen_mib_profile_stays_within_time_budget() {
    let (_, text) = FIXTURES[0];
    let (head, detail) = text.split_once("DetailProfile").unwrap();
    let detail = detail.split("Appendix:").next().unwrap();
    // Repeat the detail tasks until just under the limit; they merge into the same operators.
    let mut profile = format!("{head}DetailProfile");
    while profile.len() + detail.len() < MAX_PROFILE_BYTES {
        profile.push_str(detail);
    }
    let started = std::time::Instant::now();
    let response =
        analyze_with(&profile, &AnalysisContext::default(), &DorisOptions::default()).unwrap();
    let elapsed = started.elapsed();
    assert_eq!(response.execution_tree.unwrap().nodes.len(), 4);
    // Budget documented in docs/profile/doris-runtime-diagnostics.md (debug build; release is much faster).
    assert!(elapsed < std::time::Duration::from_secs(20), "16 MiB analysis took {elapsed:?}");
    eprintln!("16 MiB Doris profile analyzed in {elapsed:?}");
    let mut too_big = profile;
    too_big.push_str(&"x".repeat(MAX_PROFILE_BYTES));
    assert!(analyze_with(&too_big, &AnalysisContext::default(), &DorisOptions::default()).is_err());
}
