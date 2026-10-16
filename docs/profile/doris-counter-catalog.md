# Doris profile counter catalog

Status: **observed in Doris 4.1.3** (`doris-4.1.3-rc02`). Every name below appears verbatim in a committed fixture:

- `J` = `backend/tests/fixtures/profiles/doris-4.1.3-jdbc.txt` (MergedProfile + DetailProfile + Appendix, 1 BE, 4 operators)
- `M` = `backend/tests/fixtures/profiles/doris-4.1.3-join-merged.txt` (MergedProfile only, no DetailProfile/Appendix, 13 operators)
- `F` = `backend/tests/fixtures/profiles/doris-4.1.3-failed-summary.txt` (captured failure: Summary/Execution Summary only, zero runtime operators; `Task State: ERR`, `System Message: N/A`). Its summary keys are a subset of those catalogued below, including measured zero and unknown `N/A`/`-1ms` timings. The failing phase and cause are not established by this fixture.

The two operator profiles scan an external JDBC catalog. **No native `OLAP_SCAN_OPERATOR`, spill, or 2.1/3.x profile has been captured yet**, so counters for those cases are not listed here and rules must not depend on them until a fixture is added. A failure summary cannot establish the error's cause when Doris did not record a message.

Diagnostic rules may read only counters listed here (or counters cited from official Doris docs in a code comment). Extend this file whenever a fixture is added.

## Value formats

| Format | Example | Where |
| --- | --- | --- |
| Merged time | `avg 31.228us, max 31.228us, min 31.228us` | MergedProfile (no `sum` for times) |
| Merged count | `sum 9, avg 1, max 2, min 1` | MergedProfile; `avg` is integer-truncated (`avg 0` with `sum 1`) |
| Merged bytes | `sum 4.13 KB, avg 528.00 B, max 4.13 KB, min 0.00` | Zero is printed as `0.00 ` without a unit |
| Merged rate | `sum 0.0 /sec, ...` | `OverallThroughput` |
| Empty merged string | `sum , avg , max , min` | `RuntimeFilterInfo` |
| Detail scalar | `32.995ms`, `5.00 B`, `0.00`, `1` | DetailProfile |
| Per-scanner array | `[32.813ms, ]` | `PerScanner*` (distribution, not additive) |
| Unknown | `N/A` | Execution Summary; must stay unknown, never 0 |
| JSON | `{"phase1":{"host: port":{...}}}` | `Schedule Time Of BE` |

Pitfalls observed: `GetBlockFailedTime` is a **count** despite its name (`sum 1, avg 0, ...`); `MaxFindRecvrTime(NS)` is a bare integer; `ExecteReadTime` is spelled that way by Doris.

## Summary (J, M)

`Profile ID`, `Task Type`, `Start Time`, `End Time`, `Total`, `Task State` (`OK` observed), `User`, `Default Catalog`, `Default Db`, `Sql Statement` (may span lines), `Distributed Plan`.

## Execution Summary (J, M)

Indentation shows nesting.

```
Workload Group
Parse SQL Time
Plan Time
  Garbage Collect During Plan Time
  Nereids Preload External Metadata Time
  Nereids Lock Table Time
  Nereids Analysis Time
  Nereids Rewrite Time
    Nereids Fold Const By BE Time
  Nereids Collect Table Partition Time
Nereids Pre Rewrite By Mv Time          (printed at top-level indent in 4.1.3; logically under Plan Time)
  Nereids Optimize Time
  Nereids Translate Time
    Init Scan Node Time
    Finalize Scan Node Time
      Get Splits Time
      Get Partitions Time
      Get Partition Files Time
      Sink Set Partition Values Time
    Create Scan Range Time
      Iceberg Scan Metrics
      Paimon Scan Metrics
  Nereids Distribute Time
Get Meta Version Time
  Get Partition Version Time
  Get Partition Version Count (hasData)
  Get Partition Version Count
  Get Table Version Time
  Get Table Version Count
Schedule Time
  Fragment Assign Time
  Fragment Serialize Time
  Fragment RPC Phase1 Time
  Fragment RPC Phase2 Time
  Fragment Compressed Size
  Fragment RPC Count
Schedule Time Of BE        JSON: phase1|phase2 -> "<host>: <port>" -> {RPC Work Time, RPC Latency From FE To BE, RPC Work Queue Time, RPC Latency From BE To FE}
Wait and Fetch Result Time
Fetch Result Time
Write Result Time
Doris Version
Is Nereids
Is Cached
Total Instances Num
Instances Num Per BE       "<host>:<port>:<n>"
Parallel Fragment Exec Instance Num
Trace ID
Transaction Commit Time
System Message
Executed By Frontend
Splits Assignment Weight
```

