import { Component, OnDestroy, OnInit } from '@angular/core';
import { NbDialogService, NbToastrService } from '@nebular/theme';
import { LocalDataSource } from 'ng2-smart-table';
import { Observable, Subject, forkJoin, of } from 'rxjs';
import { finalize, map, switchMap, takeUntil, tap } from 'rxjs/operators';

import {
  CreateRolePayload,
  PermissionDto,
  RoleService,
  RoleSummary,
  UpdateRolePayload,
} from '../../../@core/data/role.service';
import { PermissionService } from '../../../@core/data/permission.service';
import { ErrorHandler } from '../../../@core/utils/error-handler';
import { ConfirmDialogService } from '../../../@core/services/confirm-dialog.service';
import { RolesActionsCellComponent, RoleActionPermissions } from './table/actions-cell.component';
import {
  RoleFormDialogComponent,
  RoleFormDialogResult,
} from './role-form/role-form-dialog.component';
import { Organization, OrganizationService } from '../../../@core/data/organization.service';
import { AuthService } from '../../../@core/data/auth.service';
import { TranslateService } from '@ngx-translate/core';

@Component({
  selector: 'ngx-roles',
  templateUrl: './roles.component.html',
  styleUrls: ['./roles.component.scss'],
})
export class RolesComponent implements OnInit, OnDestroy {
  source: LocalDataSource = new LocalDataSource();
  loading = false;
  private destroy$ = new Subject<void>();
  private rolePermissionCache = new Map<number, number[]>();

  private basePermissions: PermissionDto[] = [];

  hasListPermission = false;
  canCreateRole = false;
  canUpdateRole = false;
  canDeleteRole = false;

  settings: any = {};
  organizations: Organization[] = [];
  currentOrganization?: Organization;
  isSuperAdmin = false;

  constructor(
    private roleService: RoleService,
    private permissionService: PermissionService,
    private dialogService: NbDialogService,
    private confirmDialog: ConfirmDialogService,
    private toastrService: NbToastrService,
    private organizationService: OrganizationService,
    private authService: AuthService,
    private translate: TranslateService,
  ) {}

