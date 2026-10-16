import { NO_ERRORS_SCHEMA } from '@angular/core';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { TranslateModule } from '@ngx-translate/core';
import { DiagnosticResult, ProfileAnalysisResult } from '../../../../../@core/data/node.service';
import { FindingFocus, ProfileDiagnosticsListComponent } from './profile-diagnostics-list.component';

// Smoke test with a mocked Doris analysis response (shape of /clusters/profiles/{id}/analyze).
const MOCK_ANALYSIS: Partial<ProfileAnalysisResult> = {
  engine: 'doris',
  diagnostics: [
    {
    rule_id: 'DORIS_TIME_SHARE', rule_name: 'Dominant operator', severity: 'Info', node_path: 'doris_f3_p1_JDBC_SCAN_OPERATOR_0_none',
    message: 'JDBC_SCAN: 59.4% of measured operator execution time', reason: 'ExecTime avg = 20.278ms', suggestions: [],
    impact: 0.59, confidence: 'high', evidence_counters: ['ExecTime'],
    },
    {
    rule_id: 'DORIS_OPERATOR_MEMORY', rule_name: 'High operator memory', severity: 'Critical', node_path: 'doris_f2_p1_HASH_JOIN_SINK_OPERATOR_3_none',
    message: 'HASH_JOIN_SINK peaked at 1.80 GB', reason: 'MemoryUsagePeak max=1.80 GB', suggestions: ['Filter the build side earlier.'],
    impact: 0.84, confidence: 'high', evidence_counters: ['MemoryUsagePeak'],
    parameter_suggestions: [{
      name: 'exec_mem_limit', param_type: 'Session', current: '2147483648', recommended: '3221225472',
      command: 'SET exec_mem_limit = 3221225472;', description: '', impact: '', scope: 'session', risk: 'Raises memory.',
    }],
    },
    {
    rule_id: 'DORIS_PLAN_TIME', rule_name: 'Planning takes a large share', severity: 'Warning', node_path: 'query',
    message: 'Plan Time is 40% of Total', reason: 'Plan Time = 4.000s', suggestions: [], impact: 0.4,
    sql_snippets: [{ label: 'Enable profiles for all new sessions', sql: 'SET GLOBAL enable_profile = true;', scope: 'global', warning: 'Affects every new connection.' }],
    },
  ] as DiagnosticResult[],
};

describe('ProfileDiagnosticsListComponent', () => {
  let fixture: ComponentFixture<ProfileDiagnosticsListComponent>;
  let element: HTMLElement;

  beforeEach(async () => {
    await TestBed.configureTestingModule({
      imports: [TranslateModule.forRoot()],
      declarations: [ProfileDiagnosticsListComponent],
      schemas: [NO_ERRORS_SCHEMA],
    }).compileComponents();
    fixture = TestBed.createComponent(ProfileDiagnosticsListComponent);
    fixture.componentInstance.diagnostics = MOCK_ANALYSIS.diagnostics;
    fixture.detectChanges();
    element = fixture.nativeElement;
  });

  it('orders findings by severity then impact and labels severity in text', () => {
    const items = Array.from(element.querySelectorAll('li.finding'));
    expect(items.map(item => item.getAttribute('data-severity'))).toEqual(['Critical', 'Warning', 'Info']);
    // Color is not the only signal: each header carries a severity label.
    expect(items[0].querySelector('.severity')?.textContent).toContain('profile.severity.Critical');
  });

  it('focuses the operator and passes its evidence counters', () => {
    const focused: FindingFocus[] = [];
    fixture.componentInstance.focusFinding.subscribe((focus: FindingFocus) => focused.push(focus));
    (element.querySelector('.finding-header') as HTMLButtonElement).click();
    fixture.detectChanges();
    (element.querySelector('.details .link') as HTMLButtonElement).click();
    expect(focused).toEqual([{ nodeId: 'doris_f2_p1_HASH_JOIN_SINK_OPERATOR_3_none', counters: ['MemoryUsagePeak'] }]);
  });

  it('shows query-level findings without a focus link and marks global snippets', () => {
    const headers = element.querySelectorAll<HTMLButtonElement>('.finding-header');
    headers[1].click();
    fixture.detectChanges();
    const details = element.querySelectorAll('.details')[0];
    expect(details.querySelector('.link')).toBeNull();
    expect(details.querySelector('[data-scope="global"]')).not.toBeNull();
    expect(details.textContent).toContain('Affects every new connection.');
  });

  it('moves focus between findings with arrow keys', () => {
    const headers = element.querySelectorAll<HTMLButtonElement>('.finding-header');
    headers[0].focus();
    headers[0].dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    expect(document.activeElement).toBe(headers[1]);
    headers[1].dispatchEvent(new KeyboardEvent('keydown', { key: 'End', bubbles: true }));
    expect(document.activeElement).toBe(headers[2]);
  });
});
