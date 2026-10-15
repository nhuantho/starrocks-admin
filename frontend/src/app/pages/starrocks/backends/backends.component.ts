import { Component, OnInit, OnDestroy } from '@angular/core';
import { interval, Subject } from 'rxjs';
import { takeUntil, switchMap } from 'rxjs/operators';
import { NbToastrService } from '@nebular/theme';
import { LocalDataSource } from 'ng2-smart-table';
import { TranslateService } from '@ngx-translate/core';
import { NodeService, Backend } from '../../../@core/data/node.service';
import { ClusterService, Cluster } from '../../../@core/data/cluster.service';
import { ClusterContextService } from '../../../@core/data/cluster-context.service';
import { ErrorHandler } from '../../../@core/utils/error-handler';
import { ConfirmDialogService } from '../../../@core/services/confirm-dialog.service';
import { MetricThresholds, renderMetricBadge } from '../../../@core/utils/metric-badge';

@Component({
  selector: 'ngx-backends',
  templateUrl: './backends.component.html',
  styleUrls: ['./backends.component.scss'],
})
export class BackendsComponent implements OnInit, OnDestroy {
  source: LocalDataSource = new LocalDataSource();
  clusterId: number;
  activeCluster: Cluster | null = null;
  clusterName: string = '';
  deploymentMode: string = '';
  pageTitle: string = 'cluster.backends.title';
  deploymentModeText: string = '';
  deploymentModeBadgeClass: string = '';
  loading = true;
  private destroy$ = new Subject<void>();
  private readonly diskThresholds: MetricThresholds = { warn: 70, danger: 85 };
  private readonly cpuThresholds: MetricThresholds = { warn: 60, danger: 85 };
  private readonly memoryThresholds: MetricThresholds = { warn: 65, danger: 85 };

  settings: any;

  private buildSettings(): any {
    return {
      mode: 'external',
      hideSubHeader: false, // Enable search
      noDataMessage: this.translate.instant('cluster.backends.noData'),
      actions: {
        columnTitle: this.translate.instant('cluster.shared.actions'),
        add: false,
        edit: false,
        delete: true,
        position: 'right',
      },
      delete: {
        deleteButtonContent: '<i class="nb-trash"></i>',
        confirmDelete: true,  // Enable custom confirmation via deleteConfirm event
      },
      pager: {
        display: true,
        perPage: 15,
      },
      columns: {
        BackendId: {
          title: this.translate.instant('cluster.backends.columns.nodeId'),
          type: 'string',
          width: '8%',
        },
        IP: {
          title: this.translate.instant('cluster.backends.columns.host'),
          type: 'string',
        },
        HeartbeatPort: {
          title: this.translate.instant('cluster.backends.columns.heartbeatPort'),
          type: 'string',
          width: '10%',
        },
        BePort: {
          title: this.translate.instant('cluster.backends.columns.bePort'),
          type: 'string',
          width: '10%',
        },
        Alive: {
          title: this.translate.instant('cluster.shared.status'),
          type: 'html',
          width: '8%',
          valuePrepareFunction: (value: string) => {
            const status = value === 'true' ? 'success' : 'danger';
            const text = value === 'true' ? this.translate.instant('cluster.shared.online') : this.translate.instant('cluster.shared.offline');
            return `<span class="badge badge-${status}">${text}</span>`;
          },
        },
        TabletNum: {
          title: this.translate.instant('cluster.backends.columns.tabletNum'),
          type: 'string',
          width: '10%',
        },
        DataUsedCapacity: {
          title: this.translate.instant('cluster.backends.columns.dataUsed'),
          type: 'string',
        },
        TotalCapacity: {
          title: this.translate.instant('cluster.backends.columns.totalCapacity'),
          type: 'string',
        },
        UsedPct: {
          title: this.translate.instant('cluster.backends.columns.diskUsage'),
          type: 'html',
          width: '10%',
          valuePrepareFunction: (value: string) => renderMetricBadge(value, this.diskThresholds),
        },
        CpuUsedPct: {
          title: this.translate.instant('cluster.backends.columns.cpuUsage'),
          type: 'html',
          width: '12%',
          valuePrepareFunction: (value: string) => renderMetricBadge(value, this.cpuThresholds),
        },
        MemUsedPct: {
          title: this.translate.instant('cluster.backends.columns.memUsage'),
          type: 'html',
          width: '10%',
          valuePrepareFunction: (value: string) => renderMetricBadge(value, this.memoryThresholds),
        },
        NumRunningQueries: {
          title: this.translate.instant('cluster.backends.columns.runningQueries'),
          type: 'string',
          width: '10%',
        },
      },
    };
  }

