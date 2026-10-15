import { Component, OnDestroy, OnInit } from '@angular/core';
import { NbDialogService, NbToastrService } from '@nebular/theme';
import { LocalDataSource } from 'ng2-smart-table';
import { forkJoin, of, Subject } from 'rxjs';
import { finalize, map, switchMap, takeUntil } from 'rxjs/operators';

import {
  CreateUserPayload,
  UpdateUserPayload,
  UserService,
  UserWithRoles,
} from '../../../@core/data/user.service';
import { ErrorHandler } from '../../../@core/utils/error-handler';
import { PermissionService } from '../../../@core/data/permission.service';
import { RoleService, RoleWithPermissions } from '../../../@core/data/role.service';
import { OrganizationService, Organization } from '../../../@core/data/organization.service';
import { AuthService } from '../../../@core/data/auth.service';
import { UsersRoleBadgeCellComponent } from './table/role-badge-cell.component';
import { UsersActionsCellComponent } from './table/actions-cell.component';
import {
  UserFormDialogComponent,
  UserFormDialogResult,
} from './user-form/user-form-dialog.component';
import { ConfirmDialogService } from '../../../@core/services/confirm-dialog.service';
import { TranslateService } from '@ngx-translate/core';

@Component({
  selector: 'ngx-users',
  templateUrl: './users.component.html',
  styleUrls: ['./users.component.scss'],
})
export class UsersComponent implements OnInit, OnDestroy {
  source: LocalDataSource = new LocalDataSource();
  loading = false;
  roleCatalog: RoleWithPermissions[] = [];
  roleCatalogLoading = false;
  organizations: Organization[] = [];
  currentOrganization?: Organization;
  isSuperAdmin = false;

  hasListPermission = false;
  canCreateUser = false;
  canUpdateUser = false;
  canDeleteUser = false;

  settings: any = {};

  private destroy$ = new Subject<void>();

