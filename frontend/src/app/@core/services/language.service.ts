import { Injectable } from '@angular/core';
import { TranslateService } from '@ngx-translate/core';
import { Observable } from 'rxjs';
import { map, startWith } from 'rxjs/operators';
import { AppLanguage, DEFAULT_LANGUAGE, SUPPORTED_LANGUAGES } from '../i18n/i18n.config';

@Injectable({
  providedIn: 'root',
})
export class LanguageService {
  private readonly STORAGE_KEY = 'starrocks_admin_lang';

  readonly languages = SUPPORTED_LANGUAGES;

  constructor(private translate: TranslateService) {}

  /** Load the initial language. Used by APP_INITIALIZER. */
  init(): Promise<unknown> {
    this.translate.setDefaultLang(DEFAULT_LANGUAGE);
    return this.use(this.resolveInitialLanguage()).toPromise();
  }

  get current(): AppLanguage {
    return (this.translate.currentLang as AppLanguage) || DEFAULT_LANGUAGE;
  }

  /** Emits the current language immediately and on every change. */
  get current$(): Observable<AppLanguage> {
    return this.translate.onLangChange.pipe(
      map(e => e.lang as AppLanguage),
      startWith(this.current),
    );
  }

  use(lang: AppLanguage): Observable<unknown> {
    try {
      localStorage.setItem(this.STORAGE_KEY, lang);
    } catch (e) {
      // Storage unavailable; the choice only lasts for this session.
    }
    document.documentElement.lang = lang;
    return this.translate.use(lang);
  }

  private resolveInitialLanguage(): AppLanguage {
    let saved: string | null = null;
    try {
      saved = localStorage.getItem(this.STORAGE_KEY);
    } catch (e) {
      saved = null;
    }
    if (this.isSupported(saved)) {
      return saved;
    }
    const browser = (navigator.language || '').toLowerCase();
    return browser.startsWith('zh') ? 'zh-CN' : DEFAULT_LANGUAGE;
  }

  private isSupported(lang: string | null): lang is AppLanguage {
    return SUPPORTED_LANGUAGES.some(l => l.value === lang);
  }
}

export function initLanguage(languageService: LanguageService): () => Promise<unknown> {
  return () => languageService.init();
}