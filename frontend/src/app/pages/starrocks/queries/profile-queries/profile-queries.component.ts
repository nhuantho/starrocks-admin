import { ChangeDetectorRef, Component, OnInit, OnDestroy, TemplateRef, ViewChild, ViewEncapsulation } from '@angular/core';
import { ActivatedRoute } from '@angular/router';
import { Location } from '@angular/common'; // Import Location
import { NbToastrService, NbDialogService } from '@nebular/theme';
import { LocalDataSource } from 'ng2-smart-table';
import { Subject, forkJoin, of } from 'rxjs';
import { catchError, takeUntil } from 'rxjs/operators';
import { NodeService, ProfileListItem } from '../../../../@core/data/node.service';
import { ClusterContextService } from '../../../../@core/data/cluster-context.service';
import { Cluster } from '../../../../@core/data/cluster.service';
import { ErrorHandler } from '../../../../@core/utils/error-handler';
import { MetricThresholds, renderMetricBadge, parseStarRocksDuration } from '../../../../@core/utils/metric-badge';
import { renderLongText } from '../../../../@core/utils/text-truncate';
import { AuthService } from '../../../../@core/data/auth.service';
import * as dagre from 'dagre';
import { TranslateService } from '@ngx-translate/core';

@Component({
  selector: 'ngx-profile-queries',
  templateUrl: './profile-queries.component.html',
  styleUrls: ['./profile-queries.component.scss'],
  encapsulation: ViewEncapsulation.None,
})
export class ProfileQueriesComponent implements OnInit, OnDestroy {
  // Data sources
  profileSource: LocalDataSource = new LocalDataSource();
  
  // State
  clusterId: number;
  activeCluster: Cluster | null = null;
  loading = true;
  autoRefresh = false; // Default: disabled
  refreshInterval: any;
  selectedRefreshInterval: number | 'off' = 'off'; // Default: off (Grafana style)
  refreshIntervalOptions: { value: number | 'off'; label: string }[] = [];
  private destroy$ = new Subject<void>();
  private profileDurationThresholds: MetricThresholds = { warn: 120000, danger: 240000 }; // Will be updated dynamically

  // Profile dialog
  currentProfileDetail: string = '';
  currentQueryId: string = '';
  profileDetailLoading = false;
  @ViewChild('profileDetailDialog') profileDetailDialogTemplate: TemplateRef<any>;
  @ViewChild('queryExplainDialog') queryExplainDialogTemplate: TemplateRef<any>;
  profiles: ProfileListItem[] = [];
  inspectedQuery: ProfileListItem | null = null;
  explainSql = '';
  explainCatalog = '';
  explainDatabase = '';
  explainCatalogs: string[] = [];
  explainPlan = '';
  explainTimeMs: number | null = null;
  explainLoading = false;
  explainError = '';
  queryStatusLoading = false;
  queryStatusError = '';
  nativeProfileLoading = false;
  nativeProfileText = '';
  nativeProfileError = '';
  private explainRequestId = 0;
  diagnosticsState = 'UNKNOWN';
  diagnosticsUpdatedAt: Date | null = null;
  diagnosticsNotice = '';
  diagnosticsAutoRefresh = true;
  diagnosticsRefreshing = false;
  diagnosticsFinal = false;
  private diagnosticsTimer: any;
  private diagnosticsEpoch = 0;
  private diagnosticsRetries = 0;
  private diagnosticsStop$ = new Subject<void>();
  private diagnosticsDialog: any;

  private stopDiagnostics(): void {
    clearTimeout(this.diagnosticsTimer);
    this.diagnosticsTimer = null;
    this.diagnosticsEpoch++;
    this.diagnosticsStop$.next();
    this.diagnosticsRefreshing = false;
  }

  toggleDiagnosticsRefresh(): void {
    this.diagnosticsAutoRefresh = !this.diagnosticsAutoRefresh;
    clearTimeout(this.diagnosticsTimer);
    if (this.diagnosticsAutoRefresh) { this.refreshAnalysis(); }
  }

  get diagnosticsProvisional(): boolean {
    return !this.diagnosticsFinal;
  }
  
  get runningProfileCount(): number {
    return this.profiles.filter(query => query.State === 'RUNNING').length;
  }
  get finishedProfileCount(): number {
    return this.profiles.filter(query => query.State === 'FINISHED').length;
  }
  get unsuccessfulProfileCount(): number {
    return this.profiles.filter(query => ['FAILED', 'CANCELLED', 'CANCELED', 'ABORTED'].includes(query.State)).length;
  }

  activeCauseNodeIds = new Set<string>();
  selectedCauseIndex: number | null = null;

  selectCausePath(index: number): void {
    this.selectedCauseIndex = this.selectedCauseIndex === index ? null : index;
    const path = this.analysisData?.root_cause_paths?.[this.selectedCauseIndex as number];
    this.activeCauseNodeIds = new Set(path?.node_ids || []);
  }

  isCauseEdge(edge: any): boolean {
    const path = this.analysisData?.root_cause_paths?.[this.selectedCauseIndex as number]?.node_ids || [];
    return path.some((id: string, index: number) => id === edge.v && path[index + 1] === edge.w);
  }

  causeNodeName(nodeId: string): string {
    const node = this.graphNodes.find(item => item.id === nodeId);
    return node ? `${node.operator_name} · F${node.fragment_id ?? '—'}/P${node.pipeline_id ?? '—'}` : nodeId;
  }

  getDorisTimingMetrics(): {key: string; value: string}[] {
    if (this.analysisData?.engine !== 'doris' || !this.selectedNode) { return []; }
    return Object.entries(this.selectedNode.unique_metrics || {})
      .filter(([key]) => key.includes('Time') && key !== 'OperatorTotalTime')
      .map(([key, value]) => ({ key, value: String(value) }))
      .sort((a, b) => {
        const rank = (key: string) => key === 'ExecTime' ? 0 : key === 'InitConnectorTime' ? 1 : key.startsWith('WaitForDependency') ? 2 : 3;
        return rank(a.key) - rank(b.key) || a.key.localeCompare(b.key);
      });
  }

  // DAG Analysis
  analysisLoading = false;
  analysisError: string = '';
  analysisData: any = null;
  topNodes: any[] = [];
  graphNodes: any[] = [];
  graphEdges: any[] = [];
  graphWidth = 800;
  graphHeight = 600;
  selectedNode: any = null;
  zoomLevel = 1; // Zoom level for DAG
  // Panning state
  isPanning = false;
  startX = 0;
  startY = 0;
  translateX = 0;
  translateY = 0;
  
  // Window control state
  isFullscreen = false; // Default to normal layout, toggle for full screen
  
  // Right Panel State (default width expanded by 30%)
  rightPanelWidth = 500;
  isRightPanelCollapsed = false;
  isResizingRight = false;
  
  // Right panel section collapse states (Figure 1 style)
  isTop10Collapsed = false;
  isSummaryCollapsed = false;
  isDiagnosisCollapsed = false;
  showMemoryView = false; // Toggle between Time and Memory view
  
  // Graph direction: 'BT' (bottom-to-top) or 'LR' (left-to-right)
  graphDirection: 'BT' | 'LR' = 'BT';
  
  // Node detail view state (when a node is selected)
  selectedNodeTab: 'core' | 'detail' | 'pipeline' = 'core';
  
  // Core metrics by operator type
  private coreMetricsByType: { [key: string]: string[] } = {
    'SCAN': ['RawRowsRead', 'BytesRead', 'CompressedBytesRead', 'ScanTime', 'IOTime', 'Table', 'Rollup', 'TabletCount', 'PushdownPredicates', 'BitmapIndexFilterRows', 'BloomFilterFilterRows', 'RuntimeInFilterRows', 'RuntimeBloomFilterRows'],
    'JOIN': ['BuildRows', 'ProbeRows', 'HashTableMemoryUsage', 'RuntimeFilterNum', 'JoinType', 'DistributionMode', 'HashTableSize', 'ProbeTime', 'BuildTime'],
    'AGGREGATE': ['InputRowCount', 'OutputRowCount', 'HashTableMemoryUsage', 'AggFuncComputeTime', 'GroupByKeyCount', 'StreamingTime', 'HashTableIteratorTime'],
    'EXCHANGE': ['ShuffleRowsPerBuffer', 'NetworkTime', 'WaitTime', 'OverallThroughput', 'PeakBufferUsage'],
    'SORT': ['SortKeys', 'SortType', 'TopNLimit', 'SortTime', 'MergeTime'],
    'PROJECT': ['ExprComputeTime', 'CommonSubExprComputeTime'],
    'DEFAULT': ['OperatorTotalTime', 'PullRowNum', 'PushRowNum', 'PeakMemoryUsage']
  };
  
  private nodeRankMap: Map<string, number> = new Map(); // Node rank by time percentage
  objectKeys = Object.keys; // Helper for template

