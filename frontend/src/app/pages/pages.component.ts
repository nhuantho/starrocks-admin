import { Component, OnInit } from '@angular/core';
import { Router, NavigationEnd } from '@angular/router';
import { NbMenuItem, NbMenuService } from '@nebular/theme';
import { TranslateService } from '@ngx-translate/core';
import { combineLatest } from 'rxjs';
import { filter, map, startWith } from 'rxjs/operators';

import { MENU_ITEMS } from './pages-menu';
import { AuthService } from '../@core/data/auth.service';
import { TabService } from '../@core/services/tab.service';
import { MenuFilterService } from '../@core/services/menu-filter.service';
import { PermissionService } from '../@core/data/permission.service';

@Component({
  selector: 'ngx-pages',
  styleUrls: ['pages.component.scss'],
  template: `
    <ngx-one-column-layout>
      <nb-menu [items]="menu" tag="menu" (itemClick)="onMenuClick($event)"></nb-menu>
      <router-outlet></router-outlet>
    </ngx-one-column-layout>
  `,
})
export class PagesComponent implements OnInit {
  menu = MENU_ITEMS;

  constructor(
    private menuService: NbMenuService,
    private authService: AuthService,
    private router: Router,
    private tabService: TabService,
    private menuFilterService: MenuFilterService,
    private permissionService: PermissionService,
    private translate: TranslateService,
  ) {}

  ngOnInit() {
    // Filter menu items based on permissions, and re-translate titles on language change
    combineLatest([
      this.permissionService.permissions$,
      this.translate.onLangChange.pipe(startWith(null)),
    ]).subscribe(() => {
      this.menu = this.translateMenu(this.menuFilterService.filterMenuItems(MENU_ITEMS));
    });

    // Initialize permissions if not already initialized
    if (this.authService.isAuthenticated()) {
      this.permissionService.initPermissions().subscribe();
    }

    // Listen to menu item clicks
    this.menuService.onItemClick()
      .pipe(
        filter(({ tag }) => tag === 'menu'),
        map(({ item }) => item)
      )
      .subscribe(item => {
        if (item.data?.id === 'logout') {
          this.authService.logout();
        }
      });

    // Listen to route changes and add tabs
    // Only handle route changes when navigation is triggered by menu or direct URL
    this.router.events
      .pipe(filter(event => event instanceof NavigationEnd))
      .subscribe((event: NavigationEnd) => {
        // Skip if this is triggered by tab switching
        // Check if the URL matches any existing active tab (使用规范化比较)
        const activeTab = this.tabService.getActiveTab();
        if (activeTab) {
          // 规范化比较 URL（处理编码问题）
          const normalizedEventUrl = this.normalizeUrl(event.url);
          const normalizedActiveTabUrl = this.normalizeUrl(activeTab.url);
          if (normalizedActiveTabUrl === normalizedEventUrl) {
            // This navigation is from tab switching, skip adding new tab
            return;
          }
        }
        this.handleRouteChange(event.url);
      });
  }

  /**
   * 处理路由变化，自动添加Tab
   */
  private handleRouteChange(url: string): void {
    // 跳过登录页面
    if (url.includes('/auth/')) {
      return;
    }

    // 对于带查询参数的 system 路由，优先使用 inferTitleFromUrl 获取更具体的标题
    // title is a translation key when it starts with "menu."; otherwise a literal
    let title: string | null = null;
    if (url.includes('/starrocks/system') && url.includes('?')) {
      title = this.inferTitleFromUrl(url);
    }

    // 如果没有通过 URL 推断获得标题，尝试查找菜单项
    if (!title) {
      const menuItem = this.findMenuItemByUrl(url);
      if (menuItem) {
        title = menuItem.title;
      }
    }

    // 如果还是没有标题，尝试从 URL 推断
    if (!title) {
      title = this.inferTitleFromUrl(url);
    }

    // 如果获得了标题，添加 Tab
    if (title) {
      const tabId = this.generateTabId(title);
      const icon = this.getIconForUrl(url);
      // 路由变化时不再触发导航（因为已经在目标路由了）
      const isKey = title.startsWith('menu.');
      this.tabService.addTab({
        id: tabId,
        title: isKey ? this.translate.instant(title) : title,
        titleKey: isKey ? title : undefined,
        url: url,
        closable: true,
        pinned: false,
        icon: icon  // Add icon to tab
      }, false);
    }
  }

  /**
   * 规范化 URL，用于比较（处理编码问题）
   */
  private normalizeUrl(url: string): string {
    try {
      let decoded = url;
      try {
        decoded = decodeURIComponent(url);
      } catch (e) {
        decoded = url;
      }
      
      const [path, queryString] = decoded.split('?');
      
      let normalizedPath = path.replace(/\/+$/, '');
      if (!normalizedPath.startsWith('/')) {
        normalizedPath = '/' + normalizedPath;
      }
      
      if (queryString) {
        try {
          const params = new URLSearchParams(queryString);
          const sortedParams = Array.from(params.entries())
            .sort((a, b) => a[0].localeCompare(b[0]));
          const normalizedParams = new URLSearchParams(sortedParams);
          return normalizedPath + '?' + normalizedParams.toString();
        } catch (e) {
          return normalizedPath + '?' + queryString;
        }
      }
      
      return normalizedPath;
    } catch (e) {
      // 如果所有处理都失败，返回原始 URL
      return url;
    }
  }

