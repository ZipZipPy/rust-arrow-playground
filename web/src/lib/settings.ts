import {
  type ApiInfo,
  type ColumnInfo,
  type DataFormat,
  type DataRequest,
  type DatasetInfo,
  type DatasetKind,
  IPC_COMPRESSIONS,
  type IpcCompression,
  type OffsetMode,
  PARQUET_COMPRESSIONS,
  type ParquetCompression,
  USE_SCOPES,
  type UseScope,
} from './types';

export type ArrowFlavor = 'arrow' | 'parquet';

/** The part of the settings that belongs to one dataset. */
export interface DatasetChoice {
  /** `null` = all columns. */
  columns: string[] | null;
  offset: number;
}

export interface Settings {
  /** The selected dataset; '' until one is picked, which means the server's `default_dataset`. */
  dataset: string;
  /** The headline switch: off sends JSON, on sends `arrowFlavor`. */
  useArrow: boolean;
  arrowFlavor: ArrowFlavor;
  rows: number;
  /** Columns of the selected dataset; `null` = all columns. */
  columns: string[] | null;
  /** Start id: rows whose key column is greater than this. */
  offset: number;
  /** Columns and start id of the other datasets, restored when one is picked again. */
  otherDatasets: Record<string, DatasetChoice>;
  randomizeOffset: boolean;
  httpCompression: boolean;
  cache: boolean;
  dictionary: boolean;
  ipcCompression: IpcCompression;
  arrowBatchSize: number;
  parquetCompression: ParquetCompression;
  rowGroupSize: number;
  /** What "use" reads after decoding: every selected value, or one numeric column. */
  useScope: UseScope;
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
  dataset: '',
  useArrow: true,
  arrowFlavor: 'arrow',
  rows: 100_000,
  columns: null,
  offset: 0,
  otherDatasets: {},
  randomizeOffset: false,
  httpCompression: true,
  cache: false,
  dictionary: false,
  ipcCompression: 'none',
  arrowBatchSize: 65_536,
  parquetCompression: 'snappy',
  rowGroupSize: 65_536,
  useScope: 'all',
  iterations: 3,
  warmup: true,
};

export const FORMAT_LABELS: Record<DataFormat, string> = {
  json: 'JSON',
  arrow: 'Arrow IPC',
  parquet: 'Parquet',
};

export const KIND_LABELS: Record<DatasetKind, string> = {
  table: 'table',
  view: 'view',
  materialized_view: 'materialized view',
};

export const USE_SCOPE_LABELS: Record<UseScope, string> = {
  all: 'Every value',
  column: 'One column',
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

function columnList(value: unknown): string[] | null {
  return Array.isArray(value) && value.every((c) => typeof c === 'string') ? (value as string[]) : null;
}

function otherDatasets(value: unknown): Record<string, DatasetChoice> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const out: Record<string, DatasetChoice> = {};
  for (const [name, choice] of Object.entries(value)) {
    if (!choice || typeof choice !== 'object') continue;
    const c = choice as Partial<Record<keyof DatasetChoice, unknown>>;
    out[name] = { columns: columnList(c.columns), offset: int(c.offset, 0, 0, Number.MAX_SAFE_INTEGER) };
  }
  return out;
}

/** Accepts anything (e.g. stale localStorage) and returns valid settings. */
export function sanitizeSettings(raw: unknown): Settings {
  const r = (raw && typeof raw === 'object' ? raw : {}) as Partial<Record<keyof Settings, unknown>>;
  const d = DEFAULT_SETTINGS;
  return {
    dataset: typeof r.dataset === 'string' ? r.dataset : d.dataset,
    useArrow: bool(r.useArrow, d.useArrow),
    arrowFlavor: pick(r.arrowFlavor, ['arrow', 'parquet'] as const, d.arrowFlavor),
    rows: int(r.rows, d.rows, 1, Number.MAX_SAFE_INTEGER),
    columns: columnList(r.columns),
    offset: int(r.offset, d.offset, 0, Number.MAX_SAFE_INTEGER),
    otherDatasets: otherDatasets(r.otherDatasets),
    randomizeOffset: bool(r.randomizeOffset, d.randomizeOffset),
    httpCompression: bool(r.httpCompression, d.httpCompression),
    cache: bool(r.cache, d.cache),
    dictionary: bool(r.dictionary, d.dictionary),
    ipcCompression: pick(r.ipcCompression, IPC_COMPRESSIONS, d.ipcCompression),
    arrowBatchSize: int(r.arrowBatchSize, d.arrowBatchSize, MIN_BATCH_SIZE, MAX_BATCH_SIZE),
    parquetCompression: pick(r.parquetCompression, PARQUET_COMPRESSIONS, d.parquetCompression),
    rowGroupSize: int(r.rowGroupSize, d.rowGroupSize, MIN_BATCH_SIZE, MAX_BATCH_SIZE),
    useScope: pick(r.useScope, USE_SCOPES, d.useScope),
    iterations: int(r.iterations, d.iterations, 1, MAX_ITERATIONS),
    warmup: bool(r.warmup, d.warmup),
  };
}

