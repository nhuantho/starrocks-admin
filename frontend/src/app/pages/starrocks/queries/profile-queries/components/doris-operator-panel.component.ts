import { ChangeDetectionStrategy, Component, Input } from '@angular/core';
import { DiagnosticResult, ProfileAnalysisNode } from '../../../../../@core/data/node.service';
import { CounterRow, counterRows, severitySymbol, sortDiagnostics } from '../profile-diagnostics.utils';

// Doris operator counters with min/avg/max per counter. Skew is max/avg of the same counter
// across tasks; waits are shown as their own counters and never summed into ExecTime.
@Component({
  selector: 'ngx-doris-operator-panel',
  templateUrl: './doris-operator-panel.component.html',
  styleUrls: ['./doris-operator-panel.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class DorisOperatorPanelComponent {
  @Input() node?: ProfileAnalysisNode;
  @Input() set diagnostics(value: DiagnosticResult[] | undefined) {
    this.findings = sortDiagnostics(value);
  }
  // Counter names to highlight (evidence of the selected finding)
  @Input() highlight: string[] = [];

  findings: DiagnosticResult[] = [];
  filter = '';
  readonly severitySymbol = severitySymbol;
  // max/avg at or above this is marked as skewed (same threshold as DORIS_EXEC_SKEW)
  readonly skewThreshold = 2;

  get rows(): CounterRow[] {
    const needle = this.filter.trim().toLowerCase();
    const evidence = this.findings.reduce<string[]>(
      (all, diagnostic) => all.concat(diagnostic.evidence_counters || []),
      [...this.highlight],
    );
    return counterRows(this.node?.unique_metrics, evidence)
      .filter(row => !needle || row.key.toLowerCase().includes(needle));
  }

  get instances(): string | undefined {
    return this.node?.unique_metrics?.InstanceCount;
  }

  trackByKey(_: number, row: CounterRow): string {
    return row.key;
  }
}
