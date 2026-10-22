import {
  type ApiInfo,
  type ColumnInfo,
  type DataFormat,
  type DataRequest,
  IPC_COMPRESSIONS,
  type IpcCompression,
  PARQUET_COMPRESSIONS,
  type ParquetCompression,
} from './types';

export type ArrowFlavor = 'arrow' | 'parquet';

export interface Settings {
  /** The headline switch: off sends JSON, on sends `arrowFlavor`. */
  useArrow: boolean;
  arrowFlavor: ArrowFlavor;
  rows: number;
  /** `null` = all columns. */
  columns: string[] | null;
  offset: number;
  randomizeOffset: boolean;
  httpCompression: boolean;
  cache: boolean;
  dictionary: boolean;
  ipcCompression: IpcCompression;
  arrowBatchSize: number;
  parquetCompression: ParquetCompression;
  rowGroupSize: number;
  iterations: number;
  warmup: boolean;
}

export const ROW_PRESETS = [1_000, 10_000, 100_000, 250_000, 500_000, 1_000_000] as const;
export const BATCH_SIZES = [1_024, 4_096, 16_384, 65_536, 262_144, 1_048_576] as const;
export const MIN_BATCH_SIZE = 1_024;
export const MAX_BATCH_SIZE = 1_048_576;
export const MAX_ITERATIONS = 20;
export const COLUMN_GROUPS = ['numeric', 'text', 'time'] as const;

export const DEFAULT_SETTINGS: Settings = {
  useArrow: true,
  arrowFlavor: 'arrow',
  rows: 100_000,
  columns: null,
  offset: 0,
  randomizeOffset: false,
  httpCompression: true,
  cache: false,
  dictionary: false,
  ipcCompression: 'none',
  arrowBatchSize: 65_536,
  parquetCompression: 'snappy',
  rowGroupSize: 65_536,
  iterations: 3,
  warmup: true,
};

export const FORMAT_LABELS: Record<DataFormat, string> = {
  json: 'JSON',
  arrow: 'Arrow IPC',
  parquet: 'Parquet',
};

const clampInt = (value: number, min: number, max: number) => Math.min(max, Math.max(min, Math.round(value)));

function pick<T>(value: unknown, allowed: readonly T[], fallback: T): T {
  return allowed.includes(value as T) ? (value as T) : fallback;
}

function bool(value: unknown, fallback: boolean): boolean {
  return typeof value === 'boolean' ? value : fallback;
}

function int(value: unknown, fallback: number, min: number, max: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? clampInt(value, min, max) : fallback;
}

/** Accepts anything (e.g. stale localStorage) and returns valid settings. */
export function sanitizeSettings(raw: unknown): Settings {
  const r = (raw && typeof raw === 'object' ? raw : {}) as Partial<Record<keyof Settings, unknown>>;
  const d = DEFAULT_SETTINGS;
  const columns =
    Array.isArray(r.columns) && r.columns.every((c) => typeof c === 'string') ? (r.columns as string[]) : null;
  return {
    useArrow: bool(r.useArrow, d.useArrow),
    arrowFlavor: pick(r.arrowFlavor, ['arrow', 'parquet'] as const, d.arrowFlavor),
    rows: int(r.rows, d.rows, 1, Number.MAX_SAFE_INTEGER),
    columns,
    offset: int(r.offset, d.offset, 0, Number.MAX_SAFE_INTEGER),
    randomizeOffset: bool(r.randomizeOffset, d.randomizeOffset),
    httpCompression: bool(r.httpCompression, d.httpCompression),
    cache: bool(r.cache, d.cache),
    dictionary: bool(r.dictionary, d.dictionary),
    ipcCompression: pick(r.ipcCompression, IPC_COMPRESSIONS, d.ipcCompression),
    arrowBatchSize: int(r.arrowBatchSize, d.arrowBatchSize, MIN_BATCH_SIZE, MAX_BATCH_SIZE),
    parquetCompression: pick(r.parquetCompression, PARQUET_COMPRESSIONS, d.parquetCompression),
    rowGroupSize: int(r.rowGroupSize, d.rowGroupSize, MIN_BATCH_SIZE, MAX_BATCH_SIZE),
    iterations: int(r.iterations, d.iterations, 1, MAX_ITERATIONS),
    warmup: bool(r.warmup, d.warmup),
  };
}

/** Makes settings consistent with what the server offers (row cap, known columns). */
export function fitToServer(s: Settings, info: ApiInfo): Settings {
  const known = new Set(info.columns.map((c) => c.name));
  let columns = s.columns?.filter((c) => known.has(c)) ?? null;
  if (columns && columns.length === info.columns.length) columns = null;
  return { ...s, rows: Math.min(s.rows, Math.max(1, info.max_rows)), columns };
}

export function activeFormat(s: Settings): DataFormat {
  return s.useArrow ? s.arrowFlavor : 'json';
}

/** Selected columns in catalog order. */
export function selectedColumns(s: Settings, catalog: readonly ColumnInfo[]): ColumnInfo[] {
  if (s.columns === null) return [...catalog];
  const chosen = new Set(s.columns);
  return catalog.filter((c) => chosen.has(c.name));
}

/** The `columns` parameter: `null` when every column is selected. */
export function columnsParam(s: Settings, catalog: readonly ColumnInfo[]): string[] | null {
  const chosen = selectedColumns(s, catalog);
  return chosen.length === catalog.length ? null : chosen.map((c) => c.name);
}

export function columnsForGroup(catalog: readonly ColumnInfo[], group: string | null): string[] | null {
  return group === null ? null : catalog.filter((c) => c.group === group).map((c) => c.name);
}

export function buildRequest(s: Settings, format: DataFormat, offset: number, columns: string[] | null): DataRequest {
  return {
    format,
    rows: s.rows,
    offset,
    columns,
    httpCompression: s.httpCompression,
    cache: s.cache,
    dictionary: s.dictionary,
    ipcCompression: s.ipcCompression,
    parquetCompression: s.parquetCompression,
    batchSize: format === 'parquet' ? s.rowGroupSize : s.arrowBatchSize,
  };
}

/** Human label for a request's format and format options, e.g. "Arrow IPC · zstd · dict". */
export function variantLabel(req: Pick<DataRequest, 'format' | 'ipcCompression' | 'dictionary' | 'parquetCompression'>): string {
  switch (req.format) {
    case 'json':
      return FORMAT_LABELS.json;
    case 'arrow': {
      const parts = [FORMAT_LABELS.arrow];
      if (req.ipcCompression !== 'none') parts.push(req.ipcCompression);
      if (req.dictionary) parts.push('dict');
      return parts.join(' · ');
    }
    case 'parquet':
      return `${FORMAT_LABELS.parquet} · ${req.parquetCompression}`;
  }
}

export function columnsKey(columns: string[] | null): string {
  return columns === null ? 'all' : columns.join(',');
}

/** A random start id that still leaves `rows` rows after it. */
export function randomOffset(maxId: number, rows: number, random: () => number = Math.random): number {
  const span = Math.max(0, maxId - rows);
  return Math.floor(random() * (span + 1));
}
