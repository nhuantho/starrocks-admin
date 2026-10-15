import { Injectable, Injector } from '@angular/core';
import {
  HttpRequest,
  HttpHandler,
  HttpEvent,
  HttpInterceptor,
  HttpErrorResponse,
} from '@angular/common/http';
import { Router } from '@angular/router';
import { Observable, throwError } from 'rxjs';
import { catchError } from 'rxjs/operators';
import { AuthService } from '../data/auth.service';
import { NbToastrService } from '@nebular/theme';
import { TranslateService } from '@ngx-translate/core';

/** Backend messages (zh / en) signalling that the password must be changed on first login. */
const FIRST_LOGIN_MESSAGES = ['首次登录必须修改密码', 'password change required on first login'];
const FIRST_LOGIN_CODE = 'FIRST_LOGIN_PASSWORD_CHANGE_REQUIRED';

@Injectable()
export class JwtInterceptor implements HttpInterceptor {
  constructor(
    private authService: AuthService,
    private toastrService: NbToastrService,
    private router: Router,
    private injector: Injector,
  ) {}

  /**
   * Resolve TranslateService lazily: injecting it in the constructor would create a
   * DI cycle (TranslateService -> HttpClient -> interceptors).
   */
  private t(key: string, fallback: string): string {
    try {
      const value = this.injector.get(TranslateService).instant(key);
      return value && value !== key ? value : fallback;
    } catch (e) {
      return fallback;
    }
  }

  intercept(request: HttpRequest<unknown>, next: HttpHandler): Observable<HttpEvent<unknown>> {
    // Tell the backend which language to use for messages. Read from <html lang>
    // (set by LanguageService) to avoid a DI cycle through TranslateService -> HttpClient.
    const lang = document.documentElement.lang;
    if (lang && !request.headers.has('Accept-Language')) {
      request = request.clone({ setHeaders: { 'Accept-Language': lang } });
    }

    // Add authorization header with JWT token if available
    const token = this.authService.token;
    if (token) {
      request = request.clone({
        setHeaders: {
          Authorization: `Bearer ${token}`,
        },
      });
    }

    return next.handle(request).pipe(
      catchError((error: HttpErrorResponse) => {
        if (error.status === 401) {
          const errorMessage = error.error?.message || 'Unauthorized';
          // Check if this is an authentication error (missing/invalid token) or permission error
          const isAuthError = errorMessage.includes('Missing authorization header') ||
                              errorMessage.includes('Invalid authorization header') ||
                              errorMessage.includes('JWT verification failed') ||
                              errorMessage.includes('Token expired') ||
                              errorMessage.includes('Invalid credentials');

          const lowerMessage = String(errorMessage).toLowerCase();
          const isFirstLoginError = error.error?.error_code === FIRST_LOGIN_CODE ||
                                    FIRST_LOGIN_MESSAGES.some(m => lowerMessage.includes(m.toLowerCase()));

          if (this.authService.isAuthenticated()) {
            if (isAuthError) {
              // Token is invalid/expired - clear auth and redirect to login
              this.toastrService.danger(
                this.t('common.auth.sessionExpired', 'Your session has expired. Please log in again.'),
                this.t('common.auth.authFailed', 'Authentication Failed'),
              );
              const safeUrl = this.authService.normalizeReturnUrl(this.router.url);
              this.authService.logout({ returnUrl: safeUrl });
            } else if (isFirstLoginError) {
              // First login - show nothing
            } else {
              // Permission denied - show error message but don't logout
              this.toastrService.danger(errorMessage, this.t('common.auth.permissionDenied', 'Permission Denied'));
            }
          } else {
            // User not authenticated - silently ignore (user has logged out)
            // This happens when user logs out but components are still running auto-refresh
          }
        }
        // RxJS 6 expects the error itself; a factory would reach subscribers as a function.
        return throwError(error);
      }),
    );
  }
}