  // Metric descriptions for tooltips
  // Reference: https://docs.starrocks.io/zh/docs/best_practices/query_tuning/query_profile_operator_metrics/
  // Descriptions live in assets/i18n/<lang>/profile.json under `profile.metrics.<Key>`
  private readonly metricDescriptionKeys: string[] = [
    // ============================================
    // 概要指标 (Summary Metrics)
    // ============================================
    'Total',
    'QueryState',
    'QueryId',
    'StartTime',
    'EndTime',
    'QueryType',
    'StarRocksVersion',
    'User',
    'DefaultDb',
    'SqlStatement',
    'Variables',
    'NonDefaultSessionVariables',
    'CollectProfileTime',
    'IsProfileAsync',
    // ============================================
    // 执行概览指标 (Execution Overview Metrics)
    // ============================================
    'FrontendProfileMergeTime',
    'QueryAllocatedMemoryUsage',
    'QueryDeallocatedMemoryUsage',
    'QueryPeakMemoryUsagePerNode',
    'QuerySumMemoryUsage',
    'QueryExecutionWallTime',
    'ExecutionWallTime',
    'QueryCumulativeCpuTime',
    'QueryCpuTime',
    'QueryCumulativeOperatorTime',
    'OperatorTime',
    'QueryCumulativeNetworkTime',
    'QueryNetworkTime',
    'QueryCumulativeScanTime',
    'QueryScanTime',
    'QueryPeakScheduleTime',
    'PeakScheduleTime',
    'QuerySpillBytes',
    'SpillBytes',
    'ResultDeliverTime',
    'MemoryUsage',
    // ============================================
    // Fragment 指标
    // ============================================
    'InstanceNum',
    'InstanceIds',
    'BackendNum',
    'BackendAddresses',
    'FragmentInstancePrepareTime',
    'InstanceAllocatedMemoryUsage',
    'InstanceDeallocatedMemoryUsage',
    'InstancePeakMemoryUsage',
    'FragmentID',
    // ============================================
    // Pipeline 指标
    // ============================================
    'DriverTotalTime',
    'ActiveTime',
    'PendingTime',
    'ScheduleTime',
    'InputEmptyTime',
    'FirstInputEmptyTime',
    'OutputFullTime',
    'PreconditionBlockTime',
    'PipelineID',
    'Depth',
    'NodeType',
    'IsHotspot',
    'HotspotSeverity',
    'IsGroupExecution',
    'BlockByInputEmpty',
    'BlockByOutputFull',
    'BlockByPrecondition',
    'DegreeOfParallelism',
    'TotalDegreeOfParallelism',
    'PeakDriverQueueSize',
    'ScheduleCount',
    'YieldByLocalWait',
    'YieldByPreempt',
    'YieldByTimeLimit',
    'EnableEventScheduler',
    'InitialProcessDriverCount',
    'InitialProcessMem',
    'JITCounter',
    'JITTotalCostTime',
    'QueryMemoryLimit',
    'BackendProfileMergeTime',
    // ============================================
    // Operator 通用指标
    // ============================================
    'OperatorTotalTime',
    'PushRowNum',
    'PullRowNum',
    'PullChunkNum',
    'PushChunkNum',
    'PullTotalTime',
    'PushTotalTime',
    'PeakMemoryUsage',
    'OutputRows',
    'TimePercentage',
    'OutputChunkBytes',
    'IsSubordinate',
    'ConjunctsInputRows',
    'ConjunctsOutputRows',
    'ConjunctsTime',
    'RuntimeFilterNum',
    'RuntimeInFilterNum',
    'JoinRuntimeFilterEvaluate',
    'JoinRuntimeFilterHashTime',
    'JoinRuntimeFilterInputRows',
    'JoinRuntimeFilterOutputRows',
    'JoinRuntimeFilterTime',
    'SetFinishingTime',
    'RequestReceived',
    'SinkType',
    'AppendChunkTime',
    'ResultRendTime',
    'TupleConvertTime',
    'NumSentRows',
    // ============================================
    // Scan Operator (OLAP/Connector)
    // ============================================
    'Table',
    'Rollup',
    'ScanTime',
    'TabletCount',
    'MorselsCount',
    'PushdownPredicates',
    'BytesRead',
    'RowsRead',
    'RawRowsRead',
    'CompressedBytesRead',
    'UncompressedBytesRead',
    'IOTime',
    'IOTimeTotal',
    'IOTimeLocalDisk',
    'IOTimeRemote',
    'BitmapIndexFilterRows',
    'BitmapIndexFilter',
    'BloomFilterFilterRows',
    'BloomFilterFilter',
    'SegmentRuntimeZoneMapFilterRows',
    'SegmentZoneMapFilterRows',
    'ZoneMapIndexFilterRows',
    'ZoneMapIndexFiter',
    'ShortKeyFilterRows',
    'ShortKeyFilter',
    'PredFilter',
    'PredFilterRows',
    'DelVecFilterRows',
    'SegmentInit',
    'SegmentRead',
    'ColumnIteratorInit',
    'BlockSeek',
    'BlockSeekCount',
    'BlockFetch',
    'BlockFetchCount',
    'ChunkCopy',
    'DecompressT',
    'CreateSegmentIter',
    'GetDelVec',
    'GetDeltaColumnGroup',
    'PrepareChunkSourceTime',
    'SubmitTaskTime',
    'SubmitTaskCount',
    'IOTaskWaitTime',
    'IOTaskExecTime',
    'PrefetchHitCount',
    'PrefetchPendingTime',
    'PrefetchWaitFinishTime',
    // Connector Scan (存算分离)
    'CompressedBytesReadLocalDisk',
    'CompressedBytesReadRemote',
    'CompressedBytesReadRequest',
    'CompressedBytesReadTotal',
    'IOCountLocalDisk',
    'IOCountRemote',
    'IOCountRequest',
    'IOCountTotal',
    'PagesCountLocalDisk',
    'PagesCountRemote',
    'PagesCountMemory',
    'PagesCountTotal',
    'DataSourceType',
    'SharedScan',
    'MorselQueueType',
    'AdaptiveIOTasks',
    'PeakIOTasks',
    'ChunkBufferCapacity',
    'DefaultChunkBufferCapacity',
    'PeakChunkBufferSize',
    'PeakChunkBufferMemoryUsage',
    'PeakScanTaskQueueSize',
    'RowsetsReadCount',
    'SegmentsReadCount',
    'TotalColumnsDataPageCount',
    'AccessPathHits',
    'AccessPathUnhits',
    'PushdownAccessPaths',
    'ShortKeyRangeNumber',
    'RemainingRowsAfterShortKeyFilter',
    'MemAllocFailed',
    'IOStatistics',
    'ReadPKIndex',
    'BitmapIndexIteratorInit',
    'RawInputBytes',
    // ============================================
    // Exchange Operator
    // ============================================
    // Sink
    'PartType',
    'DestFragments',
    'DestID',
    'ChannelNum',
    'BytesSent',
    'BytesUnsent',
    'RequestSent',
    'RequestUnsent',
    'BytesPassThrough',
    'PassThroughBufferPeakMemoryUsage',
    'OverallThroughput',
    'OverallTime',
    'NetworkTime',
    'WaitTime',
    'NetworkBandwidth',
    'SerializeChunkTime',
    'SerializedBytes',
    'CompressTime',
    'CompressedBytes',
    'RpcCount',
    'RpcAvgTime',
    'ShuffleHashTime',
    'ShuffleChunkAppendTime',
    'ShuffleChunkAppendCounter',
    'BufferUnplugCount',
    'ClosureBlockCount',
    'ClosureBlockTime',
    // Source
    'SenderWaitLockTime',
    'WaitLockTime',
    'BytesReceived',
    'DecompressChunkTime',
    'DeserializeChunkTime',
    'SenderTotalTime',
    'ReceiverProcessTotalTime',
    'PeakBufferMemoryBytes',
    // ============================================
    // Aggregate Operator
    // ============================================
    'GroupingKeys',
    'AggregateFunctions',
    'AggComputeTime',
    'ExprComputeTime',
    'HashTableSize',
    'HashTableMemoryUsage',
    'InputRowCount',
    'PassThroughRowCount',
    'ResultAggAppendTime',
    'ResultGroupByAppendTime',
    'GetResultsTime',
    'IteratorMergeTime',
    'StreamingTime',
    'AggStateName',
    // ============================================
    // Join Operator
    // ============================================
    // Probe
    'DistributionMode',
    'JoinType',
    'OtherJoinConjunctEvaluateTime',
    'ProbeConjunctEvaluateTime',
    'SearchHashTableTime',
    'SearchHashTableCount',
    'WhereConjunctEvaluateTime',
    'OutputBuildColumnTime',
    'OutputProbeColumnTime',
    'OutputTupleColumnTime',
    'ProbeRowsCounter',
    // Build
    'JoinPredicates',
    'BuildBuckets',
    'BuildHashTableTime',
    'CopyRightTableChunkTime',
    'RuntimeFilterBuildTime',
    'BuildConjunctEvaluateTime',
    'BuildRowsCounter',
    'RuntimeBloomFilterNum',
    // ============================================
    // Window Function Operator
    // ============================================
    'ComputeTime',
    'PartitionKeys',
    'PartitionChunksNum',
    'PartitionRowsNum',
    'PeerGroupChunksNum',
    'PeerGroupRowsNum',
    'ColumnResize',
    // ============================================
    // Sort Operator
    // ============================================
    'SortKeys',
    'SortType',
    'MaxBufferedRows',
    'MaxBufferedBytes',
    'NumSortedRuns',
    'BuildingTime',
    'MergingTime',
    'SortingTime',
    'OutputTime',
    // ============================================
    // Merge Operator
    // ============================================
    'Limit',
    'Offset',
    'StreamingBatchSize',
    'LateMaterializationMaxBufferChunkNum',
    'OverallStageCount',
    'OverallStageTime',
    'LateMaterializationGenerateOrdinalTime',
    'SortedRunProviderTime',
    // ============================================
    // TableFunction Operator
    // ============================================
    'TableFunctionExecTime',
    'TableFunctionExecCount',
    // ============================================
    // Project Operator
    // ============================================
    'CommonSubExprComputeTime',
    // ============================================
    // LocalExchange Operator
    // ============================================
    'Type',
    'ShuffleNum',
    'LocalExchangePeakMemoryUsage',
    'LocalExchangePeakBufferSize',
    'LocalExchangePeakBufferMemoryUsage',
    'LocalExchangePeakBufferChunkNum',
    'LocalExchangePeakBufferRowNum',
    'LocalExchangePeakBufferBytes',
    'LocalExchangePeakBufferChunkSize',
    'LocalExchangePeakBufferChunkRowNum',
    'LocalExchangePeakBufferChunkBytes',
    // ============================================
    // OlapTableSink Operator
    // ============================================
    'IndexNum',
    'ReplicatedStorage',
    'TxnID',
    'RowsFiltered',
    'RowsReturned',
    'RpcClientSideTime',
    'RpcServerSideTime',
    'PrepareDataTime',
    'SendDataTime',
    // ============================================
    // 执行时间分类 (Execution Time Categories)
    // ============================================
    'IO',
    'Processing',
    'IoSeekTime',
    'LocalDiskReadIOTime',
    'RemoteReadIOTime',
  ];

  // Lower-cased metric name -> canonical metric name (for case-insensitive lookup)
  private readonly metricDescriptionKeyMap: Map<string, string> = new Map(
    this.metricDescriptionKeys.map(k => [k.toLowerCase(), k] as [string, string]),
  );

  // Get metric description (case-insensitive matching)
  getMetricDescription(key: string): string | null {
    if (!key) {
      return null;
    }
    const canonicalKey = this.metricDescriptionKeyMap.get(key.toLowerCase());
    return canonicalKey ? this.translate.instant(`profile.metrics.${canonicalKey}`) : null;
  }

  // Exact-match metric description lookup
  private getExactMetricDescription(key: string): string | undefined {
    return this.metricDescriptionKeyMap.get(key.toLowerCase()) === key
      ? this.translate.instant(`profile.metrics.${key}`)
      : undefined;
  }

  // Profile management settings
  profileSettings: any;

  private buildProfileSettings(): any {
    return {
      mode: 'external',
      hideSubHeader: false, // Enable search
      noDataMessage: this.translate.instant('profile.list.noData'),
      actions: {
        add: false,
        edit: true,
        delete: false,
        position: 'right',
        width: '120px',
        custom: [{ name: 'explain', title: '<span role="img" aria-label="Explain SQL" title="Explain SQL"><i class="fas fa-code" aria-hidden="true"></i></span>' }],
      },
      edit: {
        editButtonContent: '<span role="img" aria-label="Query diagnostics" title="Query diagnostics"><i class="nb-search" aria-hidden="true"></i></span>',
      },
      pager: {
        display: true,
        perPage: 20,
      },
      columns: {
        QueryId: { title: 'Query ID', type: 'string', width: '25%' },
        StartTime: { title: this.translate.instant('profile.list.startTime'), type: 'string', width: '15%' },
        Time: {
          title: this.translate.instant('profile.list.executionTime'),
          type: 'html',
          width: '10%',
          valuePrepareFunction: (value: string | number) => {
            // Parse StarRocks duration string to milliseconds for accurate threshold comparison
            const durationMs = parseStarRocksDuration(value);
            return renderMetricBadge(durationMs, this.profileDurationThresholds, {
              labelFormatter: (val) => {
                // Use original string for display, but parsed number for thresholds
                return typeof value === 'string' ? value : `${val}ms`;
              }
            });
          },
        },
        State: {
          title: this.translate.instant('profile.list.state'),
          type: 'html',
          width: '10%',
          valuePrepareFunction: (value: string) => {
            const safeState = this.safeState(value);
            return `<span class="badge badge-${this.queryStateColor(safeState)}">${safeState}</span>`;
          },
        },
        Statement: { 
          title: this.translate.instant('profile.list.sqlStatement'), 
          type: 'html', 
          width: '40%',
          valuePrepareFunction: (value: any) => renderLongText(value, 100),
        },
        Catalog: { title: this.translate.instant('profile.inspect.catalog'), type: 'string', width: '12%' },
        User: { title: this.translate.instant('profile.inspect.user'), type: 'string', width: '8%' },
      },
    };
  }

