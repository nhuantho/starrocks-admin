import { Component, OnInit } from '@angular/core';
import { Router } from '@angular/router';
import { NbToastrService } from '@nebular/theme';
import { AuthService, User } from '../../@core/data/auth.service';
import { ApiService } from '../../@core/data/api.service';
import { DiceBearService } from '../../@core/services/dicebear.service';
import { TranslateService } from '@ngx-translate/core';

@Component({
  selector: 'ngx-user-settings',
  templateUrl: './user-settings.component.html',
  styleUrls: ['./user-settings.component.scss']
})
export class UserSettingsComponent implements OnInit {
  loading = false;
  submitted = false;
  currentUser: User | null = null;
  isFirstLogin = false;
  
  userForm = {
    username: '',
    email: '',
    avatar: '',
    currentPassword: '',
    newPassword: '',
    confirmPassword: ''
  };

  errors: string[] = [];
  showPasswordFields = false;
  showAvatarSelection = false;
  selectedAvatarStyle = 'avataaars';

  // DiceBear头像选项
  availableAvatars: string[] = [];
  avatarStyles = this.diceBearService.avatarStyles;

  constructor(
    private authService: AuthService,
    private apiService: ApiService,
    private toastrService: NbToastrService,
    private router: Router,
    private diceBearService: DiceBearService,
    private translate: TranslateService,
  ) {}

  ngOnInit() {
    this.loadUserInfo();
  }

  loadUserInfo() {
    this.loading = true;
    this.authService.getMe().subscribe({
      next: (user: any) => {
        this.currentUser = user;
        this.userForm.username = user.username;
        this.userForm.email = user.email || '';
        this.userForm.avatar = user.avatar || this.availableAvatars[0];
        this.loading = false;
        
        // Check if this is first login - show toast notification
        if (user.first_log) {
          this.isFirstLogin = true;
          this.showPasswordFields = true; // Auto-show password fields for first login
          this.toastrService.warning(this.translate.instant('system.userSettings.firstLoginWarning'), this.translate.instant('system.shared.notice'), {
            duration: 0, // Set to 0 so the toast doesn't auto-close
            destroyByClick: true,
          });
        }
      },
      error: (error) => {
        this.toastrService.danger(this.translate.instant('system.userSettings.loadFailed'), this.translate.instant('system.shared.error'));
        this.loading = false;
      }
    });
  }

  selectAvatar(avatar: string) {
    this.userForm.avatar = avatar;
  }

  generateAvatarOptions() {
    this.availableAvatars = this.diceBearService.generateAvatarOptions(6, this.selectedAvatarStyle);
  }

  onAvatarStyleChange() {
    this.generateAvatarOptions();
  }

  toggleAvatarSelection() {
    this.showAvatarSelection = !this.showAvatarSelection;
    if (this.showAvatarSelection && this.availableAvatars.length === 0) {
      this.generateAvatarOptions();
    }
  }

  togglePasswordFields() {
    this.showPasswordFields = !this.showPasswordFields;
    if (!this.showPasswordFields) {
      this.userForm.currentPassword = '';
      this.userForm.newPassword = '';
      this.userForm.confirmPassword = '';
    }
  }