## ChangedSessionVariables (J, M)

JSON array of `{VarName, CurrentValue, DefaultValue}` for variables that differ from default. Observed: `enable_profile`, `max_allowed_packet`. `profile_level` appears here only when it was changed from its default.

## Appendix (J only)

`PhysicalPlan:` Nereids physical plan text. Nodes carry `stats=<estimated rows>` and, for scans, `actualRows=<n>`, e.g. `PhysicalJdbcScan ( ..., stats=1 actualRows=1 )`. Bracketed numbers (`PhysicalLimit[98]@3`) are Nereids ids, **not** the runtime `id=` plan node ids; the mapping is not yet established.

## Fragment / pipeline level

- MergedProfile: `Fragment <n>:` / `Pipeline <n>(instance_num=<k>):`
- DetailProfile (J): `FragmentLevelProfile:(host=...)` with `BuildPipelinesTime`, `BuildTasksTime`, `InitContextTime`, `PlanLocalLocalExchangerTime`, `PrepareAllPipelinesTime`, `PrepareTime`; `Pipeline <n>(host=TNetworkAddress(...))` -> `PipelineTask(index=<i>)` with `TaskState` (`FINALIZED` observed), `BlockedByDependency`.

## Operators

`C` = CommonCounters, `U` = CustomCounters. "Detail only" counters are present in DetailProfile (J) but not in MergedProfile; in M (no DetailProfile) they are absent.

Common to every operator in DetailProfile (J): `IsColocate`, `IsShuffled`, `FollowedByShuffledOperator`, `InitTime`, `OpenTime`, `CloseTime`, `ExecTime`, `MemoryUsage`, `MemoryUsagePeak`.
Sinks additionally: `InputRows`, `PendingFinishDependency`. Sources additionally: `RowsProduced`, `BlocksProduced`, `OutputBlockBytes`, `MaxOutputBlockBytes`, `MinOutputBlockBytes`, `ProjectionTime` (detail).

The dependency-wait counter name is `WaitForDependency[<DEP>]Time`, where `<DEP>` is **not always** `<OPERATOR>_DEPENDENCY` (see join and local exchange sink).