  constructor(
    private route: ActivatedRoute,
    private nodeService: NodeService,
    private clusterContextService: ClusterContextService,
    private toastrService: NbToastrService,
    private dialogService: NbDialogService,
    private authService: AuthService,
    private cdr: ChangeDetectorRef,
    private location: Location, // Inject Location
    private translate: TranslateService,
  ) {
    this.refreshIntervalOptions = this.buildRefreshIntervalOptions();
    this.profileSettings = this.buildProfileSettings();
    // Try to get clusterId from route first (for direct navigation)
    const routeClusterId = parseInt(this.route.snapshot.paramMap.get('clusterId') || '0', 10);
    this.clusterId = routeClusterId;
  }

  private buildRefreshIntervalOptions(): { value: number | 'off'; label: string }[] {
    return [
      { value: 'off', label: this.translate.instant('profile.refresh.off') },
      { value: 3, label: this.translate.instant('profile.refresh.seconds', { n: 3 }) },
      { value: 5, label: this.translate.instant('profile.refresh.seconds', { n: 5 }) },
      { value: 10, label: this.translate.instant('profile.refresh.seconds', { n: 10 }) },
      { value: 30, label: this.translate.instant('profile.refresh.seconds', { n: 30 }) },
      { value: 60, label: this.translate.instant('profile.refresh.minutes', { n: 1 }) },
    ];
  }

  ngOnInit(): void {
    this.translate.onLangChange
      .pipe(takeUntil(this.destroy$))
      .subscribe(() => {
        this.refreshIntervalOptions = this.buildRefreshIntervalOptions();
        this.profileSettings = this.buildProfileSettings();
      });

    // Subscribe to active cluster changes
    this.clusterContextService.activeCluster$
      .pipe(takeUntil(this.destroy$))
      .subscribe(cluster => {
        this.activeCluster = cluster;
        if (this.clusterId !== cluster?.id) {
          this.stopDiagnostics();
          this.diagnosticsDialog?.close();
        }
        if (cluster) {
          // Always use the active cluster (override route parameter)
          const newClusterId = cluster.id;
          if (this.clusterId !== newClusterId) {
            this.clusterId = newClusterId;
            this.loadProfiles();
          }
        }
        // Backend will handle "no active cluster" case
      });

    // Load data - backend will get active cluster automatically
    this.loadProfiles();
  }

  ngOnDestroy(): void {
    this.stopAutoRefresh();
    this.stopDiagnostics();
    this.diagnosticsDialog?.close();
    this.destroy$.next();
    this.destroy$.complete();
  }

  // Grafana-style: selecting an interval automatically enables auto-refresh
  // Selecting 'off' disables auto-refresh
  onRefreshIntervalChange(interval: number | 'off'): void {
    this.selectedRefreshInterval = interval;
    
    if (interval === 'off') {
      // Disable auto-refresh
      this.autoRefresh = false;
      this.stopAutoRefresh();
    } else {
      // Enable auto-refresh with selected interval
      this.autoRefresh = true;
      this.stopAutoRefresh();
      this.startAutoRefresh();
    }
  }

  startAutoRefresh(): void {
    this.stopAutoRefresh(); // Clear any existing interval
    
    // Only start if interval is a number (not 'off')
    if (typeof this.selectedRefreshInterval !== 'number') {
      return;
    }
    
    this.refreshInterval = setInterval(() => {
      // Stop auto-refresh if user is not authenticated (logged out)
      if (!this.authService.isAuthenticated()) {
        this.autoRefresh = false;
        this.selectedRefreshInterval = 'off';
        this.stopAutoRefresh();
        return;
      }
      // Only update data, don't show loading spinner during auto-refresh
      this.loadProfilesSilently();
    }, this.selectedRefreshInterval * 1000);
  }

  stopAutoRefresh(): void {
    if (this.refreshInterval) {
      clearInterval(this.refreshInterval);
      this.refreshInterval = null;
    }
  }

  // Load profiles
  loadProfiles(): void {
    this.loading = true;
    this.nodeService.listProfiles().subscribe(
      data => {
        this.profiles = data;
        this.profileSource.load(data);
        this.updateDynamicThresholds(data);
        this.loading = false;
      },
      error => {
        this.toastrService.danger(ErrorHandler.handleClusterError(error), this.translate.instant('profile.loadFailed'));
        this.loading = false;
      }
    );
  }

  // Load profiles silently (for auto-refresh, without loading spinner)
  loadProfilesSilently(): void {
    this.nodeService.listProfiles().subscribe(
      data => {
        this.profiles = data;
        if (this.inspectedQuery) {
          const updated = data.find(query => query.QueryId === this.inspectedQuery.QueryId);
          if (updated) { this.inspectedQuery = updated; this.queryStatusError = ''; }
          else { this.queryStatusError = this.translate.instant('profile.inspect.statusMissing'); }
        }
        this.profileSource.load(data);
        this.updateDynamicThresholds(data);
      },
      error => {
        // Silently handle errors during auto-refresh
        console.error('Failed to refresh profiles:', error);
      }
    );
  }

  /**
   * Update dynamic thresholds based on maximum time in current data
   * Algorithm:
   * - Find the maximum execution time in the dataset
   * - Red (danger): > max_time * 70%
   * - Yellow (warning): > max_time * 40% and <= max_time * 70%
   * - Green (success): <= max_time * 40%
   * 
   * This ensures color coding adapts to the actual data range
   */
  updateDynamicThresholds(profiles: any[]): void {
    if (!profiles || profiles.length === 0) {
      return;
    }

    // Extract duration values from profiles
    const durationValues = profiles
      .map(profile => parseStarRocksDuration(profile.Time))
      .filter(value => !isNaN(value) && value > 0);

    if (durationValues.length === 0) {
      // No valid data, use defaults
      return;
    }

    // Find maximum time
    const maxTime = Math.max(...durationValues);
    
    // Calculate thresholds based on max time percentage
    // Red: > 70% of max time
    // Yellow: > 40% of max time
    const warnThreshold = maxTime * 0.5;   // 40% of max
    const dangerThreshold = maxTime * 0.8; // 70% of max

    // Update thresholds
    this.profileDurationThresholds = {
      warn: warnThreshold,
      danger: dangerThreshold,
    };
  }

  // Helper: Format milliseconds to readable duration
  private formatDuration(ms: number): string {
    if (ms < 1000) return `${ms}ms`;
    if (ms < 60000) return `${(ms / 1000).toFixed(2)}s`;
    return `${(ms / 60000).toFixed(2)}m`;
  }

  // Handle profile edit action (view profile)
  onProfileEdit(event: any): void {
    this.viewProfileDetail(event.data.QueryId);
  }

  onProfileCustom(event: any): void {
    if (event.action === 'explain') { this.openExplain(event.data); }
  }

  safeState(value: string): string {
    const state = String(value || 'UNKNOWN').toUpperCase();
    return /^[A-Z0-9_ -]{1,64}$/.test(state) ? state : 'UNKNOWN';
  }

  queryStateColor(value: string): string {
    switch (this.safeState(value)) {
      case 'FINISHED': case 'EOF': case 'SUCCESS': return 'success';
      case 'FAILED': case 'ERROR': case 'ERR': case 'ABORTED': case 'CANCELLED': case 'CANCELED': return 'danger';
      case 'RUNNING': case 'PENDING': case 'QUEUED': return 'warning';
      default: return 'secondary';
    }
  }

  openExplain(query: ProfileListItem | null = null): void {
    this.inspectedQuery = query;
    this.explainSql = query?.Statement || '';
    this.explainCatalog = query?.Catalog || '';
    this.explainDatabase = query?.Database || '';
    this.explainPlan = '';
    this.explainError = '';
    this.explainTimeMs = null;
    this.explainLoading = false;
    this.queryStatusLoading = false;
    this.queryStatusError = '';
    this.nativeProfileLoading = false;
    this.nativeProfileText = '';
    this.nativeProfileError = '';
    this.explainRequestId++;
    this.dialogService.open(this.queryExplainDialogTemplate, {hasBackdrop: true, closeOnBackdropClick: false, closeOnEsc: true});
    this.nodeService.getCatalogs().pipe(takeUntil(this.destroy$)).subscribe({
      next: catalogs => {
        const selectedCatalog = this.explainCatalog || catalogs.find(catalog => catalog === 'internal' || catalog === 'default_catalog') || '';
        this.explainCatalogs = catalogs;
        this.explainCatalog = selectedCatalog;
        this.cdr.markForCheck();
      },
      error: error => { this.explainError = ErrorHandler.extractErrorMessage(error); this.cdr.markForCheck(); },
    });
  }

  generateExplain(): void {
    const requestId = ++this.explainRequestId;
    this.explainLoading = true;
    this.explainError = '';
    this.explainPlan = '';
    this.explainTimeMs = null;
    this.nodeService.explainQuery(this.explainSql, this.explainCatalog || undefined, this.explainDatabase || undefined)
      .pipe(takeUntil(this.destroy$)).subscribe({
        next: result => {
          if (requestId !== this.explainRequestId) { return; }
          this.explainPlan = result.plan;
          this.explainTimeMs = result.execution_time_ms;
          this.explainLoading = false;
          this.cdr.markForCheck();
        },
        error: error => {
          if (requestId !== this.explainRequestId) { return; }
          this.explainError = ErrorHandler.extractErrorMessage(error);
          this.explainLoading = false;
          this.cdr.markForCheck();
        },
      });
  }

  refreshQueryStatus(): void {
    const queryId = this.inspectedQuery?.QueryId;
    if (!queryId) { return; }
    this.queryStatusLoading = true;
    this.queryStatusError = '';
    this.nodeService.getProfileStatus(queryId).pipe(takeUntil(this.destroy$)).subscribe({
      next: query => {
        if (this.inspectedQuery?.QueryId !== queryId) { return; }
        this.inspectedQuery = query;
        this.queryStatusLoading = false;
        this.cdr.markForCheck();
      },
      error: error => {
        if (this.inspectedQuery?.QueryId !== queryId) { return; }
        this.queryStatusError = ErrorHandler.extractErrorMessage(error);
        this.queryStatusLoading = false;
        this.cdr.markForCheck();
      },
    });
  }

