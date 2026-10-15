import { Component, OnInit, OnDestroy } from '@angular/core';

import { NbToastrService, NbDialogService } from '@nebular/theme';
import { LocalDataSource } from 'ng2-smart-table';
import { TranslateService } from '@ngx-translate/core';
import { Subject } from 'rxjs';
import { takeUntil } from 'rxjs/operators';
import { ClusterContextService } from '../../../@core/data/cluster-context.service';
import { Cluster } from '../../../@core/data/cluster.service';
import { NodeService, Variable } from '../../../@core/data/node.service';
import { ErrorHandler } from '../../../@core/utils/error-handler';

@Component({
  selector: 'ngx-variables',
  templateUrl: './variables.component.html',
  styleUrls: ['./variables.component.scss'],
})
export class VariablesComponent implements OnInit, OnDestroy {
  clusterId: number;
  activeCluster: Cluster | null = null;
  variables: Variable[] = [];
  source: LocalDataSource = new LocalDataSource();
  loading = false;
  searchText = '';
  variableType = 'global'; // 'global' or 'session'
  private destroy$ = new Subject<void>();

  settings: any;

  private buildSettings(): any {
    return {
      hideSubHeader: false, // Enable search
      noDataMessage: this.translate.instant('cluster.variables.noMatch'),
      actions: {
        add: false,
        edit: true,
        delete: false,
        position: 'right',
      },
      edit: {
        editButtonContent: '<i class="nb-edit"></i>',
      },
      pager: {
        display: true,
        perPage: 20,
      },
      columns: {
        name: {
          title: this.translate.instant('cluster.variables.columns.name'),
          type: 'string',
          width: '40%',
        },
        value: {
          title: this.translate.instant('cluster.variables.columns.value'),
          type: 'string',
          width: '60%',
          valuePrepareFunction: (value: any) => {
            if (!value) return 'NULL';
            return value.length > 200 ? value.substring(0, 200) + '...' : value;
          },
        },
      },
    };
  }

  constructor(
    
    private toastrService: NbToastrService,
    private dialogService: NbDialogService,
    private clusterContext: ClusterContextService,
    private nodeService: NodeService,
    private translate: TranslateService,
  ) {
    this.settings = this.buildSettings();
    // Try to get clusterId from route first
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
    this.clusterContext.activeCluster$
      .pipe(takeUntil(this.destroy$))
      .subscribe(cluster => {
        this.activeCluster = cluster;
        if (cluster) {
          // Always use the active cluster (override route parameter)
          const newClusterId = cluster.id;
          if (this.clusterId !== newClusterId) {
            this.clusterId = newClusterId;
            this.loadVariables();
          }
        }
        // Backend will handle "no active cluster" case
      });

    // Load variables - backend will get active cluster automatically
    this.loadVariables();
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  loadVariables(): void {
    this.loading = true;
    this.nodeService.getVariables(
      
      this.variableType,
      this.searchText || undefined
    ).subscribe({
      next: (variables) => {
        this.variables = variables;
        this.source.load(variables);
        this.loading = false;
      },
      error: (error) => {
        console.error('[Variables] Error loading variables:', error);
        this.toastrService.danger(
          error.error?.message || this.translate.instant('cluster.variables.loadFailed'),
          this.translate.instant('cluster.shared.error')
        );
        this.variables = [];
        this.source.load([]);
        this.loading = false;
      },
    });
  }

  onEdit(event: any): void {
    this.editVariable(event.data);
  }

  editVariable(variable: Variable): void {
    const newValue = prompt(this.translate.instant('cluster.variables.editPrompt', { name: variable.name }), variable.value);
    if (newValue !== null && newValue !== variable.value) {
      this.loading = true;
      this.nodeService.updateVariable(variable.name, {
        value: newValue,
        scope: this.variableType.toUpperCase(),
      }).subscribe({
        next: () => {
          this.toastrService.success(
            this.translate.instant('cluster.variables.updateSuccess', { name: variable.name }),
            this.translate.instant('cluster.shared.success'),
          );
          this.loadVariables();
        },
        error: (error) => {
          this.toastrService.danger(
            error.error?.message || this.translate.instant('cluster.variables.updateFailed'),
            this.translate.instant('cluster.shared.error')
          );
          this.loading = false;
        },
      });
    }
  }

  onTypeChange(): void {
    this.loadVariables();
  }

  onSearch(): void {
    this.loadVariables();
  }

  refresh(): void {
    this.loadVariables();
  }
}

