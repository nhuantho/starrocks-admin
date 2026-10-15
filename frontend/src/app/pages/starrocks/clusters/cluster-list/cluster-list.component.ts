import { Component, OnDestroy, OnInit } from '@angular/core';
import { Router } from '@angular/router';
import { NbDialogService, NbToastrService } from '@nebular/theme';
import { LocalDataSource } from 'ng2-smart-table';
import { TranslateService } from '@ngx-translate/core';
import { Subscription } from 'rxjs';
import { ClusterService, Cluster } from '../../../../@core/data/cluster.service';
import { ErrorHandler } from '../../../../@core/utils/error-handler';
import { ConfirmDialogService } from '../../../../@core/services/confirm-dialog.service';

@Component({
  selector: 'ngx-cluster-list',
  templateUrl: './cluster-list.component.html',
  styleUrls: ['./cluster-list.component.scss'],
})
export class ClusterListComponent implements OnInit, OnDestroy {
  source: LocalDataSource = new LocalDataSource();
  loading = true;

  settings: any;
  private langChangeSub?: Subscription;

  private buildSettings(): any {
    return {
      mode: 'external',
      hideSubHeader: false,  // Enable search
      noDataMessage: this.translate.instant('cluster.list.noData'),
      actions: {
        columnTitle: this.translate.instant('cluster.shared.actions'),
        add: false,
        edit: true,
        delete: true,
        position: 'right',
      },
      edit: {
        editButtonContent: '<i class="nb-edit"></i>',
      },
      delete: {
        deleteButtonContent: '<i class="nb-trash"></i>',
        confirmDelete: false,
      },
      pager: {
        display: true,
        perPage: 10,
      },
      columns: {
        id: {
          title: 'ID',
          type: 'number',
          width: '5%',
        },
        name: {
          title: this.translate.instant('cluster.list.columns.name'),
          type: 'string',
        },
        fe_host: {
          title: this.translate.instant('cluster.list.columns.feHost'),
          type: 'string',
        },
        fe_http_port: {
          title: this.translate.instant('cluster.list.columns.httpPort'),
          type: 'number',
          width: '10%',
        },
        fe_query_port: {
          title: this.translate.instant('cluster.list.columns.queryPort'),
          type: 'number',
          width: '10%',
        },
        username: {
          title: this.translate.instant('cluster.list.columns.username'),
          type: 'string',
          width: '10%',
        },
        description: {
          title: this.translate.instant('cluster.list.columns.description'),
          type: 'string',
        },
        created_at: {
          title: this.translate.instant('cluster.list.columns.createdAt'),
          type: 'string',
          valuePrepareFunction: (date: string) => {
            return new Date(date).toLocaleString(this.translate.currentLang || 'en');
          },
        },
      },
    };
  }

  constructor(
    private clusterService: ClusterService,
    private router: Router,
    private dialogService: NbDialogService,
    private toastrService: NbToastrService,
    private confirmDialogService: ConfirmDialogService,
    private translate: TranslateService,
  ) {
    this.settings = this.buildSettings();
  }

  ngOnInit(): void {
    this.langChangeSub = this.translate.onLangChange.subscribe(() => {
      this.settings = this.buildSettings();
    });
    this.loadClusters();
  }

  ngOnDestroy(): void {
    this.langChangeSub?.unsubscribe();
  }

  loadClusters(): void {
    this.loading = true;
    this.clusterService.listClusters().subscribe({
      next: (clusters) => {
        this.source.load(clusters);
        this.loading = false;
      },
      error: (error) => {
        this.toastrService.danger(
          ErrorHandler.extractErrorMessage(error),
          this.translate.instant('cluster.shared.error'),
        );
        this.loading = false;
      },
    });
  }

  onCreate(): void {
    this.router.navigate(['/pages/starrocks/clusters/new']);
  }

  onEdit(event: any): void {
    this.router.navigate(['/pages/starrocks/clusters', event.data.id, 'edit']);
  }

  onDelete(event: any): void {
    const cluster = event.data as Cluster;

    this.confirmDialogService.confirmDelete(cluster.name)
      .subscribe(confirmed => {
        if (!confirmed) {
          return;
        }

        this.clusterService.deleteCluster(cluster.id).subscribe({
          next: () => {
            this.toastrService.success(
              this.translate.instant('cluster.list.deleteSuccess'),
              this.translate.instant('cluster.shared.success'),
            );
            this.loadClusters();
          },
          error: (error) => {
            this.toastrService.danger(
              ErrorHandler.extractErrorMessage(error),
              this.translate.instant('cluster.shared.error'),
            );
          },
        });
      });
  }

  onRowSelect(event: any): void {
    this.router.navigate(['/pages/starrocks/clusters', event.data.id]);
  }

  testConnection(cluster: Cluster): void {
    this.clusterService.getHealth(cluster.id).subscribe({
      next: (health) => {
        if (health.status === 'healthy') {
          const details = health.checks.map(c => `${c.name}: ${c.message}`).join('\n');
          this.toastrService.success(details, this.translate.instant('cluster.shared.healthCheckPassed'));
        } else if (health.status === 'warning') {
          const details = health.checks.map(c => `${c.name}: ${c.message}`).join('\n');
          this.toastrService.warning(details, this.translate.instant('cluster.list.healthCheckWarning'));
        } else {
          const details = health.checks.map(c => `${c.name}: ${c.message}`).join('\n');
          this.toastrService.danger(details, this.translate.instant('cluster.shared.healthCheckFailed'));
        }
      },
      error: (error) => {
        this.toastrService.danger(
          ErrorHandler.extractErrorMessage(error),
          this.translate.instant('cluster.shared.error'),
        );
      },
    });
  }
}