  loadNativeProfile(): void {
    const queryId = this.inspectedQuery?.QueryId;
    if (!queryId) { return; }
    this.nativeProfileLoading = true;
    this.nativeProfileError = '';
    this.nodeService.getProfile(queryId).pipe(takeUntil(this.destroy$)).subscribe({
      next: profile => {
        if (this.inspectedQuery?.QueryId !== queryId) { return; }
        this.nativeProfileText = profile.profile_content;
        this.nativeProfileLoading = false;
        this.cdr.markForCheck();
      },
      error: error => {
        if (this.inspectedQuery?.QueryId !== queryId) { return; }
        this.nativeProfileError = ErrorHandler.extractErrorMessage(error);
        this.nativeProfileLoading = false;
        this.cdr.markForCheck();
      },
    });
  }

  // View profile detail from profile list
  viewProfileDetail(queryId: string): void {
    this.stopDiagnostics();
    this.diagnosticsState = this.profiles.find(query => query.QueryId === queryId)?.State || 'UNKNOWN';
    this.diagnosticsUpdatedAt = null;
    this.diagnosticsNotice = '';
    this.diagnosticsAutoRefresh = true;
    this.diagnosticsFinal = false;
    this.diagnosticsRetries = 0;
    this.currentQueryId = queryId;
    this.profileDetailLoading = true;
    this.analysisLoading = true;
    this.currentProfileDetail = '';
    this.analysisData = null;
    this.analysisError = '';
    this.topNodes = [];
    this.graphNodes = [];
    this.graphEdges = [];
    this.selectedNode = null;
    this.activeCauseNodeIds = new Set();
    this.selectedCauseIndex = null;
    
    // Open dialog first with loading state
    this.diagnosticsDialog = this.dialogService.open(this.profileDetailDialogTemplate, {
      context: { profile: this.currentProfileDetail },
      hasBackdrop: true,
      closeOnBackdropClick: true,
      closeOnEsc: true,
      dialogClass: 'profile-dialog-lg',
    });
    
    const epoch = this.diagnosticsEpoch;
    this.diagnosticsDialog.onClose.pipe(takeUntil(this.destroy$)).subscribe(() => {
      if (epoch === this.diagnosticsEpoch) { this.stopDiagnostics(); }
    });
    this.loadAnalysis(queryId);
  }
  
  // Poll status and the original runtime profile together; never rerun SQL.
  loadAnalysis(queryId: string): void {
    if (this.diagnosticsRefreshing) { return; }
    clearTimeout(this.diagnosticsTimer);
    const epoch = this.diagnosticsEpoch;
    this.diagnosticsRefreshing = true;
    this.analysisLoading = !this.analysisData;
    this.profileDetailLoading = true;
    forkJoin({
      status: this.nodeService.getProfileStatus(queryId).pipe(catchError(error => of({ error }))),
      analysis: this.nodeService.analyzeProfile(queryId).pipe(catchError(error => of({ error }))),
    }).pipe(takeUntil(this.destroy$), takeUntil(this.diagnosticsStop$)).subscribe((result: any) => {
      if (epoch !== this.diagnosticsEpoch || queryId !== this.currentQueryId) { return; }
      const data = result.analysis.error ? null : result.analysis;
      const reportedState = data?.summary?.query_state;
      const liveState = result.status.error ? null : result.status.State;
      this.diagnosticsState = this.safeState(liveState || reportedState || 'UNKNOWN');
      const terminal = (state: string) => ['FINISHED', 'EOF', 'OK', 'SUCCESS', 'FAILED', 'ERROR', 'ERR',
        'CANCELLED', 'CANCELED', 'ABORTED'].includes(this.safeState(state));
      this.diagnosticsFinal = !!data && terminal(this.diagnosticsState) && terminal(reportedState)
        && data.summary?.is_profile_complete !== false;
      this.diagnosticsNotice = '';
      if (data) {
        const selectedId = this.selectedNode?.id;
        const previousPath = this.analysisData?.root_cause_paths?.[this.selectedCauseIndex as number]?.node_ids?.join('|');
        const hadGraph = !!this.analysisData;
        this.analysisData = data;
        this.analysisError = '';
        this.topNodes = data.summary?.top_time_consuming_nodes || [];
        this.currentProfileDetail = data.profile_content || '';
        this.diagnosticsUpdatedAt = new Date();
        if (data.execution_tree) { this.buildGraph(data.execution_tree, hadGraph); }
        this.selectedNode = this.graphNodes.find(node => node.id === selectedId) || null;
        const matchingPath = data.root_cause_paths?.findIndex(path => path.node_ids.join('|') === previousPath);
        this.selectedCauseIndex = matchingPath >= 0 ? matchingPath : (!hadGraph && data.root_cause_paths?.length ? 0 : null);
        this.activeCauseNodeIds = new Set(data.root_cause_paths?.[this.selectedCauseIndex as number]?.node_ids || []);
      } else {
        const error = result.analysis.error;
        // Keep the last good graph visible on a transient fetch/collection failure.
        this.analysisError = this.analysisData ? '' : ErrorHandler.extractErrorMessage(error);
        this.diagnosticsNotice = 'profile.live.waiting';
      }
      const active = !result.status.error && ['RUNNING', 'PENDING', 'QUEUED'].includes(this.diagnosticsState);
      this.diagnosticsRetries = active ? 0 : this.diagnosticsRetries + 1;
      if (result.status.error && data) { this.diagnosticsNotice = 'profile.live.statusUnavailable'; }
      const retry = !this.diagnosticsFinal && (active || this.diagnosticsRetries < 6);
      if (!this.diagnosticsFinal && !retry) { this.diagnosticsNotice = 'profile.live.unavailable'; }
      this.diagnosticsRefreshing = false;
      this.analysisLoading = false;
      this.profileDetailLoading = false;
      this.cdr.markForCheck();
      if (retry && this.diagnosticsAutoRefresh && this.authService.isAuthenticated()) {
        this.diagnosticsTimer = setTimeout(() => this.loadAnalysis(queryId), 5000);
      }
    });
  }

  refreshAnalysis(): void {
    if (this.currentQueryId) {
      this.diagnosticsRetries = 0;
      this.loadAnalysis(this.currentQueryId);
    }
  }

  private getIntersectionPoint(
    from: { x: number, y: number },
    target: { x: number, y: number, width: number, height: number }
  ): { x: number, y: number } {
    const dx = from.x - target.x;
    const dy = from.y - target.y;
    if (dx === 0 && dy === 0) {
      return { ...target };
    }

    const halfW = target.width / 2;
    const halfH = target.height / 2;
    const tx = dx !== 0 ? halfW / Math.abs(dx) : Infinity;
    const ty = dy !== 0 ? halfH / Math.abs(dy) : Infinity;
    const t = Math.min(tx, ty);

    return {
      x: target.x + t * dx,
      y: target.y + t * dy,
    };
  }

  // Build DAG graph using dagre
  buildGraph(tree: any, preserveView = false): void {
    const g = new dagre.graphlib.Graph();
    g.setGraph({ 
      rankdir: this.graphDirection,
      marginx: 40, 
      marginy: 40,
      nodesep: 80,  // Increase node separation
      ranksep: 100  // Increase rank separation
    });
    g.setDefaultEdgeLabel(() => ({}));

    // Create a shallow copy of nodes array to avoid mutating the original data
    const nodeList = [...(tree.nodes || [])];
    if (tree.root) {
      // Check if root is already in the list to avoid duplicates
      if (!nodeList.find((n: any) => n.id === tree.root.id)) {
        nodeList.unshift(tree.root);
      }
    }

    console.log('Building graph with nodes:', nodeList.length);

    // Add nodes
    nodeList.forEach((node: any) => {
      // Node height: Header(35) + Body(55) + Progress(3) ≈ 93px
      // Use 95px for dagre layout to ensure proper spacing
      g.setNode(node.id, { width: 220, height: 95 });
    });

    // Add edges
    nodeList.forEach((node: any) => {
      if (node.children) {
        node.children.forEach((childId: string) => {
          g.setEdge(childId, node.id);
        });
      }
    });

    // Calculate layout
    dagre.layout(g);

    console.log('Graph layout complete:', {
      width: g.graph().width,
      height: g.graph().height,
      nodes: g.nodes().length,
      edges: g.edges().length
    });

    // Extract coordinates
    this.graphNodes = nodeList.map((node: any) => {
      const layoutNode = g.node(node.id);
      
      // Calculate coordinates for sanitized output
      const x = (layoutNode && isFinite(layoutNode.x)) ? layoutNode.x : 0;
      const y = (layoutNode && isFinite(layoutNode.y)) ? layoutNode.y : 0;
      const width = (layoutNode && isFinite(layoutNode.width)) ? layoutNode.width : 220;
      const height = (layoutNode && isFinite(layoutNode.height)) ? layoutNode.height : 95;

      return {
        ...node,
        x,
        y,
        width,
        height
      };
    });

    // First pass: collect all rows values to determine max for stroke width calculation
    const allRows = nodeList.map((n: any) => n.rows || 0).filter((r: number) => r > 0);
    const maxRows = allRows.length > 0 ? Math.max(...allRows) : 1;

    this.graphEdges = g.edges().map((e: any, index: number) => {
      const edge = g.edge(e);
      const sourceNodeData = nodeList.find((n: any) => n.id === e.v);
      const sourceNode = this.graphNodes.find((n: any) => n.id === e.v);
      const targetNode = this.graphNodes.find((n: any) => n.id === e.w);
      const rows = sourceNodeData?.rows || 0;

      let labelFormatted = rows > 0 ? 'Rows: ' + Number(rows).toLocaleString() : '';

      if (targetNode && (
          targetNode.operator_name.includes('JOIN') ||
          targetNode.operator_name === 'NESTLOOP_JOIN' ||
          targetNode.operator_name === 'HASH_JOIN'
        ) && targetNode.children && targetNode.children.length >= 2) {
        if (targetNode.children[0] === e.v) {
          labelFormatted += ' (PROBE)';
        } else if (targetNode.children[1] === e.v) {
          labelFormatted += ' (BUILD)';
        }
      }

      // Calculate dynamic stroke width based on rows (Algorithm: Logarithmic Scale)
      // Use Log scale because rows can vary from 0 to billions. Linear scale makes small diffs invisible.
      const minStrokeWidth = 1.5;
      const maxStrokeWidth = 6;
      
      let strokeWidth = minStrokeWidth;
      if (rows > 0 && maxRows > 0) {
        const logRows = Math.log10(rows + 1); // +1 to avoid log(0)
        const logMax = Math.log10(maxRows + 1);
        // Calculate ratio based on orders of magnitude
        const ratio = logMax > 0 ? logRows / logMax : 0;
        strokeWidth = minStrokeWidth + ratio * (maxStrokeWidth - minStrokeWidth);
      }

      // Calculate target position for multi-child nodes (Algorithm: Center Distribution)
      // Instead of spreading across the full width, distribute from center with fixed spacing
      // This looks better and more "connected"
      let targetX = targetNode?.x || 0;
      if (targetNode && targetNode.children && targetNode.children.length > 1) {
        const childIndex = targetNode.children.indexOf(e.v);
        const numChildren = targetNode.children.length;
        const spacing = 40; // 40px spacing between connection points
        
        // Calculate offset from center
        // e.g. 2 children: -20, +20
        // e.g. 3 children: -40, 0, +40
        const centerOffset = (childIndex - (numChildren - 1) / 2) * spacing;
        targetX = targetNode.x + centerOffset;
      }

      // Calculate visible line segment
      let visibleStart = { x: 0, y: 0 };
      let visibleEnd = { x: 0, y: 0 };
      
      if (sourceNode && targetNode) {
        // Source: arrow starts from node's TOP edge
        visibleStart = {
          x: sourceNode.x,
          y: sourceNode.y - sourceNode.height / 2
        };
        // Target: arrow ends at node's BOTTOM edge
        // Use dagre height/2 as base, will be fine-tuned by updateEdgesAfterRender()
        visibleEnd = {
          x: targetX,
          y: targetNode.y + targetNode.height / 2
        };
      }

      // Define points for the line
      const displayPoints = [visibleStart, visibleEnd];

      // Label position: geometric middle of visible segment
      const labelPos = {
        x: (visibleStart.x + visibleEnd.x) / 2,
        y: (visibleStart.y + visibleEnd.y) / 2
      };

      // Determine stroke color based on target node type
      let strokeColor = '#bfbfbf';
      if (targetNode) {
        const name = targetNode.operator_name?.toUpperCase() || '';
        if (name.includes('SCAN') || name.includes('JOIN')) {
          strokeColor = '#fa8c16';
        }
      }

      // Calculate arrow size based on stroke width
      const arrowSize = Math.max(8, strokeWidth * 2);

      return {
        v: e.v,
        w: e.w,
        points: displayPoints,
        markerId: `edge-marker-${e.v}-${e.w}-${index}`,
        labelPos,
        strokeColor,
        strokeWidth,
        arrowSize,
        label: rows,
        labelFormatted,
      };
    });
    
    // Calculate bounding box
    const maxX = Math.max(...this.graphNodes.map((n: any) => (n.x || 0) + (n.width || 0)/2));
    const maxY = Math.max(...this.graphNodes.map((n: any) => (n.y || 0) + (n.height || 0)/2));
    this.graphWidth = Math.max(maxX + 50, 600);
    this.graphHeight = Math.max(maxY + 50, 400);
    
    // Force change detection to update view
    this.cdr.markForCheck();
    
    // DEBUG: Log final results
    console.log('=== 构建完成 ===');
    console.log('graphNodes 数量:', this.graphNodes.length);
    console.log('graphEdges 数量:', this.graphEdges.length);
    console.log('图表尺寸:', this.graphWidth, 'x', this.graphHeight);
    if (this.graphNodes.length > 0) {
      console.log('第一个节点:', {
        id: this.graphNodes[0].id,
        operator_name: this.graphNodes[0].operator_name,
        x: this.graphNodes[0].x,
        y: this.graphNodes[0].y,
        width: this.graphNodes[0].width,
        height: this.graphNodes[0].height
      });
    }
    console.log('=============');
    
    // Calculate node ranks for color coding
    this.calculateNodeRanks();
    
    // Center the graph after layout, then update edges based on actual DOM heights
    setTimeout(() => {
      if (!preserveView) { this.resetZoom(); }
      this.updateEdgesAfterRender();
    });
  }
  
