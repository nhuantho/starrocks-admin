# Doris diagnostics acceptance audit

Date: 2026-10-16 (Asia/Ho_Chi_Minh). Target: current working tree, local UI at `http://localhost:18080`, connected to the configured staging Doris 4.1.3 cluster. Cluster operations were read-only; no recommendations or `SET` statements were executed.

## Verdict

There is a real improvement beyond EXPLAIN text: the live UI renders measured operators, query phases, evidence-linked findings and counter distributions, and compares two retained profiles. **The full original six-phase prompt is not yet satisfied.** Missing native OLAP/spill/older-version evidence and the limitations below prevent claiming StarRocks diagnostic parity or full Doris 2.1/3.x support.

The local service initially served an old embedded frontend. The current production frontend was rebuilt and embedded into the backend before the browser checks.

## Browser checks

| Scenario | Observed result |
| --- | --- |
| Open a retained dbt profile | Doris 4.1.3 version, final state, confidence, two-node SCHEMA_SCAN/RESULT_SINK DAG and measured phase breakdown displayed. |
| Focus a finding | Clicking its operator link selected SCHEMA_SCAN and highlighted the ExecTime evidence row. Min/avg/max values matched the displayed raw profile. |
| Zoom and refresh | `scale(1.1)`, selected operator and highlighted evidence survived a diagnostics refresh. |
| Keyboard graph navigation | ArrowDown moved DOM focus from RESULT_SINK to SCHEMA_SCAN. |
| Compare actual retained queries | Same SQL fingerprint; 7 ms baseline vs 9 ms target; phase/operator deltas and new/resolved findings rendered. |
| Recurring statements | UI grouped retained statements by fingerprint and showed counts/average/maximum/total duration. This is retained-profile history, not audit-table coverage. |
| Live query | `SELECT SLEEP(30) AS stellar_runtime_qa LIMIT 1`: observed RUNNING, low confidence and provisional counters, followed by FINISHED, high confidence, approximately 30 s UNION execution, final snapshot and disabled auto-polling control. |
| Original profile level unknown | Header omitted the level; the console session's current `profile_level=2` was not falsely attributed to the historical query. |
| Failure with no recorded operators | Initially failed analysis despite a usable summary. Captured and anonymized as `doris-4.1.3-failed-summary.txt`; after the fix the live UI displayed FAILED, medium confidence, measured phases, workload group and a query-level Warning without guessing a cause or inventing a graph. Polling stopped. |
| Missing/detail guidance | UI retrieved current profile variables and FE retention settings, and labelled session/global SQL snippets as advice only. |
| Snapshot archive | UI explicitly reported archiving off. Save/reopen/purge have backend tests; durable archive was not enabled on the user's local database merely for this audit. |
| Export | Backend Markdown redaction and scriptless HTML tests passed. Browser export was clicked without console errors, but the downloaded file could not be independently inspected through the supported browser surface; do not count this as an end-to-end export pass. |
| Catalog visibility | UI listed internal plus 12 external catalogs and loaded internal/external database trees. This is an observation for the configured account, not proof of every role/catalog combination. |
| SQL result rendering after audit fixes | `SELECT 1 AS stellar_result_qa LIMIT 1` completed in 72 ms on the tested run. Run became available after editing, the loading state cleared and the result table visibly contained one cell with value `1`. |

## Defects fixed during this audit

- Archive eligibility previously treated any non-running string, including UNKNOWN, as terminal and ignored an explicit incomplete marker. It now requires a recognized terminal state and rejects incomplete profiles, with a regression test.
- Historical profile level previously fell back to the console's live session setting. The response now uses the captured ChangedSessionVariables only; captured values take precedence in detail guidance.
- Failed profiles without runtime operators were rejected. A real failure fixture now verifies summary-only analysis, unknown-cause warnings, measured phases and raw-profile access. The frontend explains the absent graph and stops polling for confirmed terminal failures.
- Doris summary percentages could use a fabricated 1 ms denominator while query total was unknown. They now show only the measured duration until a positive total is available.
- Workload group was parsed but not surfaced. It is now displayed in the overview.
- SQL editor/database/table loading callbacks and the CodeMirror change listener did not mark an OnPush view for checking, leaving stale Loading/Running UI or a disabled Run button after typing. The observed callbacks now mark the view; result datasource changes are also observed so asynchronous grid rows render.
- Session query debug logging included SQL text. The completion log now records only row count and elapsed time.

## Remaining acceptance gaps

