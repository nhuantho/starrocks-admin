import { ChangeDetectionStrategy, ChangeDetectorRef, Component, EventEmitter, OnDestroy, OnInit, Output } from '@angular/core';
import { Subscription } from 'rxjs';
import { NodeService, ProfileSettings } from '../../../../../@core/data/node.service';

// Shown when a profile cannot be opened: the cluster's current profile settings and what to check.
@Component({
  selector: 'ngx-profile-guidance',
  templateUrl: './profile-guidance.component.html',
  styleUrls: ['./profile-guidance.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class ProfileGuidanceComponent implements OnInit, OnDestroy {
  @Output() copyText = new EventEmitter<string>();

  settings?: ProfileSettings;
  failed = false;
  private subscription?: Subscription;

  constructor(private nodeService: NodeService, private cdr: ChangeDetectorRef) {}

  ngOnInit(): void {
    this.subscription = this.nodeService.getProfileSettings().subscribe({
      next: settings => {
        this.settings = settings;
        this.cdr.markForCheck();
      },
      error: () => {
        this.failed = true;
        this.cdr.markForCheck();
      },
    });
  }

  ngOnDestroy(): void {
    this.subscription?.unsubscribe();
  }

  entries(map: { [key: string]: string } | undefined): Array<[string, string]> {
    return Object.entries(map || {});
  }
}
