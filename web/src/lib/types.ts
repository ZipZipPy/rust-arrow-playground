// Shapes shared by the API client, the measurement code and the UI.
// Server payloads mirror the "HTTP API" section of docs/DESIGN.md.

export const DATA_FORMATS = ['json', 'arrow', 'parquet'] as const;
export type DataFormat = (typeof DATA_FORMATS)[number];

export const IPC_COMPRESSIONS = ['none', 'lz4', 'zstd'] as const;
export type IpcCompression = (typeof IPC_COMPRESSIONS)[number];

export const PARQUET_COMPRESSIONS = ['none', 'snappy', 'gzip', 'zstd', 'lz4', 'brotli'] as const;
export type ParquetCompression = (typeof PARQUET_COMPRESSIONS)[number];

/** What the "use" step reads after decoding: every selected value, or one column. */
export const USE_SCOPES = ['all', 'column'] as const;
export type UseScope = (typeof USE_SCOPES)[number];

export interface ColumnInfo {
  name: string;
  pg_type: string;
  arrow_type: string;
  nullable: boolean;
  dictionary: boolean;
  group: string;
}

export type DatasetKind = 'table' | 'view' | 'materialized_view';

/** One relation the server can serve (see "Datasets" in docs/DESIGN.md). */
export interface DatasetInfo {
  name: string;
  label: string;
  kind: DatasetKind;
  relation: string;
  key_column: string;
  description: string;
  /** The defining SELECT, pretty-printed, for "Show SQL". */
  sql: string;
  /** null when unknown (the live view before the first seed). */
  row_estimate: number | null;
  max_key: number | null;
  bytes: number | null;
  columns: ColumnInfo[];
}

/** The `seed_status` row: null before the first seed. */
export interface SeedStatus {
  state: 'loading' | 'complete';
  schema_version: number;
  orders: number;
  order_items: number;
  finished_at: string | null;
}

export interface ServerMeta {
  version: string;
  region: string | null;
  service: string | null;
  revision: string | null;
}

export interface ApiInfo {
  default_dataset: string;
  datasets: DatasetInfo[];
  seed: SeedStatus | null;
  max_rows: number;
  formats: string[];
  ipc_compression: string[];
  parquet_compression: string[];
  http_compression: string[];
  server: ServerMeta;
}

export interface BenchVariant {
  format: DataFormat;
  variant: string;
  options: Record<string, unknown>;
  encode_ms: number;
  bytes: number;
}

export interface BenchReport {
  rows: number;
  columns: number;
  db_ms: number;
  results: BenchVariant[];
}

/** Everything that goes into one `/api/data` request. */
export interface DataRequest {
  dataset: string;
  format: DataFormat;
  rows: number;
  offset: number;
  /** `null` means every column (the parameter is omitted). */
  columns: string[] | null;
  httpCompression: boolean;
  cache: boolean;
  dictionary: boolean;
  ipcCompression: IpcCompression;
  parquetCompression: ParquetCompression;
  /** Arrow: rows per record batch. Parquet: max row-group size. */
  batchSize: number;
}

/** Values the server reports in `X-*` / `Server-Timing` headers. */
export interface ServerHeaders {
  dbMs: number | null;
  encodeMs: number | null;
  /** Server-side HTTP compression; 0 when none was applied. */
  compressMs: number | null;
  totalMs: number | null;
  rowCount: number | null;
  columnCount: number | null;
  lastId: number | null;
  bodyBytes: number | null;
  /** Bytes actually sent, after HTTP compression. */
  encodedBytes: number | null;
  format: string | null;
  dataset: string | null;
  cache: string | null;
  contentEncoding: string | null;
  contentLength: number | null;
}

export type TimingSource = 'resource-timing' | 'fetch';

/** Whether the start id was fixed or drawn at random for every round. */
export type OffsetMode = 'fixed' | 'random';

/** One measured request, as stored in the history. */
export interface Measurement {
  id: number;
  /** Requests started by the same click share a batch id. */
  batchId: number;
  kind: 'run' | 'compare';
  at: string;
  target: string;
  dataset: string;
  format: DataFormat;
  variant: string;
  rowsRequested: number;
  offset: number;
  offsetMode: OffsetMode;
  columnsKey: string;
  rowCount: number;
  columnCount: number;
  httpCompression: boolean;
  contentEncoding: string | null;
  /** The request's `cache` flag; `cache` below is the server's X-Cache answer. */
  rowCache: boolean;
  cache: string | null;
  dbMs: number | null;
  encodeMs: number | null;
  compressMs: number | null;
  serverTotalMs: number | null;
  ttfbMs: number;
  networkWaitMs: number;
  downloadMs: number;
  decodeMs: number;
  useMs: number;
  otherMs: number;
  endToEndMs: number;
  wireBytes: number | null;
  transferBytes: number | null;
  payloadBytes: number;
  timingSource: TimingSource;
  useScope: UseScope;
  /** The column the "use" step read when `useScope` is 'column'; null otherwise. */
  useColumn: string | null;
  useValue: number;
}

/** First rows of a decoded payload, already formatted for display. */
export interface PreviewTable {
  columns: string[];
  rows: (string | null)[][];
}