  // Update edge positions after DOM has rendered (measure actual node heights)
  private updateEdgesAfterRender(): void {
    if (this.graphDirection !== 'BT') { return; }
    const nodeElements = document.querySelectorAll('.dag-node');
    if (!nodeElements.length) return;
    
    // Build a map of actual DOM heights
    const actualHeights: Map<string, number> = new Map();
    nodeElements.forEach((el: Element) => {
      const nodeId = el.getAttribute('data-node-id');
      if (nodeId) {
        actualHeights.set(nodeId, el.getBoundingClientRect().height / this.zoomLevel);
      }
    });
    
    // Update graphNodes with actual heights
    this.graphNodes.forEach(node => {
      const actualH = actualHeights.get(node.id);
      if (actualH && actualH > 0) {
        node.actualHeight = actualH;
      }
    });
    
    // Recalculate edge endpoints using actual heights
    this.graphEdges = this.graphEdges.map(edge => {
      const sourceNode = this.graphNodes.find(n => n.id === edge.v);
      const targetNode = this.graphNodes.find(n => n.id === edge.w);
      
      if (sourceNode && targetNode) {
        const sourceActualH = sourceNode.actualHeight || sourceNode.height;
        const targetActualH = targetNode.actualHeight || targetNode.height;
        
        // DOM node is positioned at: top = node.y - dagreHeight/2
        // So actual TOP edge = node.y - dagreHeight/2
        // And actual BOTTOM edge = (node.y - dagreHeight/2) + actualDOMHeight
        
        // Source node: arrow starts from its TOP edge
        const newStartY = sourceNode.y - sourceNode.height / 2;
        // Target node: arrow ends at its BOTTOM edge
        // BOTTOM = (node.y - dagreHeight/2) + actualDOMHeight
        const newEndY = (targetNode.y - targetNode.height / 2) + targetActualH;
        
        // Update points
        if (edge.points && edge.points.length >= 2) {
          edge.points[0] = { x: edge.points[0].x, y: newStartY };
          const last = edge.points.length - 1;
          edge.points[last] = { x: edge.points[last].x, y: newEndY };
        }
        
        // Update label position (middle of line)
        edge.labelPos = {
          x: (edge.points[0].x + edge.points[1].x) / 2,
          y: (newStartY + newEndY) / 2
        };
      }
      
      return edge;
    });
    console.log('Edges updated with actual DOM heights');
    this.cdr.markForCheck();
  }
  
  // Center the graph in the viewport
  centerGraph(): void {
    const viewport = document.querySelector('.dag-center-panel') as HTMLElement;
    if (viewport && this.graphWidth > 0 && this.graphHeight > 0) {
      const vw = viewport.clientWidth;
      const vh = viewport.clientHeight;
      // Calculate center
      this.translateX = (vw - this.graphWidth * this.zoomLevel) / 2;
      this.translateY = (vh - this.graphHeight * this.zoomLevel) / 2;
      // Ensure some padding top if it's too high
      if (this.translateY < 20) this.translateY = 20;
      if (this.translateX < 20) this.translateX = 20;
    }
  }
  
  // Select a node
  selectNode(node: any, event?: Event): void {
    if (event) {
      event.stopPropagation(); // Prevent panel click from clearing selection
    }
    this.selectedNode = node;
  }
  
  // Handle click on DAG panel background (clear selection)
  onDagPanelClick(event: Event): void {
    // Only clear if clicking on the panel itself, not on nodes
    const target = event.target as HTMLElement;
    if (!target.closest('.dag-node') && !target.closest('.dag-toolbar')) {
      this.clearSelectedNode();
    }
  }
  
  // Select node by ID (for top10 table click)
  selectNodeById(nodeId: string): void {
    const node = this.graphNodes.find(n => n.id === nodeId);
    if (node) {
      this.selectedNode = node;
      const viewport = document.querySelector('.dag-center-panel') as HTMLElement;
      if (viewport) {
        this.zoomLevel = Math.max(this.zoomLevel, 0.8);
        this.translateX = viewport.clientWidth / 2 - node.x * this.zoomLevel;
        this.translateY = viewport.clientHeight / 2 - node.y * this.zoomLevel;
      }
    }
  }
  
  // Clear selected node (return to summary view)
  clearSelectedNode(): void {
    this.selectedNode = null;
  }
  
  // Get top 10 nodes by time percentage (include all nodes, even 0%)
  get top10NodesByTime(): any[] {
    if (!this.graphNodes || this.graphNodes.length === 0) return [];
    return [...this.graphNodes]
      .filter(n => n.time_percentage !== undefined && n.time_percentage !== null)
      .sort((a, b) => (b.time_percentage || 0) - (a.time_percentage || 0))
      .slice(0, 10);
  }
  
  // Get top 10 nodes by memory usage (show all nodes, even with null/0 memory)
  get top10NodesByMemory(): any[] {
    if (!this.graphNodes || this.graphNodes.length === 0) return [];
    return [...this.graphNodes]
      .sort((a, b) => (b.metrics?.memory_usage || 0) - (a.metrics?.memory_usage || 0))
      .slice(0, 10);
  }
  
  // Calculate scan time percentage
  getScanTimePercentage(): number {
    const scanMs = this.analysisData?.summary?.query_cumulative_scan_time_ms || 0;
    const totalMs = this.analysisData?.summary?.query_cumulative_operator_time_ms || 
                    this.analysisData?.summary?.query_execution_wall_time_ms || 1;
    return totalMs > 0 ? Math.min((scanMs / totalMs) * 100, 100) : 0;
  }
  
  // Calculate CPU time percentage
  getCpuTimePercentage(): number {
    const cpuMs = this.analysisData?.summary?.query_cumulative_cpu_time_ms || 0;
    const totalMs = this.analysisData?.summary?.query_cumulative_operator_time_ms || 
                    this.analysisData?.summary?.query_execution_wall_time_ms || 1;
    return totalMs > 0 ? Math.min((cpuMs / totalMs) * 100, 100) : 0;
  }

  // Calculate schedule time percentage
  getScheduleTimePercentage(): number {
    const scheduleMs = this.analysisData?.summary?.query_peak_schedule_time_ms || 0;
    const totalMs = this.analysisData?.summary?.query_execution_wall_time_ms || 1;
    return totalMs > 0 ? Math.min((scheduleMs / totalMs) * 100, 100) : 0;
  }

  // Calculate network time percentage
  getNetworkTimePercentage(): number {
    const networkMs = this.analysisData?.summary?.query_cumulative_network_time_ms || 0;
    const totalMs = this.analysisData?.summary?.query_execution_wall_time_ms || 1;
    return totalMs > 0 ? Math.min((networkMs / totalMs) * 100, 100) : 0;
  }

  // Calculate result deliver time percentage
  getResultDeliverTimePercentage(): number {
    const deliverMs = this.analysisData?.summary?.result_deliver_time_ms || 0;
    const totalMs = this.analysisData?.summary?.query_execution_wall_time_ms || 1;
    return totalMs > 0 ? Math.min((deliverMs / totalMs) * 100, 100) : 0;
  }
  
  // Format time string with percentage
  formatTimeWithPercent(timeStr: string | undefined, percent: number): string {
    if (!timeStr) return '-';
    return `${percent.toFixed(1)}%(${timeStr})`;
  }

  // Check if IO metrics are available (for disaggregated storage)
  hasIoMetrics(): boolean {
    const s = this.analysisData?.summary;
    return !!(s?.io_seek_time || s?.local_disk_read_io_time || s?.remote_read_io_time);
  }

  // Check if IO statistics are available
  hasIoStatistics(): boolean {
    const s = this.analysisData?.summary;
    return !!(s?.total_raw_rows_read || s?.total_bytes_read || 
              s?.pages_count_memory || s?.pages_count_local_disk || s?.pages_count_remote ||
              s?.result_rows || s?.result_bytes);
  }

