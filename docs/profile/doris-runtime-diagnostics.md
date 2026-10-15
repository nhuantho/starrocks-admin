# Runtime profile diagnostics

Open **Profiles → Query diagnostics** (magnifier action) to inspect a retained runtime profile. SQL / EXPLAIN is a separate action. The runtime profile tab also has **Open query diagnostics**.

Both StarRocks profiles and Doris 4.x pipeline profiles use the interactive viewer: zoom, fit, pan, horizontal/vertical layout, operator details, time ranking, diagnostics and downstream dependency paths. Existing StarRocks rules remain in use. Doris has its own counter-aware parser and rules; StarRocks parameter recommendations are not applied to Doris.

For Doris, run `SET enable_profile = true;` in the same session before the query. Profiles must still be retained and visible to the connection account. The viewer analyzes the original collected profile; it does not rerun the SQL or apply its recommendations.

## Doris parsing and topology

- Prefer `MergedProfile` operator `avg` / `max` / `min` timing and row/memory statistics.
- Supplement missing phase counters from `DetailProfile`, without adding its execution times to the merged times.
- When only detailed tasks are available, group operators by fragment, pipeline, operator kind, plan ID and exchange destination. Average task execution times and sum row counts.
- Keep the profile's sink-to-source operator order inside each pipeline. Connect stream sinks to an unambiguous exchange destination; pair build/materializing sinks with their source in the same fragment using operator family and plan ID.
- Leave unmatched branches disconnected and report warnings. Do not fabricate missing edges or allow cycles.

Node identity includes fragment and pipeline, so join build/probe nodes and reused plan IDs cannot overwrite one another's counters or diagnostics.

## Timing and evidence

Doris node percentage is **average ExecTime / sum of average operator ExecTime**. It measures concentration of operator work, not percentage of query wall time. Parallel operators and dependency waits cannot simply be added into wall time. Runtime wait counters remain separate. Missing counters are unknown, while measured zero remains zero.

Highlighted downstream paths are observed dependencies associated with diagnostic candidates. They are not a proven wall-clock critical path or a claim that every dominant operator is slow.

## Doris diagnostic rules

| Rule | Evidence / trigger |
| --- | --- |
| Dominant execution | At least 30% of measured operator execution time; informational |
| Execution skew | Multiple instances, avg ExecTime at least 1 ms, max/avg at least 2 |
| Row skew | Multiple instances, at least 1,000 output rows, max/avg at least 2 |
| Connector initialization | Scan InitConnectorTime at least 5 ms and at least half of ExecTime |
| Scanner dependency wait | Scan dependency wait at least 5 ms and at least half of ExecTime; informational |
| Scan volume | At least 100,000 rows read with output/read below 1%; verify whether expected |
| Join amplification | At least 10,000 output rows and output/probe at least 10; verify join intent |
| Spill | Nonzero measured spill-byte counter |

Recommendations include the actual counter evidence. They ask the operator to verify connector reuse, task distribution, pruning, join cardinality or memory constraints; they do not automatically change server settings.

## Validation

From `backend/`, run `cargo test --offline --features rust-embed/debug-embed profile_analyzer`. Tests include an anonymized captured Doris 4.1.3 JDBC profile, multi-branch joins and exchanges, detail-only aggregation, missing counters, ambiguous edges, spill evidence and StarRocks fixture regression coverage.

## Live diagnostics

Opening Diagnostics polls the original query status and runtime profile every five seconds, independently of list auto-refresh. Running/unknown snapshots are provisional. The viewer preserves selected operators and the viewport and keeps the last successful graph if collection fails. Pause/resume and manual refresh are available. Closing the dialog, switching clusters, or leaving the page cancels polling and in-flight subscriptions.

When a terminal state and a terminal complete profile agree, automatic polling stops and the dialog keeps the final snapshot. If the query disappears without a confirmed final profile, retry for up to six cycles; show UNKNOWN or the profile-reported state, never infer success from disappearance. Failed/incomplete collection can be retried manually. This is an in-dialog snapshot, not a durable profile archive; reopening still depends on Doris retention.

Doris 4.1.3 can return a running MergedProfile via profile/text when profiling was enabled for that query. Running counters and recommendations are provisional; an execution time of N/A must not be presented as zero.

For dbt models, configure a pre-hook in the dbt project/model so it applies in the model's own connection:

```jinja
{{ config(pre_hook=["SET enable_profile = true", "SET profile_level = 2"]) }}
```

Alternatively, administrators can enable `SET GLOBAL enable_profile=true` for new connections cluster-wide. Existing sessions may retain their previous value. Verify with `SHOW VARIABLES LIKE 'enable_profile'` in the actual execution session. Airflow/dbt deployment sources are outside this repository.