| Operator | Merged counters | Detail-only counters | Seen in |
| --- | --- | --- | --- |
| `RESULT_SINK_OPERATOR` | C: ExecTime, InputRows, MemoryUsage, MemoryUsagePeak, `WaitForDependency[RESULT_SINK_OPERATOR_DEPENDENCY]Time`; U: WaitForDependencyTime | U: AppendBatchTime, ResultSendTime, TupleConvertTime, BytesSent, FetchRowIdTime, NumSentRows, WriteDataTime | J, M |
| `EXCHANGE_OPERATOR` | C: BlocksProduced, ExecTime, Max/Min/OutputBlockBytes, MemoryUsage(Peak), RowsProduced; U: WaitForDependencyTime, `WaitForData0` | U: InstanceID, CreateMergerTime, DataArrivalWaitTime, DecompressBytes, DecompressTime, DeserializeRowBatchTimer, FilterTime, FirstBatchArrivalWaitTime, GetDataFromRecvrTime, LocalBytesReceived, `MaxFindRecvrTime(NS)`, MaxWaitForWorkerTime, MaxWaitToProcessTime, RemoteBytesReceived, `SendersBlockedTotalTimer(*)` | J, M |
| `DATA_STREAM_SINK_OPERATOR` | C: ExecTime, InputRows, MemoryUsage(Peak), WaitForDependencyTime, WaitForRpcBufferQueue; U: BlocksProduced, OverallThroughput | C: WaitForBroadcastBuffer; U: BytesSent, CompressTime, DistributeRowsIntoChannelsTime, LocalBytesSent, LocalSendTime, LocalSentRows, MergeBlockTime, SendNewPartitionTime, AddPartitionRequestTime, SerializeBatchTime, SplitBlockHashComputeTime, UncompressedRowBatchSize | J, M |
| `JDBC_SCAN_OPERATOR` | C: BlocksProduced, ExecTime, Max/Min/OutputBlockBytes, MemoryUsage(Peak), RowsProduced, `WaitForDependency[JDBC_SCAN_OPERATOR_DEPENDENCY]Time`; U: RuntimeFilterInfo, ScanBytes, ScanRows, Scanner | C: WaitForRuntimeFilter; U: TopNFilterSourceNodeIds, RemainedPredicates, PushDownPredicates, MaxScanConcurrency, MinScanConcurrency, NumScanners, RowsRead, AcquireRuntimeFilter, ScannerWorkerWaitTime, JdbcDriverClass, QuerySql, PerScannerRunningTime, PerScannerRowsRead, PerScannerWaitTime, PerScannerProjectionTime, EOS, CheckTypeTime, ConnectorCloseTime, ExecteReadTime, GetDataTime, CastTime, FillBlockTime, HasNextTime, JniSetupTime, PrepareParamsTime, ReadAndFillVectorTableTime, InitConnectorTime, LoadJarTime, NewlyCreateFreeBlocksNum, RunningScanner, RunningScannerPeak, ScannerCpuTime, ScannerFilterTime, ScannerGetBlockTime | J, M |
| `LOCAL_MERGE_SORT_SOURCE_OPERATOR` | C: BlocksProduced, ExecTime, Max/Min/OutputBlockBytes, MemoryUsage(Peak), RowsProduced, `WaitForDependency[LOCAL_MERGE_SORT_SOURCE_OPERATOR_DEPENDENCY]Time` | — | M |
| `SORT_SINK_OPERATOR` | C: ExecTime, InputRows, MemoryUsage(Peak), `WaitForDependency[SORT_SINK_OPERATOR_DEPENDENCY]Time`; U: MemoryUsageSortBlocks | — | M |
| `LOCAL_EXCHANGE_OPERATOR` | C: BlocksProduced, ExecTime, Max/Min/OutputBlockBytes, MemoryUsage(Peak), RowsProduced, `WaitForDependency[LOCAL_EXCHANGE_OPERATOR_DEPENDENCY]Time`; U: GetBlockFailedTime (count) | — | M |
| `LOCAL_EXCHANGE_SINK_OPERATOR` | C: ExecTime, InputRows, MemoryUsage(Peak), `WaitForDependency[LOCAL_EXCHANGE_SINK_DEPENDENCY]Time` | — | M |
| `AGGREGATION_OPERATOR` | C: BlocksProduced, ExecTime, Max/Min/OutputBlockBytes, MemoryUsage(Peak), RowsProduced, `WaitForDependency[AGGREGATION_OPERATOR_DEPENDENCY]Time`; U: HashTableInputCount, HashTableSize, MemoryUsageHashTable | — | M |
| `AGGREGATION_SINK_OPERATOR` | C: ExecTime, InputRows, MemoryUsage(Peak), `WaitForDependency[AGGREGATION_SINK_OPERATOR_DEPENDENCY]Time`; U: MemoryUsageHashTable, MemoryUsageSerializeKeyArena | — | M |
| `STREAMING_AGGREGATION_OPERATOR` | C: BlocksProduced, ExecTime, Max/Min/OutputBlockBytes, MemoryUsage(Peak), RowsProduced; U: MemoryUsageHashTable, MemoryUsageSerializeKeyArena, MemoryUsageSerializeKeyArenaPeak (no InputRows, no dependency wait) | — | M |
| `HASH_JOIN_OPERATOR` (probe) | C: BlocksProduced, ExecTime, Max/Min/OutputBlockBytes, MemoryUsage(Peak), RowsProduced, `WaitForDependency[HASH_JOIN_PROBE_DEPENDENCY]Time`; U: MemoryUsageProbeKeyArena, MemoryUsageProbeKeyArenaPeak, ProbeIntermediateRows, ProbeRows | — | M |
| `HASH_JOIN_SINK_OPERATOR` (build) | C: ExecTime, InputRows, MemoryUsage(Peak), `WaitForDependency[HASH_JOIN_BUILD_DEPENDENCY]Time`; U: MemoryUsageBuildBlocks, MemoryUsageBuildKeyArena, MemoryUsageHashTable, RuntimeFilterInfo, BuildTime, PublishTime | — | M |

