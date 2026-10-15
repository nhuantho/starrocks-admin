import { Component, OnInit, OnDestroy } from '@angular/core';
import { ActivatedRoute, Router } from '@angular/router';
import { CdkDragDrop, moveItemInArray, transferArrayItem } from '@angular/cdk/drag-drop';

import { NbDialogService, NbToastrService, NbDialogRef } from '@nebular/theme';
import { LocalDataSource } from 'ng2-smart-table';
import { Subject } from 'rxjs';
import { takeUntil } from 'rxjs/operators';
import { NodeService } from '../../../@core/data/node.service';
import { ClusterContextService } from '../../../@core/data/cluster-context.service';
import { Cluster } from '../../../@core/data/cluster.service';
import { NestedLinkRenderComponent } from './nested-link-render.component';
import { SystemFunctionService } from '../../../@core/data/system-function.service';
import { SystemFunction, FunctionCategory, CreateFunctionRequest } from '../../../@core/data/system-function';
import { AddFunctionDialogComponent } from './add-function-dialog/add-function-dialog.component';
import { EditFunctionDialogComponent } from './edit-function-dialog/edit-function-dialog.component';
import { ErrorHandler } from '../../../@core/utils/error-handler';
import { ConfirmDialogService } from '../../../@core/services/confirm-dialog.service';
import { TranslateService } from '@ngx-translate/core';

// 内置分类名称（数据库中的值）到翻译键的映射
const BUILTIN_CATEGORY_KEYS: { [categoryName: string]: string } = {
  '集群信息': 'clusterInfo',
  '数据库管理': 'database',
  '事务管理': 'transaction',
  '任务管理': 'task',
  '元数据管理': 'metadata',
  '存储管理': 'storage',
  '作业管理': 'job',
};

interface SystemFunctionOld {
  name: string;
  description: string;
  category: string;
  status: string;
  last_updated: string;
  isSystem?: boolean;
}

interface SystemCategoryOld {
  name: string;
  functions: SystemFunctionOld[];
  icon: string;
  color: string;
}

interface NavigationState {
  path: string;           // 当前路径，如 '/transactions/10005'
  breadcrumbs: string[];  // 面包屑，如 ['transactions', '10005']
  canNavigate: boolean;   // 当前表格的第一列是否可点击
}

interface NavigationHistoryItem {
  functionName: string;  // 功能名称，如 'transactions'
  nestedPath?: string;   // 嵌套路径，如 '11125/running'
  fullPath: string;      // 完整路径，如 '/transactions/11125/running'
}

@Component({
  selector: 'ngx-system-management',
  templateUrl: './system-management.component.html',
  styleUrls: ['./system-management.component.scss']
})
export class SystemManagementComponent implements OnInit, OnDestroy {
  clusterId: number;
  activeCluster: Cluster | null = null;
  
  // 系统默认功能（硬编码）
  systemFunctions: SystemFunctionOld[] = ([
    ['backends', 'clusterInfo'],
    ['frontends', 'clusterInfo'],
    ['brokers', 'clusterInfo'],
    ['statistic', 'clusterInfo'],
    ['dbs', 'database'],
    ['tables', 'database'],
    ['tablet_schema', 'database'],
    ['partitions', 'database'],
    ['transactions', 'transaction'],
    ['routine_loads', 'task'],
    ['stream_loads', 'task'],
    ['loads', 'task'],
    ['load_error_hub', 'task'],
    ['catalog', 'metadata'],
    ['resources', 'metadata'],
    ['workload_groups', 'metadata'],
    ['workload_sched_policy', 'metadata'],
    ['compactions', 'storage'],
    ['colocate_group', 'storage'],
    ['bdbje', 'storage'],
    ['small_files', 'storage'],
    ['trash', 'storage'],
    ['jobs', 'job'],
    ['repositories', 'job']
  ] as [string, string][]).map(([name, category]) => ({
    name,
    description: name,
    category,
    status: 'active',
    last_updated: '2024-01-01',
    isSystem: true,
  }));

  // 自定义功能
  customFunctions: SystemFunction[] = [];
  
  // 合并后的分类
  categories: FunctionCategory[] = [];
  
