# Runtime profile diagnostics

The Query Profile viewer parses the retained profile and renders its execution DAG, measured query phases, operator counters and diagnostics. It does not rerun SQL or execute recommendations. StarRocks continues to use its existing analyzer.

## Doris version support and evidence

| Doris release | Parser / diagnostics status | Profile collection notes |
| --- | --- | --- |
| 4.1.3 | Captured fixtures cover JDBC scans, joins, exchanges, aggregations, sorting and query summary. This is the validated baseline. | Use `enable_profile=true`; on 4.x, `profile_level=2` adds detail counters. |
| 4.0+ | Profile-level interpretation and common 4.x counters are supported by documented names, but need fixtures from each target release. | `profile_level` has the documented 1–3 meaning from 4.0. |
| 3.x | Compatibility aliases use the 3.x docs where cited; parser output still needs a real 3.x fixture before calling this release validated. | `auto_profile_threshold_ms` is documented from 3.0. Legacy profile/API layout may differ. |
| 2.1 | Compatibility aliases use the 2.1 docs where cited; parser output still needs a real 2.1 fixture before calling this release validated. | `enable_profile` is documented. Do not assume 4.x `profile_level` semantics. |

The committed real fixtures are `doris-4.1.3-jdbc.txt`, `doris-4.1.3-join-merged.txt` and `doris-4.1.3-failed-summary.txt`. None contains a native `OLAP_SCAN_OPERATOR`, a real spill profile, or a 2.1/3.x profile. See [the counter catalog](doris-counter-catalog.md) for every observed counter and the evidence source for documented aliases. Unverified OLAP and spill rules are disabled by default; set `STELLAR_DORIS_EXPERIMENTAL_RULES=true` only in a test deployment when evaluating them.