Operator header info lines (not counters) observed: `offset`, `limit`, `order by`, `algorithm` (e.g. `heap sort`), `output`, `group by`, `join op` (e.g. `LEFT OUTER JOIN(BROADCAST)[]` — carries the distribution mode), `equal join conjunct`, `projections`, `CATALOG ID`, `TABLE`, `QUERY`.

## Counters referenced by current code but not observed

These are read by `backend/src/services/profile_analyzer/doris.rs` today without a fixture proving they exist:

| Counter | Used by | Status |
| --- | --- | --- |
| `RawRowsRead` | `DORIS_SCAN_VOLUME` fallback | Not observed (OLAP scan fixture needed) |
| any key containing `spill` + `bytes` | `DORIS_SPILL` | Not observed; test uses synthetic `SpillWriteBytes` |
| `RowsRead` on OLAP scans | `DORIS_SCAN_VOLUME` | Observed only on JDBC scan, detail-only |

## Documented in official Doris docs (not yet observed in a fixture)

Documented names can be used in rules **behind a version check**, with the URL cited in a code comment, but still need a real fixture before a rule is enabled by default.

Sources (checked 2026-10-15):

- 4.x Query Profile: https://doris.apache.org/docs/4.x/query-acceleration/query-profile/ (there is no equivalent page for 2.1/3.x)
- 2.1 / 3.x analysis tools: https://doris.apache.org/docs/2.1/query-acceleration/performance-tuning-overview/analysis-tools/ , https://doris.apache.org/docs/3.x/query-acceleration/performance-tuning-overview/analysis-tools/
- 2.1 / 3.x TopN optimization (OLAP scan counters): https://doris.apache.org/docs/2.1/query-acceleration/optimization-technology-principle/topn-optimization/ (3.x same path)
- 2.1 / 3.x runtime filter: https://doris.apache.org/docs/2.1/query-acceleration/optimization-technology-principle/runtime-filter/ and `.../tuning/tuning-execution/adjustment-of-runtimefilter-wait-time/` (3.x same paths)
- HTTP API: https://doris.apache.org/docs/4.x/admin-manual/open-api/fe-http/query-profile-action/ (2.1/3.x same path)
- 4.x spill: https://doris.apache.org/docs/4.x/key-features/spill-to-disk/