  // Scroll to diagnosis section
  scrollToDiagnosis(): void {
    this.isDiagnosisCollapsed = false;
    setTimeout(() => {
      const diagSection = document.querySelector('.diagnosis-section');
      if (diagSection) {
        diagSection.scrollIntoView({ behavior: 'smooth', block: 'start' });
      }
    }, 100);
  }
  
  // Check if any summary metric is available
  hasAnyMetric(): boolean {
    const s = this.analysisData?.summary;
    if (!s) return false;
    return !!(s.query_execution_wall_time || s.query_cumulative_operator_time ||
              s.query_cumulative_cpu_time || s.query_cumulative_scan_time ||
              s.query_cumulative_network_time || s.query_peak_schedule_time ||
              s.result_deliver_time || s.query_sum_memory_usage || s.query_spill_bytes ||
              s.datacache_hit_rate != null);
  }
  
  // Get DataCache tooltip with detailed metrics
  getDataCacheTooltip(): string {
    const s = this.analysisData?.summary;
    if (!s) return '';
    
    const local = s.datacache_bytes_local_display || '0 B';
    const remote = s.datacache_bytes_remote_display || '0 B';
    const hitRate = s.datacache_hit_rate != null ? (s.datacache_hit_rate * 100).toFixed(1) : '0';
    
    return this.translate.instant('profile.summary.dataCacheTooltip', { local, remote, hitRate });
  }
  
  // Get metric keys from metrics object (filter out null/undefined and specialized)
  getMetricKeys(metrics: any): string[] {
    if (!metrics) return [];
    return Object.keys(metrics).filter(key => {
      const val = metrics[key];
      return val !== null && val !== undefined && key !== 'specialized';
    });
  }
  
  // Format metric value based on key name
  formatMetricValue(key: string, value: any): string {
    if (value === null || value === undefined) return '-';
    const lowerKey = key.toLowerCase();
    // Time metrics (in nanoseconds)
    if (lowerKey.includes('time')) {
      return this.formatDurationNs(value);
    }
    // Memory/bytes metrics
    if (lowerKey.includes('memory') || lowerKey.includes('bytes')) {
      return this.formatBytes(value);
    }
    // Number metrics
    if (typeof value === 'number') {
      return value.toLocaleString();
    }
    return String(value);
  }

  // ============================================
  // Node Detail View Methods
  // ============================================

  // Get operator type from node name
  getOperatorType(node: any): string {
    if (!node?.operator_name) return 'DEFAULT';
    const name = node.operator_name.toUpperCase();
    if (name.includes('SCAN')) return 'SCAN';
    if (name.includes('JOIN')) return 'JOIN';
    if (name.includes('AGG')) return 'AGGREGATE';
    if (name.includes('EXCHANGE')) return 'EXCHANGE';
    if (name.includes('SORT') || name.includes('TOP')) return 'SORT';
    if (name.includes('PROJECT')) return 'PROJECT';
    return 'DEFAULT';
  }

  // Get execution time breakdown - dynamically detect from node's own metrics
  // If node has ScanTime -> show CPUTime + ScanTime
  // If node has NetworkTime -> show CPUTime + NetworkTime
  // Otherwise -> show CPUTime only (100%)
  getExecutionTimeBreakdown(): { items: { label: string; time: string; percent: number; color: string }[] } | null {
    if (!this.selectedNode || this.analysisData?.engine === 'doris') return null;
    
    const allMetrics = this.getAllNodeMetrics();
    const items: { label: string; time: string; percent: number; color: string }[] = [];
    
    // Get CPUTime from OperatorTotalTime
    const cpuTimeStr = allMetrics['__MAX_OF_OperatorTotalTime'] || allMetrics['OperatorTotalTime'] || '0ns';
    const cpuTimeNs = this.parseTimeToNs(cpuTimeStr);
    
    // Dynamically detect secondary time from the node's own metrics
    let secondaryTimeStr = '';
    let secondaryLabel = '';
    let secondaryTimeNs = 0;
    
    // Check what time metrics this node actually has
    if (allMetrics['ScanTime'] && this.parseTimeToNs(allMetrics['ScanTime']) > 0) {
      secondaryTimeStr = allMetrics['ScanTime'];
      secondaryLabel = 'ScanTime';
      secondaryTimeNs = this.parseTimeToNs(secondaryTimeStr);
    } else if (allMetrics['NetworkTime'] && this.parseTimeToNs(allMetrics['NetworkTime']) > 0) {
      secondaryTimeStr = allMetrics['NetworkTime'];
      secondaryLabel = 'NetworkTime';
      secondaryTimeNs = this.parseTimeToNs(secondaryTimeStr);
    }
    
    const totalTimeNs = cpuTimeNs + secondaryTimeNs || 1;
    
    // Add CPUTime
    items.push({
      label: 'CPUTime',
      time: cpuTimeStr,
      percent: secondaryTimeNs > 0 ? (cpuTimeNs / totalTimeNs) * 100 : 100,
      color: 'cpu'
    });
    
    // Add secondary time if this node has it
    if (secondaryLabel && secondaryTimeNs > 0) {
      items.push({
        label: secondaryLabel,
        time: secondaryTimeStr,
        percent: (secondaryTimeNs / totalTimeNs) * 100,
        color: 'secondary'
      });
    }
    
    return { items };
  }

  // Get all metrics from node (combining all sources)
  private getAllNodeMetrics(): { [key: string]: any } {
    if (!this.selectedNode) return {};
    
    // Combine metrics from node itself
    const result: { [key: string]: any } = {
      ...this.selectedNode.metrics,
      ...this.selectedNode.unique_metrics,
      ...this.selectedNode.aggregated_metrics
    };
    
    // Also get metrics from operators in pipeline
    const operators = this.getNodeOperatorsFromPipeline();
    for (const op of operators) {
      if (op.common_metrics) {
        Object.assign(result, op.common_metrics);
      }
      if (op.unique_metrics) {
        Object.assign(result, op.unique_metrics);
      }
    }
    
    return result;
  }

  // Get core metrics - directly from node's unique_metrics (no hardcoding!)
  // Filter out: time metrics (shown in Over Consuming), memory metrics (shown in Memory section), index metrics (shown in Indexes)
  getNodeCoreMetrics(): { key: string; value: any }[] {
    if (!this.selectedNode) return [];
    
    const allMetrics = this.getAllNodeMetrics();
    const result: { key: string; value: any }[] = [];
    
    // Keys to exclude (shown in other sections)
    const excludePatterns = [
      /Time$/i, /^__/, /Memory/i, /Peak/i, /FilterRows$/i,
      /^OperatorTotalTime$/, /^PullTotalTime$/, /^PushTotalTime$/
    ];
    
    // Priority keys to show first (common important metrics)
    const priorityKeys = [
      'RowsRead', 'BytesRead', 'CompressedBytesRead', 'ScanTime', 'IOTime',
      'Table', 'Rollup', 'Predicates',
      'BytesSent', 'BytesReceived', 'NetworkTime', 'WaitTime', 'OverallThroughput', 'PartType',
      'PullRowNum', 'PushRowNum', 'OutputRows'
    ];
    
    // First add priority keys that exist
    for (const key of priorityKeys) {
      const value = allMetrics[key];
      if (value !== undefined && value !== null && value !== '' && value !== '0' && value !== '0.000 B') {
        // Skip if matches exclude pattern
        if (!excludePatterns.some(p => p.test(key))) {
          result.push({ key, value });
        }
      }
    }
    
    // Then add other unique_metrics that weren't added
    const addedKeys = new Set(result.map(r => r.key));
    for (const [key, value] of Object.entries(allMetrics)) {
      if (addedKeys.has(key)) continue;
      if (value === undefined || value === null || value === '') continue;
      if (excludePatterns.some(p => p.test(key))) continue;
      
      // Only add non-zero values
      if (value !== '0' && value !== '0.000 B' && value !== 0) {
        result.push({ key, value });
      }
    }
    
    return result;
  }

  // Get memory metrics - dynamically find all memory-related metrics from node
  getNodeMemoryMetrics(): { key: string; value: any }[] {
    if (!this.selectedNode) return [];
    
    const allMetrics = this.getAllNodeMetrics();
    const result: { key: string; value: any }[] = [];
    
    // Find all metrics containing Memory or Peak (memory-related)
    for (const [key, value] of Object.entries(allMetrics)) {
      if (key.startsWith('__')) continue; // Skip aggregation keys
      if (!/Memory|Peak/i.test(key)) continue;
      if (value === undefined || value === null || value === '') continue;
      if (value === '0' || value === '0.000 B' || value === 0 || value === '0Bytes') continue;
      
      result.push({ key, value });
    }
    
    return result;
  }

  // Get index metrics - dynamically find all *FilterRows metrics from node
  getNodeIndexMetrics(): { key: string; value: any }[] {
    if (!this.selectedNode) return [];
    
    const allMetrics = this.getAllNodeMetrics();
    const result: { key: string; value: any }[] = [];
    
    // Index display name mapping
    const indexMapping: { [key: string]: string } = {
      'ShortKeyFilterRows': 'ShortKey',
      'ZoneMapIndexFilterRows': 'ZoneMap',
      'SegmentZoneMapFilterRows': 'ZoneMap',
      'SegmentRuntimeZoneMapFilterRows': 'RuntimeZoneMap',
      'BitmapIndexFilterRows': 'Bitmap',
      'BloomFilterFilterRows': 'Bloom'
    };
    
    // Find all FilterRows metrics
    for (const [sourceKey, displayKey] of Object.entries(indexMapping)) {
      const value = allMetrics[sourceKey];
      if (value !== undefined) {
        result.push({ key: displayKey, value });
      }
    }
    
    return result;
  }

  // Get over consuming metrics - dynamically find all time-related metrics from node
  // Format: "value[max=xxx, min=xxx]" like official UI
  getOverConsumingMetrics(): { key: string; value: string; indent?: number }[] {
    if (!this.selectedNode) return [];
    
    const allMetrics = this.getAllNodeMetrics();
    const result: { key: string; value: string; indent?: number }[] = [];
    
    // Time pattern: ends with Time or contains time units (ns/us/ms/s/m/h)
    const isTimeMetric = (key: string, value: any): boolean => {
      if (key.startsWith('__')) return false;
      if (!/Time$/i.test(key)) return false;
      if (key === 'OperatorTotalTime') return false; // Already shown in Execution time
      return true;
    };
    
    // Collect all time metrics from this node
    const timeMetrics: { key: string; value: string; timeNs: number }[] = [];
    
    for (const [key, value] of Object.entries(allMetrics)) {
      if (!isTimeMetric(key, value)) continue;
      if (value === undefined || value === null || value === '') continue;
      
      const timeNs = this.parseTimeToNs(value);
      if (timeNs <= 0) continue;
      
      // Build display value with max/min if available
      const maxVal = allMetrics[`__MAX_OF_${key}`];
      const minVal = allMetrics[`__MIN_OF_${key}`];
      let displayValue = String(value);
      if (maxVal && minVal) {
        displayValue = `${value}[max=${maxVal}, min=${minVal}]`;
      }
      
      timeMetrics.push({ key, value: displayValue, timeNs });
    }
    
    // Sort by time (descending) - show most time-consuming first
    timeMetrics.sort((a, b) => b.timeNs - a.timeNs);
    
    // Add to result
    for (const metric of timeMetrics) {
      result.push({ key: metric.key, value: metric.value, indent: 0 });
    }
    
    return result;
  }

