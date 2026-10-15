import { NbMenuItem } from '@nebular/theme';

// Titles are translation keys; PagesComponent translates them at render time.

export const MENU_ITEMS: NbMenuItem[] = [
  {
    title: 'menu.clusterList',
    icon: 'list-outline',
    link: '/pages/starrocks/dashboard',
    home: true,
    data: { permission: 'menu:dashboard' },
  } as NbMenuItem & { data?: { permission: string } },
  {
    title: 'menu.clusterOverview',
    icon: 'activity-outline',
    link: '/pages/starrocks/overview',
    data: { permission: 'menu:overview' },
  } as NbMenuItem & { data?: { permission: string } },
  {
    title: 'menu.nodes',
    icon: 'hard-drive-outline',
    data: { permission: 'menu:nodes' },
    children: [
      {
        title: 'menu.frontends',
        link: '/pages/starrocks/frontends',
        data: { permission: 'menu:nodes:frontends' },
      } as NbMenuItem & { data?: { permission: string } },
      {
        title: 'menu.backends',
        link: '/pages/starrocks/backends',
        data: { permission: 'menu:nodes:backends' },
      } as NbMenuItem & { data?: { permission: string } },
    ],
  } as NbMenuItem & { data?: { permission: string } },
  {
    title: 'menu.queries',
    icon: 'code-outline',
    data: { permission: 'menu:queries' },
    children: [
      {
        title: 'menu.queryExecution',
        link: '/pages/starrocks/queries/execution',
        data: { permission: 'menu:queries:execution' },
      } as NbMenuItem & { data?: { permission: string } },
      {
        title: 'menu.profiles',
        link: '/pages/starrocks/queries/profiles',
        data: { permission: 'menu:queries:profiles' },
      } as NbMenuItem & { data?: { permission: string } },
      {
        title: 'menu.auditLogs',
        link: '/pages/starrocks/queries/audit-logs',
        data: { permission: 'menu:queries:audit-logs' },
      } as NbMenuItem & { data?: { permission: string } },
    ],
  } as NbMenuItem & { data?: { permission: string } },
  {
    title: 'menu.materializedViews',
    icon: 'cube-outline',
    link: '/pages/starrocks/materialized-views',
    data: { permission: 'menu:materialized-views' },
  } as NbMenuItem & { data?: { permission: string } },
  {
    title: 'menu.systemFunctions',
    icon: 'grid-outline',
    link: '/pages/starrocks/system',
    data: { permission: 'menu:system-functions' },
  } as NbMenuItem & { data?: { permission: string } },
  {
    title: 'menu.sessions',
    icon: 'person-outline',
    link: '/pages/starrocks/sessions',
    data: { permission: 'menu:sessions' },
  } as NbMenuItem & { data?: { permission: string } },
  {
    title: 'menu.variables',
    icon: 'settings-2-outline',
    link: '/pages/starrocks/variables',
    data: { permission: 'menu:variables' },
  } as NbMenuItem & { data?: { permission: string } },
  {
    title: 'menu.system',
    icon: 'settings-outline',
    data: { permission: 'menu:system' }, // Parent menu permission
    children: [
      {
        title: 'menu.users',
        link: '/pages/system/users',
        data: { permission: 'menu:system:users' },
      } as NbMenuItem & { data?: { permission: string } },
      {
        title: 'menu.roles',
        link: '/pages/system/roles',
        data: { permission: 'menu:system:roles' },
      } as NbMenuItem & { data?: { permission: string } },
      {
        title: 'menu.organizations',
        link: '/pages/system/organizations',
        data: { permission: 'menu:system:organizations' },
      } as NbMenuItem & { data?: { permission: string } },
    ],
  } as NbMenuItem & { data?: { permission: string } },
];