| Area | Documented counters | Versions |
| --- | --- | --- |
| Common | `PeakMemoryUsage` (2.1/3.x name; 4.1.3 prints `MemoryUsagePeak`), `InputRows` and `MemoryUsagePeak` listed only for 4.x | 2.1, 3.x, 4.x |
| PipelineTask | `ExecuteTime`, `WaitWorkerTime`, `GetBlockTime`, `PrepareTime`, `SinkTime` | 2.1, 3.x |
| OLAP scan rows | `RowsRead`, `RawRowsRead`, `RowsKeyRangeFiltered`, `RowsBloomFilterFiltered`, `RowsStatsFiltered`, `RowsInvertedIndexFiltered`, `RowsZonemapRuntimePredicateFiltered`, `RowsDictFiltered`, `RowsConditionsFiltered`, `RowsBitmapIndexFiltered`, `RowsVectorPredFiltered`, `RowsVectorPredInput`, `RowsShortCircuitPredFiltered`, `RowsShortCircuitPredInput`, `RowsDelFiltered` (old API example) | 2.1, 3.x |
| OLAP scan IO/segments | `NumSegmentFiltered`, `NumSegmentTotal`, `TabletNum`, `IOTimer`, `CompressedBytesRead`, `UncompressedBytesRead`, `CachedPagesNum`, `TotalPagesNum`, `BlockLoadTime`, `BlocksLoad`, `DecompressorTimer`, `LazyReadTime`, `FirstReadTime`, `InvertedIndexFilterTime`, `InvertedIndexQueryTime`, `InvertedIndexQueryCacheHit/Miss`, `BlockConditionsFilteredTime`, `BlockConditionsFilteredZonemapTime` | 2.1, 3.x |
| Scan scheduling / RF | `NumScanners`, `MaxScannerThreadNum`, `ScannerWorkerWaitTime`, `AcquireRuntimeFilterTime`, `WaitForRuntimeFilter`, `KeyRanges`, `KeyRangesNum`, `ProcessConjunctTime`, `TotalReadThroughput` | 2.1, 3.x |
| Hash join | `BuildRows`, `RuntimeFilterComputeTime`, `RuntimeFilterInitTime`, `RealRuntimeFilterType`, `BloomFilterSize`, `JoinType` | 2.1, 3.x |
| Spill | No profile counters documented in any version. `SpillWriteBytesToLocalStorage` / `SpillReadBytesFromLocalStorage` are `current_queries` API columns (4.x), not profile counters | — |

Not found in any doc or fixture: `RowsZonemapFiltered`, `SegmentNum`.

### Profile-related settings

| Name | Kind | Versions | Notes |
| --- | --- | --- | --- |
| `enable_profile` | session/global var | 2.1, 3.x, 4.x | default `false` |
| `profile_level` | session var | 4.0+ semantics | 1–3, default 1; docs say do not use before 4.0 (different semantics). Level 2 adds EXCHANGE counters (`CreateMergerTime`, `FilterTime`, `GetDataFromRecvrTime`, `MaxFindRecvrTime(NS)`, …) |
| `auto_profile_threshold_ms` | global var | 3.0+ | default `-1` |
| `runtime_filter_type`, `runtime_filter_mode`, `runtime_filter_wait_time_ms` | session var | 2.1, 3.x (docs) | |
| `enable_spill`, `enable_force_spill`, `spill_aggregation_partition_count`, `spill_hash_join_partition_count` | session var | 4.x (docs) | |
| `parallel_pipeline_task_num`, `exec_mem_limit` | session var | 4.x (examples only) | |
| `enable_query_cache`, `query_timeout`, `enable_nereids_planner` | session var | not on profile pages | verify with `SHOW VARIABLES` per version |
| `max_query_profile_num` | FE config | 2.1, 3.x, 4.x | fe-config pages say 100, the 4.x profile page says 500; read the live value |
| `max_spilled_profile_num`, `spilled_profile_storage_path`, `spilled_profile_storage_limit_bytes`, `profile_async_collect_expire_time_secs` | FE config | 3.0+ persistence (4.x page) | |
| `profile_manager_gc_interval_seconds` | FE config | 3.x, 4.x | |