  constructor(
    private userService: UserService,
    private permissionService: PermissionService,
    private roleService: RoleService,
    private organizationService: OrganizationService,
    private authService: AuthService,
    private dialogService: NbDialogService,
    private confirmDialogService: ConfirmDialogService,
    private toastrService: NbToastrService,
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

  loadUsers(): void {
    if (!this.hasListPermission) {
      this.loading = false;
      this.source.load([]);
      return;
    }

    this.loading = true;
    this.userService.listUsers().subscribe({
      next: (users) => {
        this.source.load(users);
        this.loading = false;
      },
      error: (error) => {
        ErrorHandler.handleHttpError(error, this.toastrService);
        this.loading = false;
      },
    });
  }

  private loadRoleCatalog(): void {
    if (this.roleCatalog.length || this.roleCatalogLoading) {
      return;
    }

    this.roleCatalogLoading = true;
    this.userService
      .listRoles()
      .pipe(
        switchMap((roles) => {
          if (!roles.length) {
            return of([] as RoleWithPermissions[]);
          }
          return forkJoin(
            roles.map((role) =>
              this.roleService
                .getRolePermissions(role.id)
                .pipe(map((permissions) => ({ ...role, permissions }))),
            ),
          );
        }),
        finalize(() => (this.roleCatalogLoading = false)),
      )
      .subscribe({
        next: (roles) => {
          this.roleCatalog = roles;
        },
        error: (error) => ErrorHandler.handleHttpError(error, this.toastrService),
      });
  }

  private loadOrganizations(): void {
    this.organizationService.listOrganizations().subscribe({
      next: (orgs) => {
        this.organizations = orgs;
      },
      error: (error) => ErrorHandler.handleHttpError(error, this.toastrService),
    });
  }

  private loadCurrentOrganization(): void {
    const currentUser = this.authService.currentUserValue;
    if (!currentUser) {
      return;
    }

    // For org admins, load their organization
    if (!this.isSuperAdmin && currentUser.organization_id) {
      this.organizationService.getOrganization(currentUser.organization_id).subscribe({
        next: (org) => {
          this.currentOrganization = org;
        },
        error: (error) => ErrorHandler.handleHttpError(error, this.toastrService),
      });
    }
  }

  openCreateUser(): void {
    if (!this.canCreateUser) {
      return;
    }

    if (!this.roleCatalog.length) {
      this.loadRoleCatalog();
      this.toastrService.info(
        this.translate.instant('system.users.rolesLoading'),
        this.translate.instant('system.shared.notice'),
      );
      return;
    }

    const dialogRef = this.dialogService.open(UserFormDialogComponent, {
      context: {
        mode: 'create',
        roles: this.roleCatalog,
        organizations: this.organizations,
        currentOrganization: this.currentOrganization,
      },
      closeOnBackdropClick: false,
      autoFocus: false,
    });

    dialogRef.onClose.subscribe((result?: UserFormDialogResult) => {
      if (!result) {
        return;
      }
      this.createUser(result);
    });
  }

  openEditUser(user: UserWithRoles): void {
    if (!this.canUpdateUser) {
      return;
    }

    if (!this.roleCatalog.length) {
      this.loadRoleCatalog();
      this.toastrService.info(
        this.translate.instant('system.users.rolesLoading'),
        this.translate.instant('system.shared.notice'),
      );
      return;
    }

    const dialogRef = this.dialogService.open(UserFormDialogComponent, {
      context: {
        mode: 'edit',
        roles: this.roleCatalog,
        organizations: this.organizations,
        currentOrganization: this.currentOrganization,
        user,
      },
      closeOnBackdropClick: false,
      autoFocus: false,
    });

    dialogRef.onClose.subscribe((result?: UserFormDialogResult) => {
      if (!result) {
        return;
      }
      this.updateUser(user.id, result);
    });
  }

  onDeleteUser(user: UserWithRoles): void {
    if (!this.canDeleteUser) {
      return;
    }

    this.confirmDialogService.confirmDelete(user.username)
      .subscribe(confirmed => {
        if (confirmed) {
          this.userService.deleteUser(user.id).subscribe({
            next: () => {
              this.toastrService.success(
                this.translate.instant('system.users.deleted'),
                this.translate.instant('system.shared.success'),
              );
              this.loadUsers();
            },
            error: (error) => ErrorHandler.handleHttpError(error, this.toastrService),
          });
        }
      });
  }

  private createUser(result: UserFormDialogResult): void {
    if (result.mode !== 'create') {
      return;
    }

    const payload = result.payload as CreateUserPayload;
    this.userService.createUser(payload).subscribe({
      next: () => {
        this.toastrService.success(
          this.translate.instant('system.users.created'),
          this.translate.instant('system.shared.success'),
        );
        this.loadUsers();
      },
      error: (error) => ErrorHandler.handleHttpError(error, this.toastrService),
    });
  }

  private updateUser(userId: number, result: UserFormDialogResult): void {
    if (result.mode !== 'edit') {
      return;
    }

    const payload = result.payload as UpdateUserPayload;
    this.userService.updateUser(userId, payload).subscribe({
      next: () => {
        this.toastrService.success(
          this.translate.instant('system.users.updated'),
          this.translate.instant('system.shared.success'),
        );
        this.loadUsers();
      },
      error: (error) => ErrorHandler.handleHttpError(error, this.toastrService),
    });
  }

  private applyPermissionState(): void {
    this.hasListPermission = this.permissionService.hasPermission('api:users:list');
    this.canCreateUser = this.permissionService.hasPermission('api:users:create');
    this.canUpdateUser = this.permissionService.hasPermission('api:users:update');
    this.canDeleteUser = this.permissionService.hasPermission('api:users:delete');
    this.isSuperAdmin = this.authService.isSuperAdmin();

    this.applyTableSettings();

    this.loadUsers();

    if (this.canCreateUser || this.canUpdateUser) {
      this.loadRoleCatalog();
    }

    // Load organizations for super admin
    if (this.isSuperAdmin) {
      this.loadOrganizations();
    }

    // Load current organization for org admin
    this.loadCurrentOrganization();
  }

  private applyTableSettings(): void {
    const settings = this.buildTableSettings();

    settings.columns.actions.onComponentInitFunction = (
      component: UsersActionsCellComponent,
    ) => {
      component.editUser.subscribe((row: UserWithRoles) => this.openEditUser(row));
      component.deleteUser.subscribe((row: UserWithRoles) => this.onDeleteUser(row));
    };

    this.settings = settings;
  }

  private buildTableSettings(): any {
    const t = (key: string) => this.translate.instant(key);
    return {
      mode: 'external',
      hideSubHeader: false,
      noDataMessage: this.hasListPermission
        ? t('system.users.table.noData')
        : t('system.users.table.noPermission'),
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
          width: '6%',
        },
        username: {
          title: t('system.users.table.username'),
          type: 'string',
          width: '12%',
        },
        email: {
          title: t('system.users.table.email'),
          type: 'string',
          width: '18%',
        },
        organization_name: {
          title: t('system.users.table.organization'),
          type: 'string',
          width: '13%',
          valuePrepareFunction: (name: string) => name || '-',
        },
        is_org_admin: {
          title: t('system.users.table.orgAdmin'),
          type: 'html',
          width: '8%',
          valuePrepareFunction: (isAdmin: boolean) => {
            return isAdmin
              ? `<span class="badge badge-success">${t('system.shared.yes')}</span>`
              : `<span class="badge badge-basic">${t('system.shared.no')}</span>`;
          },
        },
        roles: {
          title: t('system.users.table.roles'),
          type: 'custom',
          width: '18%',
          renderComponent: UsersRoleBadgeCellComponent,
          filter: false,
          sort: false,
        },
        created_at: {
          title: t('system.users.table.createdAt'),
          type: 'string',
          width: '12%',
          valuePrepareFunction: (date: string) => new Date(date).toLocaleString(this.translate.currentLang || 'en'),
        },
        actions: {
          title: t('system.users.table.actions'),
          type: 'custom',
          width: '10%',
          renderComponent: UsersActionsCellComponent,
          filter: false,
          sort: false,
          valuePrepareFunction: () => ({
            canEdit: this.canUpdateUser,
            canDelete: this.canDeleteUser,
          }),
        },
      },
    };
  }
}