  ngOnInit(): void {
    this.permissionService.permissions$
      .pipe(takeUntil(this.destroy$))
      .subscribe(() => this.applyPermissionState());

    this.translate.onLangChange
      .pipe(takeUntil(this.destroy$))
      .subscribe(() => this.applyTableSettings());

    this.applyPermissionState();
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  openCreateRole(): void {
    if (!this.canCreateRole) {
      return;
    }

    if (this.isSuperAdmin && !this.organizations.length) {
      this.loadOrganizations();
      this.toastrService.info(
        this.translate.instant('system.roles.orgsLoading'),
        this.translate.instant('system.shared.notice'),
      );
      return;
    }

    if (!this.isSuperAdmin && !this.currentOrganization) {
      this.loadCurrentOrganization();
      this.toastrService.info(
        this.translate.instant('system.roles.currentOrgLoading'),
        this.translate.instant('system.shared.notice'),
      );
      return;
    }

    this.ensurePermissions().subscribe((permissions) => {
      const dialogRef = this.dialogService.open(RoleFormDialogComponent, {
        context: {
          mode: 'create',
          permissions,
          organizations: this.organizations,
          currentOrganization: this.currentOrganization,
        },
        closeOnBackdropClick: false,
        autoFocus: false,
      });

      dialogRef.onClose.subscribe((result?: RoleFormDialogResult) => {
        if (result && result.mode === 'create') {
          this.createRole(result);
        }
      });
    });
  }

  openEditRole(role: RoleSummary): void {
    if (!this.canUpdateRole || role.is_system) {
      return;
    }

    forkJoin([
      this.ensurePermissions(),
      this.getRolePermissionIds(role.id),
    ])
      .pipe(takeUntil(this.destroy$))
      .subscribe({
        next: ([permissions, rolePermissionIds]) => {
          const assignedIds = new Set(rolePermissionIds);
          const prepared = permissions.map((permission) => ({
            ...permission,
            selected: assignedIds.has(permission.id),
          }));

          const dialogRef = this.dialogService.open(RoleFormDialogComponent, {
            context: {
              mode: 'edit',
              role,
              permissions: prepared,
              organizations: this.organizations,
              currentOrganization: this.currentOrganization,
            },
            closeOnBackdropClick: false,
            autoFocus: false,
          });

          dialogRef.onClose
            .pipe(takeUntil(this.destroy$))
            .subscribe((result?: RoleFormDialogResult) => {
              if (result && result.mode === 'edit') {
                this.updateRole(role, result);
              }
            });
        },
        error: (error) => {
          ErrorHandler.handleHttpError(error, this.toastrService);
        },
      });
  }

  onDeleteRole(role: RoleSummary): void {
    if (!this.canDeleteRole || role.is_system) {
      return;
    }

    this.confirmDialog.confirmDelete(role.name).subscribe((confirmed) => {
      if (!confirmed) {
        return;
      }

      this.roleService.deleteRole(role.id).subscribe({
        next: () => {
          this.toastrService.success(
            this.translate.instant('system.roles.deleted'),
            this.translate.instant('system.shared.success'),
          );
          this.rolePermissionCache.delete(role.id);
          this.loadRoles();
        },
        error: (error) => ErrorHandler.handleHttpError(error, this.toastrService),
      });
    });
  }

  private createRole(result: RoleFormDialogResult): void {
    const payload = result.rolePayload as CreateRolePayload;
    this.loading = true;

    this.roleService
      .createRole(payload)
      .pipe(
        switchMap((role) => {
          if (!result.permissionIds.length) {
            return of(role);
          }
          return this.roleService
            .updateRolePermissions(role.id, result.permissionIds)
            .pipe(map(() => role));
        }),
        finalize(() => (this.loading = false)),
      )
      .subscribe({
        next: (role) => {
          this.toastrService.success(
            this.translate.instant('system.roles.created'),
            this.translate.instant('system.shared.success'),
          );
          if (role) {
            this.rolePermissionCache.set(role.id, [...result.permissionIds]);
          }
          this.loadRoles();
        },
        error: (error) => ErrorHandler.handleHttpError(error, this.toastrService),
      });
  }

  private updateRole(role: RoleSummary, result: RoleFormDialogResult): void {
    const payload = result.rolePayload as UpdateRolePayload;
    this.loading = true;

    this.roleService
      .updateRole(role.id, payload)
      .pipe(
        switchMap(() => this.roleService.updateRolePermissions(role.id, result.permissionIds)),
        finalize(() => (this.loading = false)),
      )
      .subscribe({
        next: () => {
          this.toastrService.success(
            this.translate.instant('system.roles.updated'),
            this.translate.instant('system.shared.success'),
          );
          this.rolePermissionCache.set(role.id, [...result.permissionIds]);
          this.loadRoles();
        },
        error: (error) => ErrorHandler.handleHttpError(error, this.toastrService),
      });
  }

  private applyPermissionState(): void {
    this.hasListPermission = this.permissionService.hasPermission('api:roles:list');
    this.canCreateRole = this.permissionService.hasPermission('api:roles:create');
    this.canUpdateRole = this.permissionService.hasPermission('api:roles:update');
    this.canDeleteRole = this.permissionService.hasPermission('api:roles:delete');
    this.isSuperAdmin = this.authService.isSuperAdmin();

    if (this.isSuperAdmin) {
      this.loadOrganizations();
    } else {
      this.loadCurrentOrganization();
    }

    this.applyTableSettings();

    if ((this.canCreateRole || this.canUpdateRole) && !this.basePermissions.length) {
      this.preloadPermissions();
    }

    this.loadRoles();
  }

  private loadRoles(): void {
    if (!this.hasListPermission) {
      this.source.load([]);
      this.loading = false;
      return;
    }

    this.loading = true;
    this.roleService
      .listRoles()
      .pipe(finalize(() => (this.loading = false)))
      .subscribe({
        next: (roles) => this.source.load(roles),
        error: (error) => ErrorHandler.handleHttpError(error, this.toastrService),
      });
  }

  private preloadPermissions(): void {
    this.roleService
      .listPermissions()
      .pipe(tap((permissions) => (this.basePermissions = permissions)))
      .subscribe({
        error: (error) => ErrorHandler.handleHttpError(error, this.toastrService),
      });
  }

  private ensurePermissions(): Observable<PermissionDto[]> {
    if (this.basePermissions.length) {
      return of(this.basePermissions.map((permission) => ({ ...permission, selected: false })));
    }

    return this.roleService.listPermissions().pipe(
      tap((permissions) => (this.basePermissions = permissions)),
      map((permissions) => permissions.map((permission) => ({ ...permission, selected: false }))),
    );
  }

  private getRolePermissionIds(roleId: number): Observable<number[]> {
    const cached = this.rolePermissionCache.get(roleId);
    if (cached) {
      return of([...cached]);
    }

    return this.roleService.getRolePermissions(roleId).pipe(
      map((permissions) => {
        const ids = permissions.map((permission) => permission.id);
        this.rolePermissionCache.set(roleId, ids);
        return [...ids];
      }),
    );
  }

  private applyTableSettings(): void {
    const settings = this.buildTableSettings();
    settings.columns.actions.onComponentInitFunction = (
      component: RolesActionsCellComponent,
    ) => {
      component.edit
        .pipe(takeUntil(this.destroy$))
        .subscribe((role: RoleSummary) => this.openEditRole(role));
      component.remove
        .pipe(takeUntil(this.destroy$))
        .subscribe((role: RoleSummary) => this.onDeleteRole(role));
    };
    this.settings = settings;
  }

  private buildTableSettings(): any {
    const t = (key: string) => this.translate.instant(key);
    return {
      mode: 'external',
      hideSubHeader: false,
      noDataMessage: this.hasListPermission
        ? t('system.roles.table.noData')
        : t('system.roles.table.noPermission'),
      actions: {
        add: false,
        edit: false,
        delete: false,
        position: 'right',
      },
      pager: {
        display: true,
        perPage: 10,
      },
      columns: {
        id: {
          title: 'ID',
          type: 'number',
          width: '8%',
        },
        code: {
          title: t('system.roles.table.code'),
          type: 'string',
        },
        name: {
          title: t('system.roles.table.name'),
          type: 'string',
        },
        description: {
          title: t('system.roles.table.description'),
          type: 'string',
        },
        is_system: {
          title: t('system.roles.table.isSystem'),
          type: 'html',
          filter: false,
          sort: false,
          valuePrepareFunction: (cell: boolean) => {
            return cell
              ? `<span class="badge badge-info">${t('system.shared.yes')}</span>`
              : `<span class="badge badge-basic">${t('system.shared.no')}</span>`;
          },
        },
        created_at: {
          title: t('system.roles.table.createdAt'),
          type: 'string',
          valuePrepareFunction: (date: string) => new Date(date).toLocaleString(this.translate.currentLang || 'en'),
        },
        actions: {
          title: t('system.roles.table.actions'),
          type: 'custom',
          renderComponent: RolesActionsCellComponent,
          filter: false,
          sort: false,
          valuePrepareFunction: (cell: unknown, row: RoleSummary): RoleActionPermissions => ({
            canEdit: this.canUpdateRole && !row.is_system,
            canDelete: this.canDeleteRole && !row.is_system,
          }),
        },
      },
    };
  }

  private loadOrganizations(): void {
    if (!this.isSuperAdmin) {
      return;
    }

    this.organizationService.listOrganizations().subscribe({
      next: (orgs) => {
        this.organizations = orgs;
      },
      error: (error) => ErrorHandler.handleHttpError(error, this.toastrService),
    });
  }

  private loadCurrentOrganization(): void {
    const currentUser = this.authService.currentUserValue;
    if (!currentUser?.organization_id) {
      return;
    }

    this.organizationService.getOrganization(currentUser.organization_id).subscribe({
      next: (org) => {
        this.currentOrganization = org;
      },
      error: (error) => ErrorHandler.handleHttpError(error, this.toastrService),
    });
  }
}