Official references: [Doris 4.x Query Profile](https://doris.apache.org/docs/4.x/query-acceleration/query-profile/), [Doris 3.x Analysis Tools](https://doris.apache.org/docs/3.x/query-acceleration/performance-tuning-overview/analysis-tools/), [Doris 3.x Profile Action API](https://doris.apache.org/docs/3.x/admin-manual/open-api/fe-http/profile-action/), [Doris 2.1 Analysis Tools](https://doris.apache.org/docs/2.1/query-acceleration/performance-tuning-overview/analysis-tools/), and [Doris 2.1 Query Memory Analysis](https://doris.apache.org/docs/2.1/admin-manual/trouble-shooting/memory-management/memory-analysis/query-memory-analysis/).

## Parsing and timing semantics

- Prefer `MergedProfile` operator `avg`, `max`, `min` and `sum` statistics. Supplement missing counters from `DetailProfile`; never add detail task times to merged times. Detail-only profiles are reconstructed from available tasks and have lower confidence.
- Node identity includes fragment, pipeline, operator, plan id and destination. Edges use observed exchange destinations and sink/source pairing. Ambiguous branches remain disconnected and are reported in topology warnings.
- Operator share is `avg ExecTime / sum(operator avg ExecTime)`. This is a share of measured operator work, not query wall time. Dependency waits and other wait counters are separate and non-additive.
- `N/A`, absent or malformed values remain unknown; measured zero remains zero. A lower-level profile can omit counters; on Doris 4.x the viewer recommends `profile_level=2` when detail evidence is missing.
- The displayed profile level comes from the captured query's `ChangedSessionVariables`, never the console session's current value. A failure can have a usable summary without any operators: the viewer retains summary diagnostics and explicitly omits the graph. An unknown failure message produces a warning without an inferred cause or failing phase.
- Highlighted downstream paths are observed dependencies around diagnostic candidates, not a proven wall-clock critical path.

## Diagnostics

Every finding includes the evidence counter names and observed values. Confidence is high for merged statistics, medium for reconstructed detail-only aggregates, and reduced for running profiles. Findings are ranked by severity and impact, aggregated by rule, and the score is monotonic with added or more severe findings; advisory findings do not reduce it. For each non-advisory rule group the weight is 1 for Info, 6 for Warning and 20 for Critical. Its cost is `weight × (0.5 + 0.5 × max impact) + weight × 0.1 × (min(instance count, 5) − 1)`; unknown impact defaults to 0.5. The score is `max(0, 100 − sum(costs))`.

| Rule | Trigger / evidence | Severity | Release/evidence status |
| --- | --- | --- | --- |
| `DORIS_TIME_SHARE` | Operator is at least 30% of summed average `ExecTime` | Info | Fixture-backed; advisory |
| `DORIS_EXEC_SKEW` | Multiple instances, average `ExecTime` ≥1 ms, `max/avg` ≥2 | Warning; Critical at `max/avg` ≥10 and operator share ≥50% | Requires task-level detail |
| `DORIS_ROW_SKEW` | At least 1,000 output rows and `RowsProduced max/avg` ≥2 | Warning | Requires per-instance row detail |
| `DORIS_CONNECTOR_INIT` | `InitConnectorTime` ≥5 ms and ≥50% of `ExecTime` | Info | JDBC fixture |
| `DORIS_SCAN_WAIT` | A scan dependency-wait counter ≥5 ms and ≥50% of `ExecTime` | Info | Wait remains separate from execution time |
| `DORIS_SCAN_VOLUME` | At least 100,000 `RowsRead` or documented `RawRowsRead`; output/read <1% | Warning | JDBC `RowsRead` observed; OLAP `RawRowsRead` only documented for 2.1/3.x |
| `DORIS_RUNTIME_FILTER_WAIT` | `WaitForRuntimeFilter max` ≥100 ms and ≥20% of query total | Info | JDBC fixture / 2.1 runtime-filter docs |
| `DORIS_SCANNER_QUEUE` | `ScannerWorkerWaitTime max` ≥100 ms and ≥20% of query total | Info | JDBC fixture |
| `DORIS_JOIN_EXPANSION` | At least 10,000 output rows and output/probe rows ≥10 | Warning; Critical at ≥100 | Probe/build counters from merged fixture |
| `DORIS_JOIN_BUILD_LARGER` | Build rows exceed probe rows and build rows ≥1,000,000 | Warning | Merged join fixture |
| `DORIS_BROADCAST_LARGE` | Broadcast build side has at least 10,000,000 rows or 1 GiB peak memory | Warning | Merged join fixture |
| `DORIS_JOIN_BUILD_WAIT` | Probe dependency wait ≥100 ms and ≥50% of query total | Info | Merged join fixture |
| `DORIS_STREAMING_AGG_INEFFECTIVE` | Streaming aggregation reduces at most 10% of at least 1,000,000 input rows | Info | Fixture-backed operator/counters |
| `DORIS_AGG_HIGH_CARDINALITY` | `HashTableSize` ≥10 million | Info | Counter is present in fixtures |
| `DORIS_LARGE_SORT` | Sort input ≥10 million rows and no plan limit | Warning | Fixture-backed sort/input counters |
| `DORIS_SINK_BACKPRESSURE` | Exchange sender wait ≥100 ms and ≥30% of query total | Info | Fixture-backed wait counter |
| `DORIS_EXCHANGE_VOLUME` | Exchange data volume ≥10 GiB | Info | Only emitted when a documented/observed byte counter exists |
| `DORIS_OPERATOR_MEMORY` | Peak operator memory ≥4 GiB without a known limit; with known limit, ≥50% | Warning; Critical at ≥80% of known limit | Uses `MemoryUsagePeak` / documented legacy alias |
| `DORIS_PLAN_TIME` | Plan time ≥100 ms and ≥30% of total | Warning | 4.1.3 summary |
| `DORIS_EXTERNAL_METADATA` | External metadata timers ≥50 ms and ≥50% of plan time | Info | 4.1.3 Nereids summary |
| `DORIS_SCHEDULE_TIME` | Schedule time ≥100 ms and ≥30% of total | Warning | 4.1.3 summary |
| `DORIS_SLOW_BE_RPC` | Backend fragment RPC is ≥3× the peer median and ≥50 ms | Warning | Requires per-BE schedule JSON |
| `DORIS_BE_RPC_QUEUE` | RPC work queue ≥50 ms | Warning | Requires per-BE schedule JSON |
| `DORIS_RESULT_WRITE` | Result write ≥100 ms and ≥30% of total | Info | 4.1.3 summary |
| `DORIS_QUERY_FAILED` | A non-success task state with a recognized failure string | Critical for known causes; otherwise Warning | Unknown messages are shown without guessed cause |
| `DORIS_ESTIMATE_ERROR` | Estimated/actual row ratio exceeds the analyzer threshold | Warning | PhysicalPlan appendix; operator attachment only when mapping is unambiguous |
| `DORIS_PROFILE_LEVEL` | No detail counters with Doris 4.x profile level below 2 or unknown | Info | Advisory; suggests session-level detail |
| `DORIS_SPILL` | Nonzero counter whose key contains both `spill` and `bytes` | Warning | Experimental; no real profile or documented counter yet |
| `DORIS_OLAP_INDEX_FILTER` | Documented OLAP read rows ≥1 million and output/read <1% | Warning | Experimental; no native OLAP fixture |
| `DORIS_OLAP_IO` | Documented OLAP `IOTimer` ≥10 ms and ≥50% of `ExecTime` | Info | Experimental; no native OLAP fixture |
| `DORIS_OLAP_SEGMENTS` | At least 1,000 segments and ≥50 segments/tablet | Warning | Experimental; no native OLAP fixture |
| `DORIS_OLAP_PAGE_CACHE` | At least 10,000 pages and cache hit ratio <10% | Info | Experimental; no native OLAP fixture |
| `DORIS_NESTED_LOOP_JOIN` | Candidate operator name contains `NESTED_LOOP_JOIN` | Warning | Experimental; operator name unobserved |
| `DORIS_SHUFFLE_SMALL_BUILD` | Partitioned join, build rows ≤10,000 and probe rows ≥10,000,000 | Info | Experimental; shuffle profile evidence missing |
| `DORIS_COUNT_DISTINCT` | Distinct aggregation with hash table ≥256 MiB | Info | Experimental; output marker unobserved |

For experimental and advisory rules, absence of a finding does not imply the condition is absent; required counters may not be collected. Thresholds are heuristic signals for review, not universal Doris performance limits.

## Variables and recommendations

The backend reads a fixed allowlist with `SHOW VARIABLES` and degrades gracefully if a variable is unavailable. Recommendations are copyable text only. A setting recommendation is omitted when its current value is unknown. The allowlist includes profile, pipeline, memory, timeout, spill, runtime-filter, query-cache and Nereids variables, but Doris version availability is determined by what the connected server returns. Doris 4.x `profile_level` and spill advice is version-gated. SQL snippets are never executed by Stellar.

## Retention, snapshots and comparison

Doris retains profiles on the executing FE and can evict them. Verify `enable_profile` was active in the query's session and connect to the FE that ran it. The UI surfaces current profile-related variables/FE config and gives recovery guidance when a profile is missing.

Durable snapshots are **opt-in** with `[profile_snapshots] enabled = true`. They are scoped by cluster and organization, capped by age, count, byte budget and maximum profile size, and periodically purged. Opening diagnostics archives a terminal profile; running or incomplete profiles are not stored as final snapshots. Archived records are labelled in the UI. Profile comparison reports phase/operator deltas and new/resolved diagnostics. Recurring slow queries are grouped by normalized SQL fingerprint from retained and archived profiles. Markdown/HTML reports can redact SQL literals.

## Frontend behavior and validation

Diagnostics sort by severity and impact. Selecting a finding focuses its operator and highlights the evidence counters. The node list and graph support keyboard navigation; severity uses a text label and symbol as well as color. The viewer keeps the selected operator and graph viewport while polling. Topology warnings are available in a collapsible details area.

Run from `backend/`: `cargo test --offline --features rust-embed/debug-embed profile_analyzer`. The fixtures and snapshot tests preserve StarRocks coverage, parser mutation coverage and a 16 MiB analysis time budget. Frontend checks: `npm run lint`, `npm run build`, and `npm test -- --watch=false` (Karma requires ChromeHeadless).

For dbt/Airflow, set profiling in the same model connection, for example:

```jinja
{{ config(pre_hook=["SET enable_profile = true", "SET profile_level = 2"]) }}
```

An administrator may instead use `SET GLOBAL enable_profile = true` for new sessions. This is cluster-wide advice; verify the active value in the model's actual session. Stellar does not change these settings.


### Doris history metadata and dbt LOAD profiles

`/rest/v2/manager/query/query_info` exposes QUERY records only and omits Default Catalog. Stellar supplements it with read-only, five-second-bounded `SHOW QUERY PROFILE LIMIT 2000` and `SHOW LOAD PROFILE '/'` on the pooled FE. Records merge by profile id without overwriting a live RUNNING state. The Type column shows the engine-provided QUERY/LOAD type. The SQL metadata fallback is best-effort on older Doris releases.

Catalog means the original session's **Default Catalog**, not every catalog referenced in its SQL. Missing catalogs on the visible page are resolved from the authorized raw profile Summary through a metadata-only response (20 rows per page, four simultaneous requests, cancelled on page changes). Successful metadata is cached for the active cluster. Missing/expired profiles remain unknown; manual refresh retries failures. This avoids fetching all retained profile bodies on every list refresh. Catalog filtering covers metadata already loaded into the client, not an exhaustive cross-FE catalog index.

The SQL supplement reflects the FE used by the pooled connection. QUERY history and authorized profile-text lookup use the all-FE HTTP endpoint; unattended cross-FE LOAD discovery is not implemented. A load balancer can route dbt and Stellar to different FEs. Snapshot archiving is still driven by opening diagnostics, and is disabled by default. Enabling Doris profiling alone does not enable Stellar archival or recover an evicted profile.

Live investigation on 2026-10-16: the OCR task succeeded at 00:12:24–00:12:26 UTC. Its retained `information_schema.columns` query had the expected Iceberg default catalog and service user. The main write profile was not located in the inspected metadata, so this does not prove whether the model connection generated a profile. The console returned enable_profile=true, profile_level=2, auto_profile_threshold_ms=-1; these values do not establish the historical model connection's values. The connected FE reported 500 in-memory / 500 persisted profiles and an asynchronous collection expiry of five seconds. No cluster settings or model data were changed.

The live list also exposed repeated failed StarRocks audit-table probes from Stellar's metrics collector. Doris now skips that StarRocks-only percentile path and uses the existing metrics fallback. This removes one avoidable source of profile churn; it does not prove that eviction caused the missing OCR write profile.
