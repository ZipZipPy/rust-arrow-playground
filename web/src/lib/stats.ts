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

function groupBy<K>(items: readonly Measurement[], key: (m: Measurement) => K): Map<K, Measurement[]> {
  const groups = new Map<K, Measurement[]>();
  for (const m of items) {
    const list = groups.get(key(m));
    if (list) list.push(m);
    else groups.set(key(m), [m]);
  }
  return groups;
}

/** Groups by format, in the order formats first appear. */
export function summarizeByFormat(items: readonly Measurement[]): GroupSummary[] {
  return [...groupBy(items, (m) => m.format)].map(([format, list]) => summarizeGroup(format, list));
}

const FORMAT_ORDER: Record<DataFormat, number> = { json: 0, arrow: 1, parquet: 2 };

/** One server in Compare servers: its formats, the fastest one, and Arrow IPC against JSON. */
export interface ServerSummary {
  target: string;
  region: string | null;
  /** Median rows per response. */
  rowCount: number;
  /** JSON, Arrow IPC, Parquet, as far as measured. */
  formats: GroupSummary[];
  /** The format with the lowest median end-to-end time, once at least two were measured. */
  fastest: DataFormat | null;
  /** JSON's median end-to-end time divided by Arrow IPC's. */
  arrowSpeedup: number | null;
  /** Arrow IPC's median wire bytes as a fraction of JSON's. */
  arrowSize: number | null;
}

/** Groups by server (target), in the order servers first appear, then by format. */
export function summarizeByServer(items: readonly Measurement[]): ServerSummary[] {
  return [...groupBy(items, (m) => m.target)].map(([target, list]) => {
    const formats = summarizeByFormat(list).sort((a, b) => FORMAT_ORDER[a.format] - FORMAT_ORDER[b.format]);
    const fastest = formats.reduce<GroupSummary | null>(
      (best, g) => (best === null || g.endToEnd.median < best.endToEnd.median ? g : best),
      null,
    );
    const json = formats.find((g) => g.format === 'json');
    const arrow = formats.find((g) => g.format === 'arrow');
    return {
      target,
      region: list[0]!.region,
      rowCount: median(list.map((m) => m.rowCount)),
      formats,
      fastest: formats.length > 1 ? (fastest?.format ?? null) : null,
      arrowSpeedup: json && arrow ? speedup(json.endToEnd.median, arrow.endToEnd.median) : null,
      arrowSize: json && arrow ? ratio(arrow.wireBytes, json.wireBytes) : null,
    };
  });
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
  // A Compare servers batch spans several targets.
  const batch = history.filter(
    (m) => m.batchId === match.batchId && m.variant === match.variant && m.target === match.target,
  );
  return { summary: summarizeGroup(match.variant, batch), differences: settingDifferences(ref, match) };
}
