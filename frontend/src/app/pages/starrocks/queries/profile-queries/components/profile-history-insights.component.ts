import { ChangeDetectionStrategy, ChangeDetectorRef, Component, EventEmitter, Input, OnChanges, OnDestroy, Output } from '@angular/core';
import { Subscription, forkJoin, of } from 'rxjs';
import { catchError } from 'rxjs/operators';
import { NodeService, ProfileSnapshotSummary, RecurringQuery } from '../../../../../@core/data/node.service';
import { formatMs } from '../profile-diagnostics.utils';

export interface ComparePair {
  base: string;
  target: string;
}

// Recurring statements (grouped by SQL fingerprint) and archived snapshots. Loads on demand.
@Component({
  selector: 'ngx-profile-history-insights',
  templateUrl: './profile-history-insights.component.html',
  styleUrls: ['./profile-history-insights.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class ProfileHistoryInsightsComponent implements OnChanges, OnDestroy {
  @Input() clusterId?: number | null;
  @Output() openDiagnostics = new EventEmitter<string>();
  @Output() compare = new EventEmitter<ComparePair>();

  open = false;
  loading = false;
  loaded = false;
  recurring: RecurringQuery[] = [];
  snapshots: ProfileSnapshotSummary[] = [];
  snapshotsEnabled = false;
  selected: string[] = [];
  manualBase = '';
  manualTarget = '';
  readonly formatMs = formatMs;
  private subscription?: Subscription;

  constructor(private nodeService: NodeService, private cdr: ChangeDetectorRef) {}

  ngOnChanges(): void {
    // A different cluster invalidates everything loaded so far.
    this.subscription?.unsubscribe();
    this.loaded = false;
    this.recurring = [];
    this.snapshots = [];
    this.selected = [];
    if (this.open) {
      this.load();
    }
  }

  ngOnDestroy(): void {
    this.subscription?.unsubscribe();
  }

  toggle(): void {
    this.open = !this.open;
    if (this.open && !this.loaded) {
      this.load();
    }
  }

  load(): void {
    if (!this.clusterId) {
      return;
    }
    this.loading = true;
    this.subscription?.unsubscribe();
    this.subscription = forkJoin({
      recurring: this.nodeService.getRecurringQueries(20).pipe(catchError(() => of([] as RecurringQuery[]))),
      snapshots: this.nodeService.listProfileSnapshots(100).pipe(catchError(() => of({ enabled: false, snapshots: [] as ProfileSnapshotSummary[] }))),
    }).subscribe(({ recurring, snapshots }) => {
      this.recurring = recurring;
      this.snapshots = snapshots.snapshots;
      this.snapshotsEnabled = snapshots.enabled;
      this.loading = false;
      this.loaded = true;
      this.cdr.markForCheck();
    });
  }

  toggleSelected(queryId: string): void {
    this.selected = this.selected.includes(queryId)
      ? this.selected.filter(id => id !== queryId)
      : [...this.selected, queryId].slice(-2);
  }

  // Snapshots are listed newest first, so the older selection is the baseline.
  compareSelected(): void {
    if (this.selected.length !== 2) {
      return;
    }
    const order = this.snapshots.map(s => s.query_id);
    const [first, second] = [...this.selected].sort((a, b) => order.indexOf(b) - order.indexOf(a));
    this.compare.emit({ base: first, target: second });
  }

  compareManual(): void {
    const base = this.manualBase.trim();
    const target = this.manualTarget.trim();
    if (base && target) {
      this.compare.emit({ base, target });
    }
  }

  compareRecurring(group: RecurringQuery): void {
    if (group.query_ids.length >= 2) {
      this.compare.emit({ base: group.query_ids[1], target: group.query_ids[0] });
    }
  }
}