  selectedFunction: SystemFunctionOld | null = null;
  functionData: any[] = [];
  functionDataSource: LocalDataSource = new LocalDataSource();
  loading = false;
  tableSettings: any = {};
  
  // 导航状态管理
  navigationState: NavigationState = {
    path: '',
    breadcrumbs: [],
    canNavigate: false
  };
  
  // 历史记录栈 - 通用导航管理
  navigationHistory: NavigationHistoryItem[] = [];
  
  private destroy$ = new Subject<void>();

  constructor(
    private nodeService: NodeService,
    private clusterContext: ClusterContextService,
    private dialogService: NbDialogService,
    private confirmDialogService: ConfirmDialogService,
    private toastrService: NbToastrService,
    private systemFunctionService: SystemFunctionService,
    private route: ActivatedRoute,
    private router: Router,
    private translate: TranslateService,
  ) {
    // Get clusterId from ClusterContextService
    this.clusterId = this.clusterContext.getActiveClusterId() || 0;
  }

  ngOnInit() {
    // Rebuild table settings (noDataMessage) when the language changes
    this.translate.onLangChange
      .pipe(takeUntil(this.destroy$))
      .subscribe(() => {
        if (this.functionData.length > 0) {
          this.setupTableSettings();
        }
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
            this.loadSystemFunctions();
          }
        }
        // Backend will handle "no active cluster" case
      });

    // Load data - backend will get active cluster automatically
    this.loadSystemFunctions();

    // 处理查询参数 - 每次查询参数变化时都处理
    this.route.queryParams.pipe(takeUntil(this.destroy$)).subscribe(params => {
      if (params['function']) {
        const functionName = params['function'];
        // 检查是否需要处理（避免重复处理相同参数）
        const currentFunction = this.selectedFunction?.name;
        if (currentFunction !== functionName) {
          // 等待功能加载完成后再处理
          setTimeout(() => {
            this.handleQueryParams(functionName);
          }, 300);
        }
      }
    });
  }

  /**
   * 处理查询参数，选择对应的功能
   */
  private handleQueryParams(functionName: string): void {
    // 先从 systemFunctions 中查找
    let targetFunction = this.systemFunctions.find(f => f.name === functionName);
    // 如果没找到，从已加载的功能中查找
    if (!targetFunction && this.customFunctions.length > 0) {
      const foundFunc = this.customFunctions.find(f => f.functionName === functionName);
      if (foundFunc) {
        // 转换为 SystemFunctionOld 格式
        targetFunction = {
          name: foundFunc.functionName,
          description: foundFunc.description,
          category: foundFunc.categoryName,
          status: 'active',
          last_updated: foundFunc.updatedAt,
          isSystem: foundFunc.isSystem
        };
      }
    }
    if (targetFunction) {
      this.selectFunction(targetFunction);
    }
  }

  ngOnDestroy() {
    this.destroy$.next();
    this.destroy$.complete();
  }

  // 加载系统功能（从数据库加载所有功能）
  loadSystemFunctions() {
    // Backend will get active cluster automatically - no need to check clusterId
    this.loading = true;
    
    // 从数据库加载所有功能（包括默认和自定义）
    this.systemFunctionService.getFunctions()
      .pipe(takeUntil(this.destroy$))
      .subscribe({
        next: (allFunctions) => {
          // 所有功能都从数据库加载，不再区分系统/自定义
          this.customFunctions = allFunctions;
          this.mergeAndOrganizeFunctions();
        this.loading = false;
      },
      error: (error) => {
        console.error('Failed to load system functions:', error);
        this.toastrService.danger(ErrorHandler.extractErrorMessage(error), this.translate.instant('sysfn.toast.loadFailed'));
        this.loading = false;
      }
    });
  }

  // 整理功能（所有功能都从数据库加载）
  mergeAndOrganizeFunctions() {
    
    // 所有功能都从数据库加载，包括系统默认功能和自定义功能
    const allFunctions = this.customFunctions;
    
    // 按收藏状态和分类排序
    allFunctions.sort((a, b) => {
      if (a.isFavorited !== b.isFavorited) {
        return b.isFavorited ? 1 : -1; // 收藏的在前
      }
      if (a.categoryOrder !== b.categoryOrder) {
        return a.categoryOrder - b.categoryOrder;
      }
      return a.displayOrder - b.displayOrder;
    });

    // 按分类分组
    const categoryMap = new Map<string, SystemFunction[]>();
    allFunctions.forEach(func => {
      if (!categoryMap.has(func.categoryName)) {
        categoryMap.set(func.categoryName, []);
      }
      categoryMap.get(func.categoryName)!.push(func);
    });


    // 转换为 FunctionCategory 数组
    this.categories = Array.from(categoryMap.entries()).map(([name, functions]) => ({
      name,
      functions: functions.slice(0, 4), // 限制每个分类最多4个功能
      order: functions[0]?.categoryOrder || 0
    })).sort((a, b) => a.order - b.order);
  }

  // 打开添加功能对话框
  openAddFunctionDialog(categoryName?: string) {
    this.dialogService.open(AddFunctionDialogComponent, {
      context: {
        categoryName: categoryName || ''
      },
      hasBackdrop: true,
      closeOnBackdropClick: false,
      closeOnEsc: true,
      hasScroll: true,
      autoFocus: true,
      dialogClass: 'add-function-dialog'
    }).onClose.subscribe((result: CreateFunctionRequest) => {
      if (result) {
        this.createCustomFunction(result);
      }
    });
  }

  // 打开编辑功能对话框
  editFunction(func: SystemFunction) {
    this.dialogService.open(EditFunctionDialogComponent, {
      context: {
        function: func
      },
      hasBackdrop: true,
      closeOnBackdropClick: false,
      closeOnEsc: true,
      hasScroll: true,
      autoFocus: true,
      dialogClass: 'edit-function-dialog'
    }).onClose.subscribe((result: SystemFunction) => {
      if (result) {
        this.updateCustomFunction(result);
      }
    });
  }

  // 创建自定义功能
  createCustomFunction(request: CreateFunctionRequest) {
    this.systemFunctionService.createFunction(request)
      .pipe(takeUntil(this.destroy$))
      .subscribe({
        next: (newFunction) => {
          this.customFunctions.push(newFunction);
          this.mergeAndOrganizeFunctions();
          this.toastrService.success(this.translate.instant('sysfn.toast.addSuccess'), this.translate.instant('sysfn.toast.success'));
        },
        error: (error) => {
          console.error('Failed to create function:', error);
          this.toastrService.danger(ErrorHandler.extractErrorMessage(error), this.translate.instant('sysfn.toast.addFailed'));
        }
      });
  }

  // 更新自定义功能
  updateCustomFunction(func: SystemFunction) {
    this.systemFunctionService.updateFunction(func.id, {
      category_name: func.categoryName,
      function_name: func.functionName,
      description: func.description,
      sql_query: func.sqlQuery
    })
      .pipe(takeUntil(this.destroy$))
      .subscribe({
        next: (updatedFunction) => {
          // 更新本地数据
          const index = this.customFunctions.findIndex(f => f.id === func.id);
          if (index !== -1) {
            this.customFunctions[index] = updatedFunction;
            this.mergeAndOrganizeFunctions();
          }
          this.toastrService.success(this.translate.instant('sysfn.toast.updateSuccess'), this.translate.instant('sysfn.toast.success'));
        },
        error: (error) => {
          console.error('Failed to update function:', error);
          this.toastrService.danger(ErrorHandler.extractErrorMessage(error), this.translate.instant('sysfn.toast.updateFailed'));
        }
      });
  }

  // 处理分类拖拽
  onCategoryDrop(event: CdkDragDrop<FunctionCategory[]>) {
    moveItemInArray(this.categories, event.previousIndex, event.currentIndex);
    
    // 更新每个分类中所有功能的 categoryOrder
    this.categories.forEach((category, categoryIndex) => {
      category.functions.forEach((func, funcIndex) => {
        func.categoryOrder = categoryIndex;
        func.displayOrder = funcIndex;
      });
    });
    
    this.saveCategoryOrders();
  }

  // 处理功能拖拽
  onFunctionDrop(event: CdkDragDrop<SystemFunction[]>, category: FunctionCategory) {
    if (event.previousContainer === event.container) {
      moveItemInArray(event.container.data, event.previousIndex, event.currentIndex);
    } else {
      transferArrayItem(
        event.previousContainer.data,
        event.container.data,
        event.previousIndex,
        event.currentIndex
      );
    }
    
    // 更新该分类中所有功能的 displayOrder
    category.functions.forEach((func, funcIndex) => {
      func.displayOrder = funcIndex;
    });
    
    this.saveFunctionOrders();
  }

  // 保存分类顺序
  saveCategoryOrders() {
    const orders: any[] = [];
    
    this.categories.forEach((category, categoryIndex) => {
      category.functions.forEach((func, funcIndex) => {
        orders.push({
          id: func.id,
          displayOrder: funcIndex,
          categoryOrder: categoryIndex
        });
      });
    });

    if (orders.length > 0) {
      this.systemFunctionService.updateOrders({ functions: orders })
        .pipe(takeUntil(this.destroy$))
        .subscribe({
          next: () => {
            this.toastrService.success(this.translate.instant('sysfn.toast.categoryOrderSaved'), this.translate.instant('sysfn.toast.success'));
          },
          error: (error) => {
            console.error('Failed to save category orders:', error);
            this.toastrService.danger(ErrorHandler.extractErrorMessage(error), this.translate.instant('sysfn.toast.saveFailed'));
          }
        });
    }
  }

  // 保存功能顺序
  saveFunctionOrders() {
    const orders: any[] = [];
    
    this.categories.forEach((category, categoryIndex) => {
      category.functions.forEach((func, funcIndex) => {
        // 保存所有功能的顺序（包括系统功能和自定义功能）
        orders.push({
          id: func.id,
          displayOrder: funcIndex,
          categoryOrder: categoryIndex
        });
      });
    });

    if (orders.length > 0) {
      this.systemFunctionService.updateOrders({ functions: orders })
        .pipe(takeUntil(this.destroy$))
        .subscribe({
          next: () => {
            this.toastrService.success(this.translate.instant('sysfn.toast.functionOrderSaved'), this.translate.instant('sysfn.toast.success'));
          },
          error: (error) => {
            console.error('Failed to save function orders:', error);
            this.toastrService.danger(ErrorHandler.extractErrorMessage(error), this.translate.instant('sysfn.toast.saveFailed'));
          }
        });
    }
  }

  // 切换收藏状态
  toggleFavorite(functionId: number) {
    this.systemFunctionService.toggleFavorite(functionId)
      .pipe(takeUntil(this.destroy$))
      .subscribe({
        next: (updatedFunction) => {
          // 更新本地数据
          const func = this.customFunctions.find(f => f.id === functionId);
          if (func) {
            func.isFavorited = updatedFunction.isFavorited;
            this.mergeAndOrganizeFunctions();
          }
          this.toastrService.success(
            this.translate.instant(updatedFunction.isFavorited ? 'sysfn.toast.favorited' : 'sysfn.toast.unfavorited'),
            this.translate.instant('sysfn.toast.success')
          );
        },
        error: (error) => {
          console.error('Failed to toggle favorite:', error);
          this.toastrService.danger(ErrorHandler.extractErrorMessage(error), this.translate.instant('sysfn.toast.operationFailed'));
        }
      });
  }

  // 删除自定义功能
  deleteFunction(functionId: number) {
    const functionToDelete = this.customFunctions.find(f => f.id === functionId);
    if (!functionToDelete) {
      return;
    }

    this.confirmDialogService.confirmDelete(functionToDelete.functionName)
      .subscribe(confirmed => {
        if (confirmed) {
          this.systemFunctionService.deleteFunction(functionId)
            .pipe(takeUntil(this.destroy$))
            .subscribe({
              next: () => {
                // 从本地数据中移除
                this.customFunctions = this.customFunctions.filter(f => f.id !== functionId);
                this.mergeAndOrganizeFunctions();
                this.toastrService.success(this.translate.instant('sysfn.toast.deleteSuccess'), this.translate.instant('sysfn.toast.success'));
              },
              error: (error) => {
                console.error('Failed to delete function:', error);
                this.toastrService.danger(ErrorHandler.extractErrorMessage(error), this.translate.instant('sysfn.toast.deleteFailed'));
              }
            });
        }
      });
  }

  // 执行功能（系统默认功能）
  selectFunction(func: SystemFunctionOld) {
    this.selectedFunction = func;
    
    // 清空历史记录栈
    this.navigationHistory = [];
    
    // 添加根级别到历史记录
    this.navigationHistory.push({
      functionName: func.name,
      nestedPath: undefined,
      fullPath: `/${func.name}`
    });
    
    // 重置导航状态
    this.navigationState = {
      path: '',
      breadcrumbs: [],
      canNavigate: false
    };
    
    this.loadFunctionData(func.name);
  }

  // 执行功能（系统功能使用HTTP查询，自定义功能使用MySQL查询）
  executeCustomFunction(func: SystemFunction) {
    if (func.isSystem) {
      // 系统功能使用HTTP查询
      
      // 更新系统功能访问时间
      this.systemFunctionService.updateSystemFunctionAccessTime(func.functionName)
        .pipe(takeUntil(this.destroy$))
        .subscribe({
          next: () => {
          },
          error: (error) => {
            console.warn('[SystemManagement] Failed to update access time:', error);
          }
        });
      
      // 设置选中的功能
      this.selectedFunction = {
        name: func.functionName,
        description: func.description,
        category: func.categoryName,
        status: 'active',
        last_updated: func.updatedAt,
        isSystem: func.isSystem
      };
      
      this.loadFunctionData(func.functionName);
    } else {
      // 自定义功能使用MySQL查询
      this.systemFunctionService.executeFunction(func.id)
        .pipe(takeUntil(this.destroy$))
        .subscribe({
          next: (result) => {
            // 显示结果
            this.functionData = result;
            this.functionDataSource.load(this.functionData);
            this.setupTableSettings();
            this.selectedFunction = {
              name: func.functionName,
              description: func.description,
              category: func.categoryName,
              status: 'active',
              last_updated: func.updatedAt,
              isSystem: func.isSystem
            };
          },
          error: (error) => {
            console.error('Failed to execute custom function:', error);
            this.toastrService.danger(ErrorHandler.extractErrorMessage(error), this.translate.instant('sysfn.toast.executeFailed'));
          }
        });
    }
  }

  // 判断是否需要嵌套导航
  private needsNestedNavigation(functionName: string): boolean {
    const nestedFunctions = [
      'transactions', 'dbs', 'catalog', 'routine_loads', 
      'stream_loads', 'loads', 'load_error_hub', 'resources',
      'workload_groups', 'workload_sched_policy', 'compactions',
      'colocate_group', 'bdbje', 'small_files', 'trash',
      'jobs', 'repositories'
    ];
    return nestedFunctions.includes(functionName);
  }

  // 加载功能数据
  loadFunctionData(functionName: string, nestedPath?: string) {
    // Backend will get active cluster automatically - no need to check clusterId
    this.loading = true;
    
    this.nodeService.getSystemFunctionDetail(functionName, nestedPath).subscribe({
      next: (data: any) => {
        this.functionData = data.data || [];

        // 如果没有设置selectedFunction，则设置一个默认的
        if (!this.selectedFunction) {
          this.selectedFunction = {
            name: functionName,
            description: this.getFunctionDescription(functionName),
            category: this.translate.instant('sysfn.systemFunction'),
            status: 'active',
            last_updated: new Date().toISOString(),
            isSystem: true
          };
        }

        // 先更新导航状态，再设置表格
        this.updateNavigationState(functionName, nestedPath);
        this.functionDataSource.load(this.functionData); // 先加载数据
        this.setupTableSettings(); // 再设置表格配置
        this.loading = false;

      },
      error: (error) => {
        console.error('[SystemManagement] Failed to load function data:', error);
        this.loading = false;
        this.toastrService.danger(ErrorHandler.extractErrorMessage(error), this.translate.instant('sysfn.toast.loadFailed'));
      }
    });
  }

  // 更新导航状态
  updateNavigationState(functionName: string, nestedPath?: string) {
    const path = nestedPath ? `/${functionName}/${nestedPath}` : `/${functionName}`;
    const breadcrumbs = [functionName];
    
    if (nestedPath) {
      breadcrumbs.push(...nestedPath.split('/'));
    }
    
    this.navigationState = {
      path,
      breadcrumbs,
      canNavigate: this.needsNestedNavigation(functionName) && this.functionData.length > 0
    };
    
  }

  // 设置表格配置
  setupTableSettings() {
    if (this.functionData.length === 0) {
      this.tableSettings = {};
      return;
    }

    const firstRow = this.functionData[0];
    const columnKeys = Object.keys(firstRow);
    
    const columns: any = {};
    
    columnKeys.forEach((key, index) => {
      // 根据列名判断是否应该可点击（优先选择ID列）
      const shouldBeClickable = this.navigationState.canNavigate && this.selectedFunction &&
        (key.toLowerCase().includes('id') || key.toLowerCase().includes('dbid') ||
         (index === 0 && !columnKeys.some(k => k.toLowerCase().includes('id'))));

      if (shouldBeClickable) {
        columns[key] = {
          title: key,
          type: 'custom',
          renderComponent: NestedLinkRenderComponent,
          onComponentInitFunction: (instance: any) => {
            instance.save.subscribe((row: any) => {
              this.navigateToChild(row, key);
            });
          }
        };
      } else {
        columns[key] = {
          title: key,
          type: 'string'
        };
      }
    });

    this.tableSettings = {
      actions: {
        add: false,
        edit: false,
        delete: false,
        position: 'right'
      },
      columns: columns,
      pager: {
        display: true,
        perPage: 10
      },
      noDataMessage: this.translate.instant('sysfn.noData')
    };
  }

  // 导航到子级
  navigateToChild(row: any, columnKey: string) {
    if (!this.selectedFunction) return;
    
    const childValue = row[columnKey];
    if (!childValue) return;
    
    // 获取当前历史记录（栈顶）
    const currentHistory = this.navigationHistory[this.navigationHistory.length - 1];
    const currentNestedPath = currentHistory?.nestedPath || '';
    
    // 构建新的嵌套路径
    const newNestedPath = currentNestedPath ? `${currentNestedPath}/${childValue}` : childValue;
    
    // 推入新的历史记录到栈
    this.navigationHistory.push({
      functionName: this.selectedFunction.name,
      nestedPath: newNestedPath,
      fullPath: `/${this.selectedFunction.name}/${newNestedPath}`
    });
    
    this.loadFunctionData(this.selectedFunction.name, newNestedPath);
  }

  // 返回上一级
  goBack() {
    // 检查是否从概览页面跳转过来的
    this.route.queryParams.pipe(takeUntil(this.destroy$)).subscribe(params => {
      if (params['from'] === 'overview') {
        // 返回到集群概览页面
        this.router.navigate(['/pages/starrocks/overview']);
        return;
      }
    });

    if (this.navigationHistory.length > 1) {
      // 弹出当前层级
      this.navigationHistory.pop();
      
      // 获取上一级历史记录
      const previousHistory = this.navigationHistory[this.navigationHistory.length - 1];
      
      // 加载上一级数据
      this.loadFunctionData(previousHistory.functionName, previousHistory.nestedPath);
    } else {
      // 返回到功能列表
      this.selectedFunction = null;
      this.functionData = [];
      this.functionDataSource.load([]);
      this.navigationHistory = [];
      this.navigationState = {
        path: '',
        breadcrumbs: [],
        canNavigate: false
      };
    }
  }

  // 刷新当前功能
  refreshCurrentFunction() {
    if (this.selectedFunction && this.navigationHistory.length > 0) {
      // 获取当前历史记录（栈顶）
      const currentHistory = this.navigationHistory[this.navigationHistory.length - 1];
      
      // 重新加载当前层级的数据
      this.loadFunctionData(currentHistory.functionName, currentHistory.nestedPath);
    }
  }

  // 获取功能描述
  getFunctionDescription(functionName: string): string {
    const key = `sysfn.functions.${functionName}`;
    const translated = this.translate.instant(key);
    return translated !== key ? translated : this.translate.instant('sysfn.systemFunction');
  }

  // 获取功能描述的显示文本（仅翻译内置系统功能，自定义功能保持原样）
  getFunctionDescriptionLabel(functionName: string, description: string, isSystem?: boolean): string {
    // Chinese UI shows the description stored in the database unchanged
    if (!isSystem || this.translate.currentLang === 'zh-CN') {
      return description;
    }
    const key = `sysfn.functions.${functionName}`;
    const translated = this.translate.instant(key);
    return translated !== key ? translated : description;
  }

  // 获取分类的显示名称（仅翻译内置分类，自定义分类保持原样）
  getCategoryLabel(categoryName: string): string {
    const key = BUILTIN_CATEGORY_KEYS[categoryName];
    return key ? this.translate.instant(`sysfn.categories.${key}`) : categoryName;
  }

  // 格式化最后更新时间
  formatLastUpdated(dateString: string): string {
    return new Date(dateString).toLocaleString();
  }

  // 获取分类颜色
  getCategoryColor(categoryName: string): string {
    // 使用 ngx-admin 的标准状态颜色
    const colorMap: { [key: string]: string } = {
      '集群信息': 'info',
      '数据库管理': 'success', 
      '事务管理': 'warning',
      '任务管理': 'primary',
      '元数据管理': 'basic',
      '存储管理': 'danger',
      '作业管理': 'control'
    };
    
    // 如果分类已定义，返回对应颜色
    if (colorMap[categoryName]) {
      return colorMap[categoryName];
    }
    
    // 为新分类提供智能颜色选择：基于分类名称的哈希值
    const availableColors = ['info', 'success', 'warning', 'primary', 'basic', 'danger', 'control'];
    const hash = this.hashString(categoryName);
    return availableColors[hash % availableColors.length];
  }

  // 简单的字符串哈希函数
  private hashString(str: string): number {
    let hash = 0;
    for (let i = 0; i < str.length; i++) {
      const char = str.charCodeAt(i);
      hash = ((hash << 5) - hash) + char;
      hash = hash & hash; // 转换为32位整数
    }
    return Math.abs(hash);
  }

  // 判断是否为自定义分类
  isCustomCategory(categoryName: string): boolean {
    const systemCategories = ['集群信息', '数据库管理', '事务管理', '任务管理', '元数据管理', '存储管理', '作业管理'];
    return !systemCategories.includes(categoryName);
  }

  // 删除分类
  deleteCategory(categoryName: string) {
    this.confirmDialogService.confirm(
      this.translate.instant('sysfn.confirm.deleteCategoryTitle'),
      this.translate.instant('sysfn.confirm.deleteCategoryMessage', { name: categoryName }),
      this.translate.instant('sysfn.confirm.delete'),
      this.translate.instant('sysfn.confirm.cancel'),
      'danger'
    ).subscribe(confirmed => {
      if (confirmed) {
        this.systemFunctionService.deleteCategory(categoryName).subscribe({
          next: () => {
            this.toastrService.success(this.translate.instant('sysfn.toast.categoryDeleted'));
            this.loadSystemFunctions(); // 重新加载功能列表
          },
          error: (error) => {
            console.error('删除分类失败:', error);
            this.toastrService.danger(ErrorHandler.extractErrorMessage(error), this.translate.instant('sysfn.toast.deleteFailed'));
          }
        });
      }
    });
  }

  // 获取分类图标
  getCategoryIcon(categoryName: string): string {
    const iconMap: { [key: string]: string } = {
      '集群信息': 'cube-outline',
      '数据库管理': 'archive-outline',
      '事务管理': 'sync-outline',
      '任务管理': 'activity-outline',
      '元数据管理': 'layers-outline',
      '存储管理': 'hard-drive-outline',
      '作业管理': 'briefcase-outline'
    };
    
    // 如果分类已定义，返回对应图标
    if (iconMap[categoryName]) {
      return iconMap[categoryName];
    }
    
    // 为新分类提供智能图标选择：基于分类名称的哈希值
    const availableIcons = [
      'grid-outline', 'folder-outline', 'settings-outline', 'monitor-outline',
      'pie-chart-outline', 'bar-chart-outline', 'trending-up-outline', 'cube-outline',
      'briefcase-outline', 'layers-outline', 'hard-drive-outline', 'archive-outline'
    ];
    const hash = this.hashString(categoryName);
    return availableIcons[hash % availableIcons.length];
  }

}