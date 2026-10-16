import {
  counterRows, formatDelta, formatMs, isOperatorFinding, parseQuantity, phaseBars, severityRank,
  severitySymbol, sortDiagnostics, splitStatistics,
} from './profile-diagnostics.utils';

describe('profile diagnostics utils', () => {
  it('ranks severity then impact, keeping input order for ties', () => {
    const sorted = sortDiagnostics([
      { id: 'a', severity: 'Info', impact: 0.9 },
      { id: 'b', severity: 'Warning', impact: 0.1 },
      { id: 'c', severity: 'Critical' },
      { id: 'd', severity: 'Warning', impact: 0.6 },
      { id: 'e', severity: 'Warning', impact: 0.6 },
    ]);
    expect(sorted.map(d => d.id)).toEqual(['c', 'd', 'e', 'b', 'a']);
    expect(sortDiagnostics(undefined)).toEqual([]);
    expect(severityRank('Unknown')).toBe(0);
    expect(severitySymbol('Critical')).not.toEqual(severitySymbol('Info'));
  });

  it('stacks additive phases against the total and scales overlapping ones to the largest', () => {
    const stacked = phaseBars({
      total_ms: 49, additive: true, unaccounted_ms: 0,
      segments: [
        { key: 'parse', label: 'Parse', time_ms: 1, raw: '1ms' },
        { key: 'plan', label: 'Plan', time_ms: 8, raw: '8ms' },
        { key: 'schedule', label: 'Schedule', time_ms: 9, raw: '9ms' },
        { key: 'execute_fetch', label: 'Execute', time_ms: 31, raw: '31ms' },
      ],
    });
    expect(stacked.stacked).toBe(true);
    expect(stacked.bars.reduce((sum, bar) => sum + bar.percent, 0)).toBeCloseTo(100, 5);
    const overlapping = phaseBars({
      total_ms: 10, additive: false,
      segments: [{ key: 'plan', label: 'Plan', time_ms: 8, raw: '8s' }, { key: 'execute_fetch', label: 'E', time_ms: 4, raw: '4s' }],
    });
    expect(overlapping.stacked).toBe(false);
    expect(overlapping.bars.map(b => b.percent)).toEqual([100, 50]);
    expect(phaseBars(undefined).bars).toEqual([]);
  });

  it('parses every observed unit and keeps unknown values unknown', () => {
    expect(parseQuantity('1min2sec3ms')).toBe(62_003_000_000);
    expect(parseQuantity('32.995ms')).toBeCloseTo(32_995_000, 0);
    expect(parseQuantity('4.13 KB')).toBeCloseTo(4.13 * 1024, 5);
    expect(parseQuantity('150.0M (150000000)')).toBe(150_000_000);
    expect(parseQuantity('1,234')).toBe(1234);
    expect(parseQuantity('1.5K')).toBe(1500);
    expect(parseQuantity('N/A')).toBeUndefined();
    expect(parseQuantity('-1')).toBeUndefined();
    expect(parseQuantity('')).toBeUndefined();
  });

  it('splits merged statistics without breaking thousands separators', () => {
    expect(splitStatistics('sum 1,234, avg 617, max 1,000, min 234')).toEqual({ sum: '1,234', avg: '617', max: '1,000', min: '234' });
    expect(splitStatistics('avg 21.883us, max 39.648us, min 16.641us')).toEqual({ avg: '21.883us', max: '39.648us', min: '16.641us' });
    expect(splitStatistics('sum , avg , max , min')).toEqual({});
    expect(splitStatistics('32.995ms')).toEqual({});
  });

  it('builds counter rows with skew and evidence first, hiding internal keys', () => {
    const rows = counterRows({
      ExecTime: 'avg 10ms, max 30ms, min 1ms',
      RowsProduced: 'sum 9, avg 1, max 2, min 1',
      TimingBasis: 'internal',
      ProbeRows: 'sum 1, avg 0, max 1, min 0',
    }, ['RowsProduced']);
    expect(rows.map(r => r.key)).toEqual(['RowsProduced', 'ExecTime', 'ProbeRows']);
    expect(rows[0].highlighted).toBe(true);
    expect(rows[1].skew).toBeCloseTo(3, 5);
    // avg 0 -> no skew claim
    expect(rows[2].skew).toBeUndefined();
  });

  it('formats milliseconds and deltas', () => {
    expect(formatMs(undefined)).toBe('N/A');
    expect(formatMs(1500)).toBe('1.50s');
    expect(formatMs(3.25)).toBe('3.3ms');
    expect(formatDelta(100_000_000, 'ns')).toBe('+100ms');
    expect(formatDelta(-2, 'ms')).toBe('-2.0ms');
    expect(isOperatorFinding('query')).toBe(false);
    expect(isOperatorFinding('doris_f0_p0_SCAN')).toBe(true);
  });
});