export function findDataset(info: ApiInfo, name: string): DatasetInfo | null {
  return info.datasets.find((d) => d.name === name) ?? null;
}

/** Picks `name`, keeping the current dataset's columns and start id for when it is picked again. */
export function switchDataset(s: Settings, name: string): Settings {
  if (name === s.dataset) return s;
  const others = { ...s.otherDatasets };
  if (s.dataset !== '') others[s.dataset] = { columns: s.columns, offset: s.offset };
  const next = others[name] ?? { columns: null, offset: 0 };
  delete others[name];
  return { ...s, dataset: name, columns: next.columns, offset: next.offset, otherDatasets: others };
}

/**
 * Replaces a dataset the server does not have with its `default_dataset`.
 * Settings that never picked one ('', including settings saved before
 * datasets existed) keep their columns and start id: they were made for the
 * default `orders` table.
 */
export function resolveDataset(s: Settings, info: ApiInfo): Settings {
  if (findDataset(info, s.dataset)) return s;
  const fallback = findDataset(info, info.default_dataset) ?? info.datasets[0];
  if (!fallback) return s;
  return s.dataset === '' ? { ...s, dataset: fallback.name } : switchDataset(s, fallback.name);
}

function fitColumns(columns: string[] | null, catalog: readonly ColumnInfo[]): string[] | null {
  if (columns === null) return null;
  const known = new Set(catalog.map((c) => c.name));
  const kept = columns.filter((c) => known.has(c));
  // A selection that names none of these columns was made for another catalog: show all rather than none.
  if (kept.length === 0 && columns.length > 0) return null;
  return kept.length === catalog.length ? null : kept;
}

/** Makes settings consistent with what the server offers (dataset, row cap, known columns). */
export function fitToServer(s: Settings, info: ApiInfo): Settings {
  const resolved = resolveDataset(s, info);
  const catalog = findDataset(info, resolved.dataset)?.columns ?? [];
  return {
    ...resolved,
    rows: Math.min(resolved.rows, Math.max(1, info.max_rows)),
    columns: fitColumns(resolved.columns, catalog),
  };
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

/** The column groups a catalog uses: numeric, text, time first, then any others. */
export function columnGroups(catalog: readonly ColumnInfo[]): string[] {
  const present = new Set(catalog.map((c) => c.group));
  const known = COLUMN_GROUPS.filter((g) => present.has(g));
  const others = [...present].filter((g) => !(COLUMN_GROUPS as readonly string[]).includes(g)).sort();
  return [...known, ...others];
}

export function offsetMode(s: Pick<Settings, 'randomizeOffset'>): OffsetMode {
  return s.randomizeOffset ? 'random' : 'fixed';
}

export function buildRequest(s: Settings, format: DataFormat, offset: number, columns: string[] | null): DataRequest {
  return {
    dataset: s.dataset,
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

/**
 * A random start id that still leaves about `rows` rows after it. Keys can
 * be sparse (an order line's id is order_id * 10 + line_no), so the rows
 * are converted to a key span with the dataset's key density.
 */
export function randomOffset(
  dataset: Pick<DatasetInfo, 'max_key' | 'row_estimate'>,
  rows: number,
  random: () => number = Math.random,
): number {
  const maxKey = dataset.max_key ?? 0;
  const density = dataset.row_estimate && maxKey > 0 ? Math.min(1, dataset.row_estimate / maxKey) : 1;
  const span = Math.max(0, Math.floor(maxKey - rows / density));
  return Math.floor(random() * (span + 1));
}
