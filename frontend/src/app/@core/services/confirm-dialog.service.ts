import { Injectable } from '@angular/core';
import { NbDialogService } from '@nebular/theme';
import { TranslateService } from '@ngx-translate/core';
import { Observable } from 'rxjs';
import { ConfirmDialogComponent } from '../components/confirm-dialog/confirm-dialog.component';

@Injectable({
  providedIn: 'root'
})
export class ConfirmDialogService {
  constructor(
    private dialogService: NbDialogService,
    private translate: TranslateService,
  ) {}

  confirm(
    title: string,
    message: string,
    confirmText?: string,
    cancelText?: string,
    confirmStatus: string = 'primary'
  ): Observable<boolean> {
    return this.dialogService.open(ConfirmDialogComponent, {
      context: {
        title,
        message,
        confirmText: confirmText || this.translate.instant('common.confirm'),
        cancelText: cancelText || this.translate.instant('common.cancel'),
        confirmStatus
      },
      hasBackdrop: true,
      closeOnBackdropClick: false,
      closeOnEsc: true,
      autoFocus: true
    }).onClose;
  }

  confirmDelete(itemName: string, additionalWarning?: string): Observable<boolean> {
    const question = this.translate.instant('common.confirmDialog.deleteMessage', { name: itemName });
    const message = additionalWarning
      ? `${question}\n\n${additionalWarning}`
      : question;

    return this.confirm(
      this.translate.instant('common.confirmDialog.deleteTitle'),
      message,
      this.translate.instant('common.delete'),
      this.translate.instant('common.cancel'),
      'danger'
    );
  }
}
