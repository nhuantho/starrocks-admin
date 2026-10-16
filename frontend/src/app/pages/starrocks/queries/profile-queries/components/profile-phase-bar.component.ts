import { ChangeDetectionStrategy, Component, Input } from '@angular/core';
import { PhaseBreakdown } from '../../../../../@core/data/node.service';
import { formatMs, phaseBars } from '../profile-diagnostics.utils';

// Measured query phases (parse, plan, schedule, execute + fetch). Draws a stack only when the
// phases add up within the total; otherwise each phase gets its own bar.
@Component({
  selector: 'ngx-profile-phase-bar',
  templateUrl: './profile-phase-bar.component.html',
  styleUrls: ['./profile-phase-bar.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class ProfilePhaseBarComponent {
  @Input() breakdown?: PhaseBreakdown;

  readonly formatMs = formatMs;

  get view() {
    return phaseBars(this.breakdown);
  }

  trackByKey(_: number, item: { key: string }): string {
    return item.key;
  }
}