| Prompt phase | Status and remaining evidence/work |
| --- | --- |
| 1 — parser foundation | Typed metrics, mutation tests, snapshots and anonymizer exist. Still no native OLAP scan/spill/2.1/3.x fixture. Counter aliases are centralized but `raw_names` is not keyed by release; the parser expects `_OPERATOR` headers, so older-format compatibility is not established. |
| 2 — summary diagnostics | Phase breakdown, Nereids planning candidates, backend RPC, workload group and summary-only failure now work. Execute and fetch remain combined where the profile cannot establish separate non-overlapping phases; Fetch Result Time must not automatically be blamed on the client (the SLEEP test spends approximately 30 s there). |
| 3 — operator diagnostics | Fixture-backed JDBC/join/aggregation/sort/exchange rules and monotonic scoring exist. Native scan/index/pruning/cache and spill remain experimental. Generic dependency-wait coverage is narrower than the prompt. Paths deduplicate identical node lists, not all overlapping findings. Most trigger tests use synthetic values, not captured bottleneck scenarios. |
| 4 — recommendations | Fixed allowlist and known-value gating exist. Memory/timeout/profile-level advice exists; pipeline/RF/cache advice is not comprehensive. Live console variables do not necessarily describe the original Airflow/dbt session; relative memory-limit conclusions still need original-session context and must be reviewed accordingly. |
| 5 — retention/history | Opt-in local archive and comparison exist. Capture is driven by opening/polling diagnostics, not unattended Airflow collection. Recurring view uses retained profiles/snapshots instead of audit data. `size_bytes` budgets raw profile bytes, not the additional analysis JSON, and caps are enforced by the periodic purge. Operator comparison uses exact runtime node ids, so changed plans have unmatched nodes rather than semantic matching. |
| 6 — UI | Core interactions are verified above. Layout/viewport/evidence work on the tested desktop. Full responsive/accessibility review, live Chinese UI, cluster-switch cancellation under load and archive reopen remain unverified. Some older UI labels remain hardcoded. |

## Verification

- Backend analyzer: `cargo test --offline --features rust-embed/debug-embed profile_analyzer` — 158 passed, 0 failed, including unchanged StarRocks fixtures and the new captured failure.
- Archive state regression: same test command with filter `snapshot_state_tests` — 1 passed.
- `cargo clippy --offline --features rust-embed/debug-embed -- -D warnings` — passed.
- Frontend `npm run lint`, `npm run i18n:check`, production build and the profile Karma subset passed. The subset contains 10 tests (four component smoke tests and six utilities).
- Whole-repository baseline limitations remain: 14 auth/org-middleware backend tests and three unrelated frontend auth tests failed in the preceding full-suite run. Repository-wide `cargo fmt --check` reports formatting in untouched legacy files; touched Rust files are checked separately.
- The 16 MiB timing test has a 20 s debug-build budget and repeats detail task samples merging into four operators. It does not establish a worst-case budget for 4,096 distinct operators.

Screenshots from the live audit are temporary local artifacts under `/tmp/stellar-qa/`, not committed staging data.


## Follow-up: catalog and Airflow OCR retention (2026-10-16)

- Catalog now resolves from original profile Summary; UI verification showed the OCR metadata query's `gsm_multimodal_vision_staging_ap_southeast_1` catalog and `multimodal_vision_service_user`. Two pages of 20 recent profiles each displayed `internal` correctly after lazy resolution. No browser console errors were observed.
- QUERY-only HTTP history is supplemented with connected-FE SQL QUERY/LOAD metadata. A new engine-provided Type column is filterable. LOAD discovery on every FE is still incomplete: the tested pooled FE returned no LOAD records, while separate direct metadata reads found LOAD records on another FE. Do not claim this restores the OCR write profile.
- The task log confirms a successful incremental model with 101 rows affected in 1.78 s. Only its 14 ms column-metadata QUERY was found; the main write profile remains unlocated. Current console settings cannot prove the original model session's values.
- Repeated failed StarRocks audit latency probes were visible in the live history. The Doris metrics collector now skips that StarRocks-only path, retaining the existing Prometheus fallback.
- Persistent archival remains opt-in and driven by diagnostics access. There is no unattended Airflow/dbt collector. Profile generation, FE retention, listing coverage and Stellar archival are separate checks.
- Validation: eight query-profile service tests, 158 analyzer tests, four MySQL/catalog tests, one profile-permission test and ten frontend profile specs passed. Strict backend clippy, frontend lint/i18n check and production build passed. New parser/merge tests cover captured catalog extraction, unknown catalog, LOAD metadata, duplicate ids and preserving live state.
- UI screenshot: `/tmp/stellar-qa/profiles-ocr-catalog.jpg`.

Post-restart check at 00:46:38 UTC found 881 retained failed audit probes, but the newest was 00:45:20 UTC, before the final restart; no new probes appeared over more than two prior 30-second collection intervals. Older failed profiles remain retained until Doris evicts them.
