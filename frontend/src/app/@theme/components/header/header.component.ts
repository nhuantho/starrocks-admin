import { Component, OnDestroy, OnInit } from '@angular/core';
import { Router } from '@angular/router';
import { NbMediaBreakpointsService, NbMenuService, NbSidebarService, NbThemeService, NbToastrService } from '@nebular/theme';

import { LayoutService } from '../../../@core/utils';
import { AuthService } from '../../../@core/data/auth.service';
import { map, takeUntil, filter } from 'rxjs/operators';
import { Subject } from 'rxjs';
import { TranslateService } from '@ngx-translate/core';
import { LanguageService } from '../../../@core/services/language.service';
import { AppLanguage } from '../../../@core/i18n/i18n.config';

@Component({
  selector: 'ngx-header',
  styleUrls: ['./header.component.scss'],
  templateUrl: './header.component.html',
})
export class HeaderComponent implements OnInit, OnDestroy {

  private destroy$: Subject<void> = new Subject<void>();
  userPictureOnly: boolean = false;
  user: any;

  themes = [
    { value: 'default', name: 'common.theme.default' },
    { value: 'dark', name: 'common.theme.dark' },
    { value: 'cosmic', name: 'common.theme.cosmic' },
    { value: 'corporate', name: 'common.theme.corporate' },
  ];

  languages = this.languageService.languages;
  currentLanguage: AppLanguage = this.languageService.current;

  currentTheme = 'default';

  userMenu = this.buildUserMenu();

  constructor(
    private sidebarService: NbSidebarService,
    private menuService: NbMenuService,
    private themeService: NbThemeService,
    private authService: AuthService,
    private layoutService: LayoutService,
    private breakpointService: NbMediaBreakpointsService,
    private router: Router,
    private toastr: NbToastrService,
    private translate: TranslateService,
    private languageService: LanguageService,
  ) {
  }

  ngOnInit() {
    this.currentTheme = this.themeService.currentTheme;

    this.languageService.current$
      .pipe(takeUntil(this.destroy$))
      .subscribe(lang => {
        this.currentLanguage = lang;
        this.userMenu = this.buildUserMenu();
      });

    // Get current user info
    this.authService.currentUser
      .pipe(takeUntil(this.destroy$))
      .subscribe(currentUser => {
        if (currentUser) {
          this.user = {
            name: currentUser.username || 'admin',
            picture: currentUser.avatar || 'assets/images/nick.png', // 使用用户的头像，如果没有则使用默认头像
          };
        } else {
          // 如果未登录，设置默认用户
          this.user = {
            name: 'admin',
            picture: 'assets/images/nick.png',
          };
        }
      });

    // Handle user menu clicks
    this.menuService.onItemClick()
      .pipe(
        filter(({ tag }) => tag === 'user-context-menu'),
        map(({ item }) => item),
        takeUntil(this.destroy$),
      )
      .subscribe(item => {
        if (item.data) {
          switch (item.data.id) {
            case 'settings':
              this.router.navigate(['/pages/user-settings']);
              break;
            case 'logout':
              this.logout();
              break;
          }
        }
      });

    const { xl } = this.breakpointService.getBreakpointsMap();
    this.themeService.onMediaQueryChange()
      .pipe(
        map(([, currentBreakpoint]) => currentBreakpoint.width < xl),
        takeUntil(this.destroy$),
      )
      .subscribe((isLessThanXl: boolean) => this.userPictureOnly = isLessThanXl);

    this.themeService.onThemeChange()
      .pipe(
        map(({ name }) => name),
        takeUntil(this.destroy$),
      )
      .subscribe(themeName => this.currentTheme = themeName);
  }

  ngOnDestroy() {
    this.destroy$.next();
    this.destroy$.complete();
  }

  changeLanguage(lang: AppLanguage) {
    this.languageService.use(lang).subscribe();
  }

  private buildUserMenu() {
    return [
      { title: this.translate.instant('common.header.userSettings'), icon: 'settings-outline', data: { id: 'settings' } },
      { title: this.translate.instant('common.header.logout'), icon: 'log-out-outline', data: { id: 'logout' } },
    ];
  }

  changeTheme(themeName: string) {
    this.themeService.changeTheme(themeName);
  }

  toggleSidebar(): boolean {
    this.sidebarService.toggle(true, 'menu-sidebar');
    this.layoutService.changeLayoutSize();

    return false;
  }

  navigateHome() {
    this.menuService.navigateHome();
    return false;
  }

  logout() {
    this.toastr.success(
      this.translate.instant('common.header.logoutSuccess'),
      this.translate.instant('common.notice'),
    );
    setTimeout(() => {
      this.authService.logout();
    }, 500);
  }
}
