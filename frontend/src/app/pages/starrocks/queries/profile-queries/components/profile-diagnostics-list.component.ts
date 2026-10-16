import { ChangeDetectionStrategy, Component, ElementRef, EventEmitter, Input, Output, QueryList, ViewChildren } from '@angular/core';
import { DiagnosticResult } from '../../../../../@core/data/node.service';
import { isOperatorFinding, severitySymbol, sortDiagnostics } from '../profile-diagnostics.utils';

export interface FindingFocus {
  nodeId: string;
  counters: string[];
}

// Findings ranked by severity then impact. Arrow keys move between findings; selecting an
// operator finding asks the parent to focus that node and highlight the evidence counters.
@Component({
  selector: 'ngx-profile-diagnostics-list',
  templateUrl: './profile-diagnostics-list.component.html',
  styleUrls: ['./profile-diagnostics-list.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class ProfileDiagnosticsListComponent {
  @Input() set diagnostics(value: DiagnosticResult[] | undefined) {
    this.sorted = sortDiagnostics(value);
  }
  // Resolves a node id to a readable operator label.
  @Input() nodeLabel: (nodeId: string) => string = id => id;
  @Output() focusFinding = new EventEmitter<FindingFocus>();
  @Output() copyText = new EventEmitter<string>();

  @ViewChildren('findingButton') buttons!: QueryList<ElementRef<HTMLButtonElement>>;

  sorted: DiagnosticResult[] = [];
  expanded = new Set<number>();

  readonly severitySymbol = severitySymbol;
  readonly isOperatorFinding = isOperatorFinding;

  toggle(index: number): void {
    if (this.expanded.has(index)) {
      this.expanded.delete(index);
    } else {
      this.expanded.add(index);
    }
  }

  focusNode(diagnostic: DiagnosticResult): void {
    if (isOperatorFinding(diagnostic.node_path)) {
      this.focusFinding.emit({ nodeId: diagnostic.node_path, counters: diagnostic.evidence_counters || [] });
    }
  }

  onKeydown(event: KeyboardEvent, index: number): void {
    const last = this.sorted.length - 1;
    const target = { ArrowDown: Math.min(index + 1, last), ArrowUp: Math.max(index - 1, 0), Home: 0, End: last }[event.key];
    if (target === undefined) {
      return;
    }
    event.preventDefault();
    this.buttons.get(target)?.nativeElement.focus();
  }

  impactPercent(diagnostic: DiagnosticResult): string | null {
    return diagnostic.impact === undefined || diagnostic.impact === null ? null : `${Math.round(diagnostic.impact * 100)}%`;
  }

  trackByIndex(index: number): number {
    return index;
  }
}
