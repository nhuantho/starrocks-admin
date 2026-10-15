import { Component, Input } from '@angular/core';
import { NbDialogRef } from '@nebular/theme';
import { TranslateService } from '@ngx-translate/core';

@Component({
  selector: 'ngx-confirm-dialog',
  template: `
    <nb-card>
      <nb-card-header>{{ title }}</nb-card-header>
      <nb-card-body>
        <p style="white-space: pre-line;">{{ message }}</p>
      </nb-card-body>
      <nb-card-footer>
        <button nbButton status="basic" (click)="cancel()">{{ cancelLabel }}</button>
        <button nbButton [status]="confirmStatus" (click)="confirm()">{{ confirmLabel }}</button>
      </nb-card-footer>
    </nb-card>
  `,
  styles: [`
    nb-card {
      margin: 0;
      min-width: 400px;
      max-width: 600px;
    }
    
    nb-card-footer {
      display: flex;
      justify-content: flex-end;
      gap: 0.5rem;
    }
    
    p {
      margin: 0;
      line-height: 1.5;
    }
  `]
})
export class ConfirmDialogComponent {
  @Input() title: string;
  @Input() message: string;
  /** Defaults to the translated "Confirm" when not provided. */
  @Input() confirmText: string;
  /** Defaults to the translated "Cancel" when not provided. */
  @Input() cancelText: string;
  @Input() confirmStatus: string = 'primary';

  constructor(
    protected ref: NbDialogRef<ConfirmDialogComponent>,
    private translate: TranslateService,
  ) {}

  // Resolved at render time so the defaults follow the current language.
  get confirmLabel(): string {
    return this.confirmText || this.translate.instant('common.confirm');
  }

  get cancelLabel(): string {
    return this.cancelText || this.translate.instant('common.cancel');
  }

  cancel() {
    this.ref.close(false);
  }

  confirm() {
    this.ref.close(true);
  }
}