  /**
   * 从URL推断标题
   */
  private inferTitleFromUrl(url: string): string | null {
    // 先解码 URL（处理可能的编码情况）
    try {
      url = decodeURIComponent(url);
    } catch (e) {
      // 如果解码失败，使用原始 URL
    }
    
    // 先分离路径和查询参数
    const [path, queryString] = url.split('?');
    const urlSegments = path.split('/').filter(segment => segment);
    
    // 处理StarRocks相关路由
    if (urlSegments.includes('starrocks')) {
      const lastSegment = urlSegments[urlSegments.length - 1];
      const prevSegment = urlSegments[urlSegments.length - 2] || '';
      
      // 特殊处理 system 路由，从查询参数中提取功能名称
      if (lastSegment === 'system' && queryString) {
        // 处理可能的编码查询参数
        let decodedQueryString = queryString;
        try {
          decodedQueryString = decodeURIComponent(queryString);
        } catch (e) {
          // 如果解码失败，使用原始查询字符串
        }
        const params = new URLSearchParams(decodedQueryString);
        const functionName = params.get('function');
        if (functionName) {
          // Known system functions map to menu.functionTabs.<name> (see menu.json)
          const knownFunctions = [
            'backends', 'frontends', 'brokers', 'statistic', 'dbs', 'tables', 'tablet_schema',
            'partitions', 'transactions', 'routine_loads', 'stream_loads', 'loads', 'load_error_hub',
            'catalog', 'resources', 'workload_groups', 'workload_sched_policy', 'compactions',
            'colocate_group', 'bdbje', 'small_files', 'trash', 'jobs', 'repositories',
          ];
          if (knownFunctions.includes(functionName)) {
            return `menu.functionTabs.${functionName}`;
          }
          return `${this.translate.instant('menu.systemFunctions')}: ${functionName}`;
        }
      }
      
      // 当路径为 /pages/starrocks/clusters/:id 时，避免使用数字作为标题
      if (prevSegment === 'clusters' && /^\d+$/.test(lastSegment)) {
        return 'menu.clusterDetail';
      }

      // 映射URL段到标题翻译 key
      const titleMap: { [key: string]: string } = {
        'dashboard': 'menu.clusterList',
        'overview': 'menu.clusterOverview',
        'frontends': 'menu.frontends',
        'backends': 'menu.backends',
        'execution': 'menu.queryExecution',
        'profiles': 'menu.profiles',
        'audit-logs': 'menu.auditLogs',
        'materialized-views': 'menu.materializedViews',
        'system': 'menu.systemFunctions',  // 对应 menu:system-functions
        'sessions': 'menu.sessions',
        'variables': 'menu.variables',
        'clusters': 'menu.clusterManagement',
        'new': 'menu.newCluster',
        'edit': 'menu.editCluster'
      };
      
      return titleMap[lastSegment] || lastSegment;
    }
    
    return null;
  }

  /**
   * 根据URL查找菜单项（包括父级菜单）
   */
  private findMenuItemByUrl(url: string): any {
    const findInMenu = (items: any[], parent: any = null): any => {
      for (const item of items) {
        if (item.link === url) {
          return item;
        }
        if (item.children) {
          const found = findInMenu(item.children, item);
          if (found) return found;
        }
      }
      return null;
    };

    return findInMenu(MENU_ITEMS);
  }

  /**
   * 根据URL获取菜单项图标
   */
  private getIconForUrl(url: string): string | undefined {
    const menuItem = this.findMenuItemByUrl(url);
    if (menuItem && menuItem.icon) {
      return menuItem.icon;
    }

    // For child routes, try to find parent icon
    const findParentIcon = (items: any[]): string | undefined => {
      for (const item of items) {
        if (item.children) {
          const childMatch = item.children.some((child: any) => child.link === url);
          if (childMatch && item.icon) {
            return item.icon;
          }
          const childResult = findParentIcon(item.children);
          if (childResult) return childResult;
        }
      }
      return undefined;
    };

    return findParentIcon(MENU_ITEMS);
  }

  /**
   * 生成Tab ID（基于标题，确保每个页面有唯一ID）
   */
  private generateTabId(title: string): string {
    // 使用标题生成固定ID，每个页面名称对应唯一ID
    return 'tab_' + title.replace(/[^a-zA-Z0-9\u4e00-\u9fa5]/g, '_');
  }

  /**
   * Deep-copy menu items with titles translated from their keys.
   */
  private translateMenu(items: NbMenuItem[]): NbMenuItem[] {
    return items.map(item => ({
      ...item,
      title: this.translate.instant(item.title),
      children: item.children ? this.translateMenu(item.children) : undefined,
    }));
  }

  onMenuClick(event: any) {
    if (event.item.data?.id === 'logout') {
      event.event.preventDefault();
      this.authService.logout();
    }
  }
}
