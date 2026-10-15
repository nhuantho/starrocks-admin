import { Component, OnInit, OnDestroy } from '@angular/core';
import { interval, Subject } from 'rxjs';
import { takeUntil, switchMap } from 'rxjs/operators';
import { NbToastrService } from '@nebular/theme';
import { LocalDataSource } from 'ng2-smart-table';
import { TranslateService } from '@ngx-translate/core';
import { NodeService } from '../../../@core/data/node.service';
import { ClusterService, Cluster } from '../../../@core/data/cluster.service';
import { ClusterContextService } from '../../../@core/data/cluster-context.service';
import { ErrorHandler } from '../../../@core/utils/error-handler';

@Component({
  selector: 'ngx-frontends',
  templateUrl: './frontends.component.html',
  styleUrls: ['./frontends.component.scss'],
})
export class FrontendsComponent implements OnInit, OnDestroy {
  source: LocalDataSource = new LocalDataSource();
  clusterId: number;
  activeCluster: Cluster | null = null;
  clusterName: string = '';
  loading = true;
  private destroy$ = new Subject<void>();

  settings: any;

  private buildSettings(): any {
    return {
      mode: 'external',
      hideSubHeader: false, // Enable search
      noDataMessage: this.translate.instant('cluster.frontends.noData'),
      actions: false,
      pager: {
        display: true,
        perPage: 15,
      },
      columns: {
        IP: { 
          title: this.translate.instant('cluster.frontends.columns.host'),
          type: 'string',
          width: '15%',
        },
        HttpPort: { 
          title: this.translate.instant('cluster.frontends.columns.httpPort'),
          type: 'string',
          width: '8%',
        },
        QueryPort: { 
          title: this.translate.instant('cluster.frontends.columns.queryPort'),
          type: 'string',
          width: '8%',
        },
        Role: { 
          title: this.translate.instant('cluster.frontends.columns.role'),
          type: 'html', 
          width: '9%',
          valuePrepareFunction: (value: string) => {
            if (value === 'LEADER') {
              return '<span class="badge badge-primary">LEADER</span>';
            } else if (value === 'FOLLOWER') {
              return '<span class="badge badge-info">FOLLOWER</span>';
            } else if (value === 'OBSERVER') {
              return '<span class="badge badge-warning">OBSERVER</span>';
            }
            return `<span class="badge badge-secondary">${value}</span>`;
          },
        },
        Alive: {
          title: this.translate.instant('cluster.shared.status'),
          type: 'html',
          width: '7%',
          valuePrepareFunction: (value: string) => {
            const status = value === 'true' ? 'success' : 'danger';
            const text = value === 'true' ? this.translate.instant('cluster.shared.online') : this.translate.instant('cluster.shared.offline');
            return `<span class="badge badge-${status}">${text}</span>`;
          },
        },
        ReplayedJournalId: { 
          title: this.translate.instant('cluster.frontends.columns.replayedJournalId'),
          type: 'string',
          width: '10%',
        },
        LastHeartbeat: { 
          title: this.translate.instant('cluster.frontends.columns.lastHeartbeat'),
          type: 'string',
          width: '11%',
        },
        StartTime: { 
          title: this.translate.instant('cluster.frontends.columns.startTime'),
          type: 'string',
          width: '11%',
        },
        Version: { 
          title: this.translate.instant('cluster.frontends.columns.version'),
          type: 'string',
          width: '9%',
        },
      },
    };
  }

  constructor(
    private nodeService: NodeService,
    private clusterService: ClusterService,
    private clusterContext: ClusterContextService,
    private toastrService: NbToastrService,
    private translate: TranslateService,
  ) {
    this.settings = this.buildSettings();
    this.clusterId = this.clusterContext.getActiveClusterId() || 0;
  }

  ngOnInit(): void {
    this.translate.onLangChange
      .pipe(takeUntil(this.destroy$))
      .subscribe(() => {
        this.settings = this.buildSettings();
      });

    // Subscribe to active cluster changes
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
            this.loadFrontends();
          }
        }
        // Backend will handle "no active cluster" case
        
      });

    // Load data - backend will get active cluster automatically
    this.loadClusterInfo();
    this.loadFrontends();
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  loadClusterInfo(): void {
    this.clusterService.getCluster(this.clusterId).subscribe({
      next: (cluster) => {
        this.clusterName = cluster.name;
      },
    });
  }

  loadFrontends(): void {
    this.loading = true;
    this.nodeService.listFrontends().subscribe({
      next: (frontends) => {
        this.source.load(frontends);
        this.loading = false;
      },
      error: (error) => {
        this.toastrService.danger(ErrorHandler.handleClusterError(error), this.translate.instant('cluster.shared.loadFailed'));
        this.loading = false;
      },
    });
  }
}
