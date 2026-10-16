// Pure helpers for the profile diagnostics UI. No Angular imports so they can be unit-tested in Node.

export interface RankedDiagnostic {
  severity: string;
  impact?: number;
}

export interface PhaseInput {
  total_ms?: number;
  additive: boolean;
  unaccounted_ms?: number;
  segments: Array<{ key: string; label: string; time_ms: number; raw: string }>;
}

export interface PhaseBar {
  key: string;
  label: string;
  timeMs: number;
  raw: string;
  // Share of the bar width (0-100)
  percent: number;
}

export interface CounterRow {
  key: string;
  raw: string;
  sum?: string;
  avg?: string;
  max?: string;
  min?: string;
  // max / avg when both are parseable and avg > 0
  skew?: number;
  highlighted: boolean;
}

const SEVERITY_RANK: { [severity: string]: number } = { Critical: 4, Error: 3, Warning: 2, Info: 1 };

export function severityRank(severity: string | undefined): number {
  return SEVERITY_RANK[severity || ''] || 0;
}

// Text symbol shown next to the severity label so color is never the only signal.
export function severitySymbol(severity: string | undefined): string {
  switch (severity) {
    case 'Critical':
    case 'Error':
      return '⛔';
    case 'Warning':
      return '⚠';
    default:
      return 'ℹ';
  }
}

// Severity first, then impact; stable for equal keys.
export function sortDiagnostics<T extends RankedDiagnostic>(diagnostics: T[] | undefined): T[] {
  return (diagnostics || [])
    .map((diagnostic, index) => ({ diagnostic, index }))
    .sort((a, b) =>
      severityRank(b.diagnostic.severity) - severityRank(a.diagnostic.severity)
      || (b.diagnostic.impact ?? -1) - (a.diagnostic.impact ?? -1)
      || a.index - b.index)
    .map(item => item.diagnostic);
}

// Stacked bar widths for additive phases; independent bars (scaled to the largest) otherwise.
export function phaseBars(breakdown: PhaseInput | undefined): { bars: PhaseBar[]; unaccountedPercent: number; stacked: boolean } {
  if (!breakdown?.segments?.length) {
    return { bars: [], unaccountedPercent: 0, stacked: false };
  }
  const total = breakdown.total_ms;
  const stacked = breakdown.additive && !!total && total > 0;
  const scale = stacked ? total : Math.max(...breakdown.segments.map(s => s.time_ms), 0);
  const bars = breakdown.segments.map(segment => ({
    key: segment.key,
    label: segment.label,
    timeMs: segment.time_ms,
    raw: segment.raw,
    percent: scale > 0 ? Math.min(100, (segment.time_ms / scale) * 100) : 0,
  }));
  const unaccountedPercent = stacked && breakdown.unaccounted_ms ? Math.min(100, (breakdown.unaccounted_ms / total) * 100) : 0;
  return { bars, unaccountedPercent, stacked };
}

const DURATION_PART = /(\d+(?:\.\d+)?)(hour|min|sec|ms|us|μs|ns|h|m|s)/g;
const DURATION = /^(?:\d+(?:\.\d+)?(?:hour|min|sec|ms|us|μs|ns|h|m|s))+$/;
const BYTES = /^(\d+(?:\.\d+)?)\s*(B|KB|MB|GB|TB)$/;
const RAW_COUNT = /\((\d[\d,]*)\)$/;
const COUNT = /^([\d,]+(?:\.\d+)?)\s*([KMGT])?$/;

// Parse a profile quantity into a comparable number (ns, bytes or count). Unknown -> undefined.
export function parseQuantity(value: string | undefined): number | undefined {
  const text = (value || '').trim();
  if (!text || text === 'N/A' || text.startsWith('-')) {
    return undefined;
  }
  if (DURATION.test(text)) {
    const factor: { [unit: string]: number } = { hour: 3.6e12, h: 3.6e12, min: 6e10, m: 6e10, sec: 1e9, s: 1e9, ms: 1e6, us: 1e3, 'μs': 1e3, ns: 1 };
    let total = 0;
    let match: RegExpExecArray | null;
    DURATION_PART.lastIndex = 0;
    while ((match = DURATION_PART.exec(text)) !== null) {
      total += parseFloat(match[1]) * factor[match[2]];
    }
    return total;
  }
  const bytes = BYTES.exec(text);
  if (bytes) {
    const power = ['B', 'KB', 'MB', 'GB', 'TB'].indexOf(bytes[2]);
    return parseFloat(bytes[1]) * Math.pow(1024, power);
  }
  const raw = RAW_COUNT.exec(text);
  if (raw) {
    return parseFloat(raw[1].replace(/,/g, ''));
  }
  const count = COUNT.exec(text);
  if (count) {
    const scale: { [suffix: string]: number } = { K: 1e3, M: 1e6, G: 1e9, T: 1e12 };
    return parseFloat(count[1].replace(/,/g, '')) * (count[2] ? scale[count[2]] : 1);
  }
  return undefined;
}

// Split "sum x, avg y, max z, min w" without breaking on thousands separators.
export function splitStatistics(value: string): { sum?: string; avg?: string; max?: string; min?: string } {
  const result: { [stat: string]: string } = {};
  const text = (value || '').trim();
  if (!/^(sum|avg|max|min) /.test(text)) {
    return {};
  }
  const pattern = /(?:^|, )(sum|avg|max|min) ?(.*?)(?=, (?:sum|avg|max|min)(?: |$)|$)/g;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(text)) !== null) {
    const stat = match[2].trim();
    if (stat) {
      result[match[1]] = stat;
    }
  }
  return result;
}

const INTERNAL_KEYS = new Set(['TimingBasis', 'InstanceCount', 'OperatorTotalTime', 'DetailTaskSamples']);

// Counter table for an operator: evidence counters first, then alphabetical.
export function counterRows(metrics: { [key: string]: string } | undefined, highlight: string[] = []): CounterRow[] {
  const wanted = new Set(highlight);
  return Object.entries(metrics || {})
    .filter(([key]) => !INTERNAL_KEYS.has(key))
    .map(([key, raw]) => {
      const stats = splitStatistics(String(raw));
      const avg = parseQuantity(stats.avg);
      const max = parseQuantity(stats.max);
      const row: CounterRow = { key, raw: String(raw), ...stats, highlighted: wanted.has(key) };
      if (avg !== undefined && max !== undefined && avg > 0) {
        row.skew = max / avg;
      }
      return row;
    })
    .sort((a, b) => Number(b.highlighted) - Number(a.highlighted) || a.key.localeCompare(b.key));
}

export function formatMs(ms: number | undefined | null): string {
  if (ms === undefined || ms === null || !isFinite(ms)) {
    return 'N/A';
  }
  if (Math.abs(ms) >= 1000) {
    return `${(ms / 1000).toFixed(2)}s`;
  }
  return `${ms.toFixed(ms < 10 && ms > -10 ? 1 : 0)}ms`;
}

export function formatDelta(value: number | undefined | null, unit: 'ms' | 'ns'): string {
  if (value === undefined || value === null) {
    return 'N/A';
  }
  const ms = unit === 'ns' ? value / 1e6 : value;
  return `${ms > 0 ? '+' : ''}${formatMs(ms)}`;
}

// Query-level findings have no operator to focus.
export function isOperatorFinding(nodePath: string | undefined): boolean {
  return !!nodePath && nodePath !== 'query';
}