  // Get all metrics for node detail view
  getNodeDetailMetrics(): { category: string; metrics: { key: string; value: any; description?: string }[] }[] {
    if (!this.selectedNode) return [];
    
    const categories: { category: string; metrics: { key: string; value: any; description?: string }[] }[] = [];
    
    // Unique metrics (non-aggregated)
    if (this.selectedNode.unique_metrics && Object.keys(this.selectedNode.unique_metrics).length > 0) {
      const metrics = Object.entries(this.selectedNode.unique_metrics)
        .filter(([_, v]) => v !== null && v !== undefined)
        .map(([key, value]) => ({
          key,
          value,
          description: this.getExactMetricDescription(key)
        }));
      if (metrics.length > 0) {
        categories.push({ category: this.translate.instant('profile.node.uniqueMetrics'), metrics });
      }
    }
    
    // Aggregated metrics (with min/max/avg)
    if (this.selectedNode.aggregated_metrics && Object.keys(this.selectedNode.aggregated_metrics).length > 0) {
      const metrics = Object.entries(this.selectedNode.aggregated_metrics)
        .filter(([_, v]) => v !== null && v !== undefined)
        .map(([key, value]) => ({
          key,
          value,
          description: this.getExactMetricDescription(key)
        }));
      if (metrics.length > 0) {
        categories.push({ category: this.translate.instant('profile.node.aggregatedMetrics'), metrics });
      }
    }
    
    return categories;
  }

  // Get pipeline info for selected node
  getNodePipelineInfo(): any[] {
    if (!this.selectedNode) return [];
    
    // Pipeline info is typically in the node's children or specialized metrics
    const pipelineMetrics = [
      'DriverTotalTime', 'ActiveTime', 'PendingTime', 'ScheduleTime',
      'InputEmptyTime', 'OutputFullTime', 'PreconditionBlockTime',
      'LocalRfWaitingTime', 'PipelineID', 'DriverNum'
    ];
    
    const result: { key: string; value: any }[] = [];
    const allMetrics = {
      ...this.selectedNode.unique_metrics,
      ...this.selectedNode.aggregated_metrics
    };
    
    for (const key of pipelineMetrics) {
      if (allMetrics[key] !== undefined && allMetrics[key] !== null) {
        result.push({ key, value: allMetrics[key] });
      }
    }
    
    return result;
  }

  // Parse time string to nanoseconds (e.g., "84.535us" -> 84535, "1s183ms" -> 1183000000)
  private parseTimeToNs(timeStr: any): number {
    if (!timeStr) return 0;
    if (typeof timeStr === 'number') return timeStr;
    
    const str = String(timeStr).trim();
    let totalNs = 0;
    
    // Match patterns like "1h30m", "1s183ms", "84.535us", "0ns"
    const patterns = [
      { regex: /([\d.]+)h/i, multiplier: 3600000000000 },
      { regex: /([\d.]+)m(?!s)/i, multiplier: 60000000000 },
      { regex: /([\d.]+)s(?!$)/i, multiplier: 1000000000 },
      { regex: /([\d.]+)ms/i, multiplier: 1000000 },
      { regex: /([\d.]+)us/i, multiplier: 1000 },
      { regex: /([\d.]+)ns/i, multiplier: 1 },
      { regex: /^([\d.]+)s$/i, multiplier: 1000000000 }, // Just seconds
    ];
    
    for (const { regex, multiplier } of patterns) {
      const match = str.match(regex);
      if (match) {
        totalNs += parseFloat(match[1]) * multiplier;
      }
    }
    
    return totalNs;
  }

  // Calculate IO percentage for node (IO time / Total time)
  getNodeIoPercentage(): number {
    if (!this.selectedNode) return 0;
    
    // Get IOTime from various sources
    const ioTimeStr = this.selectedNode.aggregated_metrics?.IOTime || 
                      this.selectedNode.unique_metrics?.IOTime ||
                      this.selectedNode.metrics?.IOTime || 0;
    const ioTimeNs = this.parseTimeToNs(ioTimeStr);
    
    // Get total operator time
    const totalTimeNs = this.selectedNode.metrics?.operator_total_time_ns || 
                        this.parseTimeToNs(this.selectedNode.metrics?.operator_total_time) || 1;
    
    return totalTimeNs > 0 ? Math.min((ioTimeNs / totalTimeNs) * 100, 100) : 0;
  }

  // Calculate Processing percentage for node (100% - IO%)
  getNodeProcessingPercentage(): number {
    if (!this.selectedNode) return 0;
    const ioPercent = this.getNodeIoPercentage();
    return Math.max(100 - ioPercent, 0);
  }

  // Clear node selection
  clearNodeSelection(): void {
    this.selectedNode = null;
    this.selectedNodeTab = 'core';
  }

  // Select node and switch to detail view
  selectNodeForDetail(node: any): void {
    this.selectedNode = node;
    this.selectedNodeTab = 'core';
  }

  // Get Fragment info for selected node
  getNodeFragmentInfo(): any | null {
    if (!this.selectedNode?.fragment_id || !this.analysisData?.fragments) return null;
    return this.analysisData.fragments.find((f: any) => f.id === this.selectedNode.fragment_id);
  }

  // Get Fragment metrics (excluding pipelines and operators)
  getFragmentMetrics(): { key: string; value: any }[] {
    const fragment = this.getNodeFragmentInfo();
    if (!fragment) return [];
    
    const result: { key: string; value: any }[] = [];
    // Add basic fragment info
    if (fragment.backend_addresses?.length) {
      result.push({ key: 'BackendAddresses', value: fragment.backend_addresses.join(', ') });
    }
    if (fragment.instance_ids?.length) {
      result.push({ key: 'InstanceNum', value: fragment.instance_ids.length });
    }
    return result;
  }

  // Get Pipeline info for selected node from fragments
  getNodePipelineFromFragments(): any | null {
    const fragment = this.getNodeFragmentInfo();
    if (!fragment || !this.selectedNode?.pipeline_id) return null;
    return fragment.pipelines?.find((p: any) => p.id === this.selectedNode.pipeline_id);
  }

  // Get Pipeline metrics
  getPipelineMetrics(): { key: string; value: any }[] {
    const pipeline = this.getNodePipelineFromFragments();
    if (!pipeline?.metrics) return [];
    
    return Object.entries(pipeline.metrics)
      .filter(([_, v]) => v !== null && v !== undefined)
      .map(([key, value]) => ({ key, value }));
  }

  // Get operators for selected node from pipeline (for 节点详情 Tab)
  getNodeOperatorsFromPipeline(): any[] {
    const pipeline = this.getNodePipelineFromFragments();
    if (!pipeline?.operators) return [];

    if (this.analysisData?.engine === 'doris') {
      return pipeline.operators.filter((op: any) => op.operator_id === this.selectedNode?.id);
    }
    
    const planNodeId = this.selectedNode?.plan_node_id?.toString();
    if (!planNodeId) return pipeline.operators;
    
    // Filter operators by plan_node_id
    return pipeline.operators.filter((op: any) => op.plan_node_id === planNodeId);
  }

  // Get all operators in the pipeline (for full view)
  getAllPipelineOperators(): any[] {
    const pipeline = this.getNodePipelineFromFragments();
    return pipeline?.operators || [];
  }
  
  // Format bytes to human readable
  formatBytes(bytes: any): string {
    if (bytes === null || bytes === undefined) return '—';
    const val = Number(bytes);
    if (isNaN(val)) return String(bytes);
    if (val < 1024) return val + ' B';
    if (val < 1024 * 1024) return (val / 1024).toFixed(2) + ' KB';
    if (val < 1024 * 1024 * 1024) return (val / (1024 * 1024)).toFixed(2) + ' MB';
    return (val / (1024 * 1024 * 1024)).toFixed(2) + ' GB';
  }

  // Get diagnostics for a specific node by plan_node_id
  getNodeDiagnostics(node: any): any[] {
    if (this.analysisData?.operator_diagnostics?.[node?.id]) {
      return this.analysisData.operator_diagnostics[node.id];
    }
    // Note: plan_node_id can be 0, so use explicit null/undefined check
    if (node?.plan_node_id == null || !this.analysisData?.node_diagnostics) {
      return [];
    }
    return this.analysisData.node_diagnostics[node.plan_node_id] || [];
  }

  // Get current diagnostics count based on context (node-specific or aggregated)
  getCurrentDiagnosticsCount(): number {
    if (this.selectedNode) {
      return this.getNodeDiagnostics(this.selectedNode)?.length || 0;
    }
    return this.analysisData?.aggregated_diagnostics?.length || 0;
  }
  
  // Get edge path for SVG
  getEdgePath(points: {x: number, y: number}[]): string {
    if (!points || points.length === 0) return '';
    return 'M' + points.map(p => `${p.x},${p.y}`).join(' L');
  }

  // Get edge label position (geometric middle of the path)
  getEdgeLabelPosition(points: {x: number, y: number}[]): {x: number, y: number} {
    if (!points || points.length < 2) return { x: 0, y: 0 };
    
    // Calculate total length
    let totalLength = 0;
    const segments = [];
    for (let i = 0; i < points.length - 1; i++) {
      const dx = points[i+1].x - points[i].x;
      const dy = points[i+1].y - points[i].y;
      const dist = Math.sqrt(dx*dx + dy*dy);
      segments.push({ dist, p1: points[i], p2: points[i+1] });
      totalLength += dist;
    }
    
    let targetLen = totalLength / 2;
    let currentLen = 0;
    
    for (const seg of segments) {
      if (currentLen + seg.dist >= targetLen) {
        // Found the segment
        const remaining = targetLen - currentLen;
        const ratio = remaining / seg.dist;
        return {
          x: seg.p1.x + (seg.p2.x - seg.p1.x) * ratio,
          y: seg.p1.y + (seg.p2.y - seg.p1.y) * ratio,
        };
      }
      currentLen += seg.dist;
    }
    
    // Fallback
    const mid = Math.floor(points.length / 2);
    return { x: points[mid].x, y: points[mid].y };
  }

  getLabelTransform(points: {x: number, y: number}[]): string {
    const pos = this.getEdgeLabelPosition(points);
    return `translate(${pos.x}, ${pos.y})`;
  }

  // Zoom controls
  zoomIn(): void {
    this.zoomLevel = Math.min(this.zoomLevel + 0.1, 3);
  }

  zoomOut(): void {
    this.zoomLevel = Math.max(this.zoomLevel - 0.1, 0.2);
  }

  resetZoom(): void {
    const viewport = document.querySelector('.dag-center-panel') as HTMLElement;
    if (!viewport || !this.graphWidth || !this.graphHeight) { return; }
    this.zoomLevel = Math.max(0.2, Math.min(1, (viewport.clientWidth - 48) / this.graphWidth, (viewport.clientHeight - 48) / this.graphHeight));
    this.centerGraph();
  }
  
