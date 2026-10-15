import { HttpClient } from '@angular/common/http';
import { TranslateLoader } from '@ngx-translate/core';
import { forkJoin, Observable, of } from 'rxjs';
import { catchError, map } from 'rxjs/operators';

export type AppLanguage = 'en' | 'zh-CN';

export const DEFAULT_LANGUAGE: AppLanguage = 'en';

export const SUPPORTED_LANGUAGES: { value: AppLanguage; label: string }[] = [
  { value: 'en', label: 'English' },
  { value: 'zh-CN', label: '简体中文' },
];

/**
 * Translation namespaces. Each namespace lives in
 * `assets/i18n/<lang>/<namespace>.json` and is exposed under the
 * `<namespace>.` key prefix, e.g. `common.confirm`.
 */
export const I18N_NAMESPACES = [
  'common',
  'menu',
  'auth',
  'cluster',
  'sql',
  'profile',
  'audit',
  'mv',
  'sysfn',
  'system',
] as const;

export class NamespacedTranslateLoader implements TranslateLoader {
  constructor(private http: HttpClient, private prefix = 'assets/i18n') {}

  getTranslation(lang: string): Observable<any> {
    const requests = I18N_NAMESPACES.map(ns =>
      this.http.get(`${this.prefix}/${lang}/${ns}.json`).pipe(
        catchError(() => of({})),
        map(json => ({ [ns]: json })),
      ),
    );
    return forkJoin(requests).pipe(
      map(parts => Object.assign({}, ...parts)),
    );
  }
}

export function createTranslateLoader(http: HttpClient): TranslateLoader {
  return new NamespacedTranslateLoader(http);
}