  onSubmit() {
    this.errors = [];
    this.submitted = true;

    // Validation
    if (!this.userForm.username || !this.userForm.email) {
      this.errors.push(this.translate.instant('system.userSettings.errors.usernameEmailRequired'));
      this.submitted = false;
      return;
    }

    // Validate email format
    const emailPattern = /^[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,4}$/;
    if (!emailPattern.test(this.userForm.email)) {
      this.errors.push(this.translate.instant('system.userSettings.errors.emailInvalid'));
      this.submitted = false;
      return;
    }

    // If changing password, validate password fields
    if (this.showPasswordFields) {
      if (!this.userForm.currentPassword) {
        this.errors.push(this.translate.instant('system.userSettings.errors.currentPasswordRequired'));
        this.submitted = false;
        return;
      }

      if (!this.userForm.newPassword) {
        this.errors.push(this.translate.instant('system.userSettings.errors.newPasswordRequired'));
        this.submitted = false;
        return;
      }

      if (this.userForm.newPassword.length < 6) {
        this.errors.push(this.translate.instant('system.userSettings.errors.newPasswordMinLength'));
        this.submitted = false;
        return;
      }

      // Password complexity requirements (only for password reset/change)
      const hasUpperCase = /[A-Z]/.test(this.userForm.newPassword);
      const hasLowerCase = /[a-z]/.test(this.userForm.newPassword);
      const hasNumber = /[0-9]/.test(this.userForm.newPassword);
      const hasSpecialChar = /[!@#$%^&*()_+=\[\]{};':"\\|,.<>\/?-]/.test(this.userForm.newPassword);

      if (!hasUpperCase) {
        this.errors.push(this.translate.instant('system.userSettings.errors.newPasswordUppercase'));
        this.submitted = false;
        return;
      }

      if (!hasLowerCase) {
        this.errors.push(this.translate.instant('system.userSettings.errors.newPasswordLowercase'));
        this.submitted = false;
        return;
      }

      if (!hasNumber) {
        this.errors.push(this.translate.instant('system.userSettings.errors.newPasswordNumber'));
        this.submitted = false;
        return;
      }

      if (!hasSpecialChar) {
        this.errors.push(this.translate.instant('system.userSettings.errors.newPasswordSpecial'));
        this.submitted = false;
        return;
      }

      if (this.userForm.newPassword !== this.userForm.confirmPassword) {
        this.errors.push(this.translate.instant('system.userSettings.errors.passwordMismatch'));
        this.submitted = false;
        return;
      }
    }

    // Prepare update data
    const updateData: any = {
      username: this.userForm.username,
      email: this.userForm.email,
      avatar: this.userForm.avatar
    };

    if (this.showPasswordFields && this.userForm.newPassword) {
      updateData.current_password = this.userForm.currentPassword;
      updateData.new_password = this.userForm.newPassword;
    }

    // Check if password is being changed
    const isChangingPassword = this.showPasswordFields && this.userForm.newPassword;

    // Call API
    this.apiService.put(`/auth/me`, updateData).subscribe({
      next: (response: any) => {
        this.submitted = false;
        
        // If password was changed, logout and redirect to login
        if (isChangingPassword) {
          this.toastrService.success(this.translate.instant('system.userSettings.passwordChanged'), this.translate.instant('system.shared.success'));
          setTimeout(() => {
            // Clear auth data and redirect to login
            localStorage.removeItem('jwt_token');
            localStorage.removeItem('current_user');
            this.router.navigate(['/auth/login']);
          }, 1500);
        } else {
          // Just show success message
          this.toastrService.success(this.translate.instant('system.userSettings.updated'), this.translate.instant('system.shared.success'));
          
          // Fetch latest user info from database to ensure we have the latest data
          this.authService.getMe().subscribe({
            next: (user) => {
              // Update current user in AuthService to trigger header update
              this.authService.updateCurrentUser(user);
              
              // Update form with latest data
              this.userForm.username = user.username;
              this.userForm.email = user.email || '';
              this.userForm.avatar = (user as any).avatar || this.availableAvatars[0];
              
              // Reset password fields
              this.showPasswordFields = false;
              this.userForm.currentPassword = '';
              this.userForm.newPassword = '';
              this.userForm.confirmPassword = '';
              
              // Redirect to cluster list page after successful update
              setTimeout(() => {
                this.router.navigate(['/pages/starrocks/clusters']);
              }, 1500);
            },
            error: (error) => {
              console.error('Failed to reload user info:', error);
              // Even if reload fails, reset password fields
              this.showPasswordFields = false;
              this.userForm.currentPassword = '';
              this.userForm.newPassword = '';
              this.userForm.confirmPassword = '';
              
              // Still redirect to cluster list page even if reload fails
              setTimeout(() => {
                this.router.navigate(['/pages/starrocks/clusters']);
              }, 1500);
            }
          });
        }
      },
      error: (error) => {
        this.submitted = false;
        // Show error in alert (form validation errors use alert, API errors use alert too for consistency)
        const errorMessage = error.error?.message || this.translate.instant('system.userSettings.updateFailed');
        this.errors = [errorMessage];
        // Don't show toast for API errors since we already show alert
        // this.toastrService.danger(errorMessage, '错误');
      }
    });
  }

  onCancel() {
    this.router.navigate(['/pages/starrocks/dashboard']);
  }
}