  toggleFullscreen(): void {
    this.isFullscreen = !this.isFullscreen;
    setTimeout(() => {
        // Trigger resize event to re-layout if needed
        window.dispatchEvent(new Event('resize'));
    });
  }

  // Toggle graph direction (BT <-> LR)
  toggleGraphDirection(): void {
    this.graphDirection = this.graphDirection === 'BT' ? 'LR' : 'BT';
    // Rebuild graph with new direction
    if (this.analysisData?.execution_tree) {
      this.buildGraph(this.analysisData.execution_tree);
    }
  }

  // Export DAG as PNG image
  exportDagAsPng(): void {
    const graphContent = document.querySelector('.graph-content') as HTMLElement;
    if (!graphContent) {
      this.toastrService.warning(
        this.translate.instant('profile.export.graphNotFound'),
        this.translate.instant('profile.export.failedTitle'),
      );
      return;
    }

    // Use html2canvas to capture the graph
    import('html2canvas').then(html2canvasModule => {
      const html2canvas = html2canvasModule.default;
      html2canvas(graphContent, {
        backgroundColor: '#ffffff',
        scale: 2, // Higher resolution
        logging: false,
        useCORS: true,
      }).then(canvas => {
        // Create download link
        const link = document.createElement('a');
        link.download = `profile-dag-${this.currentQueryId || 'export'}.png`;
        link.href = canvas.toDataURL('image/png');
        link.click();
        this.toastrService.success(
          this.translate.instant('profile.export.success'),
          this.translate.instant('profile.export.successTitle'),
        );
      }).catch(err => {
        console.error('Export failed:', err);
        this.toastrService.danger(
          this.translate.instant('profile.export.imageFailed'),
          this.translate.instant('profile.errorTitle'),
        );
      });
    }).catch(err => {
      console.error('Failed to load html2canvas:', err);
      this.toastrService.danger(
        this.translate.instant('profile.export.moduleLoadFailed'),
        this.translate.instant('profile.errorTitle'),
      );
    });
  }

  // Panning
  startPan(event: MouseEvent): void {
    // Prevent pan if resizing right panel
    if (this.isResizingRight) return;
    
    // Check if click target is a node or button
    const target = event.target as HTMLElement;
    if (target.closest('.dag-node') || target.closest('button')) return;

    if (event.button === 0) {
      this.isPanning = true;
      this.startX = event.clientX - this.translateX;
      this.startY = event.clientY - this.translateY;
      const viewport = document.querySelector('.dag-center-panel') as HTMLElement; // Use panel cursor
      if (viewport) viewport.style.cursor = 'grabbing';
    }
  }
  
  pan(event: MouseEvent): void {
    if (this.isPanning) {
      event.preventDefault();
      this.translateX = event.clientX - this.startX;
      this.translateY = event.clientY - this.startY;
    }
    // Note: resize handling is now done in startResizeRight() with dedicated listeners
  }
  
  endPan(): void {
    this.isPanning = false;
    this.isResizingRight = false;
    const viewport = document.querySelector('.dag-center-panel') as HTMLElement;
    if (viewport) {
        viewport.style.cursor = 'default';
    }
    document.body.style.cursor = 'default';
  }
  
  // Right Panel Resizing & Toggle
  toggleRightPanel(): void {
    this.isRightPanelCollapsed = !this.isRightPanelCollapsed;
  }
  
  startResizeRight(event: MouseEvent): void {
    event.preventDefault();
    event.stopPropagation();
    this.isResizingRight = true;
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none'; // 防止选择文本
    
    // 记录起始位置和宽度
    const startX = event.clientX;
    const startWidth = this.rightPanelWidth;
    
    // Add global event listeners for reliable resize tracking
    const onMouseMove = (e: MouseEvent) => {
      if (this.isResizingRight) {
        e.preventDefault();
        e.stopPropagation();
        
        // 使用拖动距离来计算新宽度（更可靠）
        const deltaX = startX - e.clientX;
        const newWidth = startWidth + deltaX;
        
        if (newWidth > 200 && newWidth < 800) {
          this.rightPanelWidth = newWidth;
        }
      }
    };
    
    const onMouseUp = (e: MouseEvent) => {
      e.preventDefault();
      this.isResizingRight = false;
      document.body.style.cursor = 'default';
      document.body.style.userSelect = '';
      document.removeEventListener('mousemove', onMouseMove, true);
      document.removeEventListener('mouseup', onMouseUp, true);
    };
    
    // 使用 capture 模式确保优先处理
    document.addEventListener('mousemove', onMouseMove, true);
    document.addEventListener('mouseup', onMouseUp, true);
  }
  
  // Mouse wheel zoom
  onWheel(event: WheelEvent): void {
    event.preventDefault();
    if (event.deltaY < 0) {
      this.zoomIn();
    } else {
      this.zoomOut();
    }
  }
  
  // Get node rank (1-3 for top 3 time-consuming nodes)
  getNodeRank(node: any): number {
    return this.nodeRankMap.get(node.id) || 0;
  }
  
  // Calculate node ranks based on time percentage
  private calculateNodeRanks(): void {
    this.nodeRankMap.clear();
    if (!this.graphNodes || this.graphNodes.length === 0) return;
    
    // Sort nodes by time_percentage descending
    const sorted = [...this.graphNodes]
      .filter(n => n.time_percentage > 0)
      .sort((a, b) => b.time_percentage - a.time_percentage);
    
    // Assign ranks to top 3
    for (let i = 0; i < Math.min(3, sorted.length); i++) {
      this.nodeRankMap.set(sorted[i].id, i + 1);
    }
  }
  
  // Format duration in nanoseconds
  formatDurationNs(ns: any): string {
    if (ns === null || ns === undefined || ns === '') return '—';
    const val = Number(ns);
    if (isNaN(val)) return ns;
    
    if (val < 1000) return val + 'ns';
    if (val < 1000000) return (val/1000).toFixed(2) + 'us';
    if (val < 1000000000) return (val/1000000).toFixed(2) + 'ms';
    
    // Convert to seconds
    const totalSeconds = val / 1000000000;
    if (totalSeconds < 60) return totalSeconds.toFixed(2) + 's';
    
    // Convert to human-readable format for larger durations
    const hours = Math.floor(totalSeconds / 3600);
    const minutes = Math.floor((totalSeconds % 3600) / 60);
    const seconds = Math.floor(totalSeconds % 60);
    const millis = Math.round((totalSeconds % 1) * 1000);
    
    let result = '';
    if (hours > 0) result += hours + 'h';
    if (minutes > 0) result += minutes + 'm';
    if (seconds > 0 || (hours === 0 && minutes === 0)) result += seconds + 's';
    if (millis > 0) result += millis + 'ms';
    
    return result || '0s';
  }

  // Fix for Angular base href issue with SVG markers
  get arrowMarkerUrl(): string {
    return `url(${this.location.path()}#dag-arrow)`;
  }

  // Get node header class based on operator type (Figure 1 style)
  getNodeHeaderClass(node: any): string {
    const name = node.operator_name?.toUpperCase() || '';
    if (this.getNodeRank(node) === 1) return 'header-red';
    if (name.includes('SCAN')) return 'header-orange';
    if (name.includes('JOIN')) return 'header-orange';
    if (name.includes('EXCHANGE')) return 'header-gray';
    if (name.includes('PROJECT')) return 'header-gray';
    if (name.includes('AGGREGATION')) return 'header-gray';
    return 'header-gray';
  }

  // Get progress bar color
  getProgressColor(node: any): string {
    const name = node.operator_name?.toUpperCase() || '';
    if (name.includes('SCAN')) return '#fa8c16';
    if (name.includes('JOIN')) return '#fa8c16';
    return '#d9d9d9';
  }

  // Toggle functions for right panel sections
  toggleTop10(): void { this.isTop10Collapsed = !this.isTop10Collapsed; }
  toggleSummary(): void { this.isSummaryCollapsed = !this.isSummaryCollapsed; }
  toggleDiagnosis(): void { this.isDiagnosisCollapsed = !this.isDiagnosisCollapsed; }
  toggleMemoryView(): void { 
    this.showMemoryView = !this.showMemoryView; 
  }

  // Copy profile content to clipboard
  copyProfileToClipboard(): void {
    if (!this.currentProfileDetail) {
      return;
    }

    // Use Clipboard API
    if (navigator.clipboard && window.isSecureContext) {
      navigator.clipboard.writeText(this.currentProfileDetail)
        .then(() => {
          this.toastrService.success(
            this.translate.instant('profile.copy.profileCopied'),
            this.translate.instant('profile.copy.successTitle'),
          );
        })
        .catch(err => {
          console.error('Failed to copy:', err);
          this.fallbackCopy();
        });
    } else {
      // Fallback for older browsers or non-secure contexts
      this.fallbackCopy();
    }
  }

  // Fallback copy method for older browsers
  private fallbackCopy(): void {
    const textArea = document.createElement('textarea');
    textArea.value = this.currentProfileDetail;
    textArea.style.position = 'fixed';
    textArea.style.left = '-999999px';
    document.body.appendChild(textArea);
    textArea.focus();
    textArea.select();

    try {
      const successful = document.execCommand('copy');
      if (successful) {
        this.toastrService.success(
          this.translate.instant('profile.copy.profileCopied'),
          this.translate.instant('profile.copy.successTitle'),
        );
      } else {
        this.toastrService.warning(
          this.translate.instant('profile.copy.failedManual'),
          this.translate.instant('profile.noticeTitle'),
        );
      }
    } catch (err) {
      console.error('Failed to copy:', err);
      this.toastrService.warning(
        this.translate.instant('profile.copy.failedManual'),
        this.translate.instant('profile.noticeTitle'),
      );
    } finally {
      document.body.removeChild(textArea);
    }
  }

  // Copy text to clipboard (for parameter commands)
  copyToClipboard(text: string): void {
    if (!text) return;
    
    if (navigator.clipboard && window.isSecureContext) {
      navigator.clipboard.writeText(text)
        .then(() => {
          this.toastrService.success(
            this.translate.instant('profile.copy.copied'),
            this.translate.instant('profile.copy.successTitle'),
          );
        })
        .catch(err => {
          console.error('Failed to copy:', err);
          this.toastrService.warning(
            this.translate.instant('profile.copy.failed'),
            this.translate.instant('profile.noticeTitle'),
          );
        });
    } else {
      // Fallback
      const textArea = document.createElement('textarea');
      textArea.value = text;
      textArea.style.position = 'fixed';
      textArea.style.left = '-999999px';
      document.body.appendChild(textArea);
      textArea.select();
      try {
        document.execCommand('copy');
        this.toastrService.success(
          this.translate.instant('profile.copy.copied'),
          this.translate.instant('profile.copy.successTitle'),
        );
      } catch (err) {
        this.toastrService.warning(
          this.translate.instant('profile.copy.failed'),
          this.translate.instant('profile.noticeTitle'),
        );
      } finally {
        document.body.removeChild(textArea);
      }
    }
  }
}
