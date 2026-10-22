import { USE_SCOPE_LABELS } from './settings';
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

export const PHASES = ['db', 'encode', 'compress', 'network', 'download', 'decode', 'use', 'other'] as const;
export type Phase = (typeof PHASES)[number];
export type PhaseTimes = Record<Phase, number>;

export function phaseTimes(m: Measurement): PhaseTimes {
  return {
    db: m.dbMs ?? 0,
    encode: m.encodeMs ?? 0,
    compress: m.compressMs ?? 0,
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
  useMs: number;
  /** Decode + use: all the browser's work to turn the body into values. */
  browserMs: number;
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
    useMs: median(items.map((m) => m.useMs)),
    browserMs: median(items.map((m) => m.decodeMs + m.useMs)),
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

interface PairedSetting {
  label: string;
  /** Compared as is. */
  value: (m: Measurement) => string;
  /** How a value is shown, when that differs from the value. */
  show?: (value: string) => string;
}

function describeColumns(key: string): string {
  if (key === 'all') return 'all';
  const names = key.split(',');
  return names.length <= 3 ? names.join(', ') : `${names.length} picked`;
}

/** The settings two runs must share before the Arrow vs JSON callout compares them. */
const PAIRED_SETTINGS: readonly PairedSetting[] = [
  { label: 'dataset', value: (m) => m.dataset },
  { label: 'rows', value: (m) => m.rowsRequested.toLocaleString('en-US') },
  { label: 'columns', value: (m) => m.columnsKey, show: describeColumns },
  { label: 'HTTP compression', value: (m) => (m.httpCompression ? 'on' : 'off') },
  { label: 'row cache', value: (m) => (m.rowCache ? 'on' : 'off') },
  { label: 'start id', value: (m) => m.offsetMode },
  { label: 'touch data', value: (m) => USE_SCOPE_LABELS[m.useScope].toLowerCase() },
];

/** One setting that differs between two runs, as shown to the user. */
export interface SettingDifference {
  label: string;
  /** This run's value. */
  current: string;
  /** The other run's value. */
  other: string;
}

export function settingDifferences(current: Measurement, other: Measurement): SettingDifference[] {
  const differences: SettingDifference[] = [];
  for (const { label, value, show = (v: string) => v } of PAIRED_SETTINGS) {
    const [a, b] = [value(current), value(other)];
    if (a !== b) differences.push({ label, current: show(a), other: show(b) });
  }
  return differences;
}

export interface Counterpart {
  summary: GroupSummary;
  /** Settings that differ from the reference run; empty when the two are comparable. */
  differences: SettingDifference[];
}

/**
 * For a JSON run, the latest Arrow/Parquet batch against the same server; for
 * an Arrow/Parquet run, the latest JSON batch. Prefers the latest one made
 * with the same paired settings; otherwise returns the latest one with what
 * differs, so the caller can say why it does not compare them. `history` is
 * newest first.
 */
export function counterpartSummary(history: readonly Measurement[], ref: Measurement): Counterpart | null {
  const wantJson = ref.format !== 'json';
  const candidates = history.filter((m) => (m.format === 'json') === wantJson && m.target === ref.target);
  const match = candidates.find((m) => settingDifferences(ref, m).length === 0) ?? candidates[0];
  if (!match) return null;
  const batch = history.filter((m) => m.batchId === match.batchId && m.variant === match.variant);
  return { summary: summarizeGroup(match.variant, batch), differences: settingDifferences(ref, match) };
}
