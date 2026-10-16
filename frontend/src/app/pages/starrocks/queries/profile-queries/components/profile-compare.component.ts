import { ChangeDetectionStrategy, ChangeDetectorRef, Component, Input, OnChanges, OnDestroy } from '@angular/core';
import { Subscription } from 'rxjs';
import { NodeService, ProfileComparison } from '../../../../../@core/data/node.service';
import { ErrorHandler } from '../../../../../@core/utils/error-handler';
import { formatDelta, formatMs } from '../profile-diagnostics.utils';

// Before/after comparison of two profiles: phases, operators, new and resolved findings.
@Component({
  selector: 'ngx-profile-compare',
  templateUrl: './profile-compare.component.html',
  styleUrls: ['./profile-compare.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class ProfileCompareComponent implements OnChanges, OnDestroy {
  @Input() baseQueryId = '';
  @Input() targetQueryId = '';

  comparison?: ProfileComparison;
  loading = false;
  error = '';
  readonly formatMs = formatMs;
  readonly formatDelta = formatDelta;
  private subscription?: Subscription;

  constructor(private nodeService: NodeService, private cdr: ChangeDetectorRef) {}

  ngOnChanges(): void {
    this.subscription?.unsubscribe();
    this.comparison = undefined;
    this.error = '';
    if (!this.baseQueryId || !this.targetQueryId) {
      return;
    }
    this.loading = true;
    this.subscription = this.nodeService.compareProfiles(this.baseQueryId, this.targetQueryId).subscribe({
      next: comparison => {
        this.comparison = comparison;
        this.loading = false;
        this.cdr.markForCheck();
      },
      error: error => {
        this.error = ErrorHandler.extractErrorMessage(error);
        this.loading = false;
        this.cdr.markForCheck();
      },
    });
  }

  ngOnDestroy(): void {
    this.subscription?.unsubscribe();
  }

  deltaClass(value: number | undefined | null): string {
    if (value === undefined || value === null || value === 0) {
      return '';
    }
    return value > 0 ? 'worse' : 'better';
  }
}
