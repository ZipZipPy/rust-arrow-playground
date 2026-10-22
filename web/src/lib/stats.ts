import type { DataFormat, Measurement } from './types';

export interface Summary {
  count: number;
  median: number;
  min: number;
  max: number;
  mean: number;
}

export function median(values: readonly number[]): number {
  if (values.length === 0) return Number.NaN;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  return sorted.length % 2 === 1 ? sorted[mid]! : (sorted[mid - 1]! + sorted[mid]!) / 2;
}

export function summarize(values: readonly number[]): Summary {
  if (values.length === 0) {
    return { count: 0, median: Number.NaN, min: Number.NaN, max: Number.NaN, mean: Number.NaN };
  }
  let min = Infinity;
  let max = -Infinity;
  let total = 0;
  for (const v of values) {
    if (v < min) min = v;
    if (v > max) max = v;
    total += v;
  }
  return { count: values.length, median: median(values), min, max, mean: total / values.length };
}

/** Median of the non-null values, or null when there are none. */
export function medianOf(values: readonly (number | null)[]): number | null {
  const present = values.filter((v): v is number => v !== null && Number.isFinite(v));
  return present.length === 0 ? null : median(present);
}

/** How many times faster `value` is than `baseline` (both durations). */
export function speedup(baseline: number, value: number): number | null {
  return baseline > 0 && value > 0 ? baseline / value : null;
}

/** `value` as a fraction of `baseline` (e.g. 0.25 = a quarter of the size). */
export function ratio(value: number | null, baseline: number | null): number | null {
  return value !== null && baseline !== null && baseline > 0 ? value / baseline : null;
}

export const PHASES = ['db', 'encode', 'network', 'download', 'decode', 'use', 'other'] as const;
export type Phase = (typeof PHASES)[number];
export type PhaseTimes = Record<Phase, number>;

export function phaseTimes(m: Measurement): PhaseTimes {
  return {
    db: m.dbMs ?? 0,
    encode: m.encodeMs ?? 0,
    network: m.networkWaitMs,
    download: m.downloadMs,
    decode: m.decodeMs,
    use: m.useMs,
    other: m.otherMs,
  };
}

/** One summarized group of measurements (a format in Compare, a Run batch). */
export interface GroupSummary {
  key: string;
  format: DataFormat;
  variant: string;
  count: number;
  endToEnd: Summary;
  phases: PhaseTimes;
  decodeMs: number;
  ttfbMs: number;
  downloadMs: number;
  wireBytes: number | null;
  payloadBytes: number;
  rowCount: number;
}

export function summarizeGroup(key: string, items: readonly Measurement[]): GroupSummary {
  const first = items[0];
  if (!first) throw new Error('summarizeGroup needs at least one measurement');
  const phases = Object.fromEntries(
    PHASES.map((p) => [p, median(items.map((m) => phaseTimes(m)[p]))]),
  ) as PhaseTimes;
  return {
    key,
    format: first.format,
    variant: first.variant,
    count: items.length,
    endToEnd: summarize(items.map((m) => m.endToEndMs)),
    phases,
    decodeMs: median(items.map((m) => m.decodeMs)),
    ttfbMs: median(items.map((m) => m.ttfbMs)),
    downloadMs: median(items.map((m) => m.downloadMs)),
    wireBytes: medianOf(items.map((m) => m.wireBytes)),
    payloadBytes: median(items.map((m) => m.payloadBytes)),
    rowCount: median(items.map((m) => m.rowCount)),
  };
}

/** Groups by format, in the order formats first appear. */
export function summarizeByFormat(items: readonly Measurement[]): GroupSummary[] {
  const groups = new Map<DataFormat, Measurement[]>();
  for (const m of items) {
    const list = groups.get(m.format);
    if (list) list.push(m);
    else groups.set(m.format, [m]);
  }
  return [...groups].map(([format, list]) => summarizeGroup(format, list));
}

/** The fields that make two measurements comparable: same server, rows and columns. */
type Shape = Pick<Measurement, 'format' | 'rowsRequested' | 'columnsKey' | 'target'>;

/**
 * For a JSON result, the latest Arrow/Parquet batch of the same shape; for an
 * Arrow/Parquet result, the latest JSON batch. `history` is newest first.
 */
export function counterpartSummary(history: readonly Measurement[], ref: Shape): GroupSummary | null {
  const wantJson = ref.format !== 'json';
  const match = history.find(
    (m) =>
      (m.format === 'json') === wantJson &&
      m.rowsRequested === ref.rowsRequested &&
      m.columnsKey === ref.columnsKey &&
      m.target === ref.target,
  );
  if (!match) return null;
  const batch = history.filter((m) => m.batchId === match.batchId && m.variant === match.variant);
  return summarizeGroup(match.variant, batch);
}