  onDeleteConfirm(event: any): void {
    const backend = event.data;
    const itemName = `${backend.IP}:${backend.HeartbeatPort}`;
    const nodeType = this.deploymentMode === 'shared_data' ? 'CN (Compute Node)' : 'BE (Backend)';
    const additionalWarning = this.translate.instant('cluster.backends.deleteWarning', { nodeType });
    
    this.confirmDialogService.confirmDelete(itemName, additionalWarning)
      .subscribe(confirmed => {
        if (!confirmed) {
          event.confirm.reject();
          return;
        }

        this.nodeService.deleteBackend(backend.IP, backend.HeartbeatPort)
          .subscribe({
            next: () => {
              this.toastrService.success(
                this.translate.instant('cluster.backends.deleteSuccess', { nodeType, itemName }),
                this.translate.instant('cluster.shared.success')
              );
              event.confirm.resolve();
              this.loadBackends();
            },
            error: (error) => {
              this.toastrService.danger(
                ErrorHandler.extractErrorMessage(error),
                this.translate.instant('cluster.detail.deleteFailed'),
              );
              event.confirm.reject();
            },
          });
      });
  }

  constructor(
    private nodeService: NodeService,
    private clusterService: ClusterService,
    private clusterContext: ClusterContextService,
    private toastrService: NbToastrService,
    private confirmDialogService: ConfirmDialogService,
    private translate: TranslateService,
  ) {
    this.settings = this.buildSettings();
    // Get clusterId from ClusterContextService
    this.clusterId = this.clusterContext.getActiveClusterId() || 0;
  }

  ngOnInit(): void {
    this.translate.onLangChange
      .pipe(takeUntil(this.destroy$))
      .subscribe(() => {
        this.settings = this.buildSettings();
      });

    // Subscribe to active cluster changes
    // activeCluster$ is a BehaviorSubject, so it emits immediately on subscribe
    this.clusterContext.activeCluster$
      .pipe(takeUntil(this.destroy$))
      .subscribe(cluster => {
        this.activeCluster = cluster;
        if (cluster) {
          // Always use the active cluster (override route parameter)
          const newClusterId = cluster.id;
          if (this.clusterId !== newClusterId) {
            this.clusterId = newClusterId;
            this.loadClusterInfo();
            this.loadBackends();
          }
        }
        // Backend will handle "no active cluster" case
        
      });

    // Load data - backend will get active cluster automatically
    // This ensures data loads even if activeCluster$ hasn't emitted yet
    this.loadClusterInfo();
    this.loadBackends();
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  loadClusterInfo(): void {
    this.clusterService.getCluster(this.clusterId)
      .pipe(takeUntil(this.destroy$))
      .subscribe({
        next: (cluster) => {
          this.clusterName = cluster.name;
          this.deploymentMode = cluster.deployment_mode || 'shared_nothing';
          
          // Update page title and badge based on deployment mode
          if (this.deploymentMode === 'shared_data') {
            this.pageTitle = 'cluster.backends.titleCn';
            this.deploymentModeText = 'cluster.shared.sharedData';
            this.deploymentModeBadgeClass = 'badge-info';
          } else {
            this.pageTitle = 'cluster.backends.titleBe';
            this.deploymentModeText = 'cluster.shared.sharedNothing';
            this.deploymentModeBadgeClass = 'badge-success';
          }
        },
      });
  }

  loadBackends(): void {
    this.loading = true;
    
    this.nodeService.listBackends()
      .pipe(takeUntil(this.destroy$))
      .subscribe({
        next: (backends) => {
          this.source.load(backends);
          this.loading = false;
        },
        error: (error) => {
          this.toastrService.danger(
            ErrorHandler.handleClusterError(error),
            this.translate.instant('cluster.shared.error'),
          );
          this.source.load([]);
          this.loading = false;
        },
      });
  }
}