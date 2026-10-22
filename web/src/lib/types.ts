// Shapes shared by the API client, the measurement code and the UI.
// Server payloads mirror the "HTTP API" section of docs/DESIGN.md.

export const DATA_FORMATS = ['json', 'arrow', 'parquet'] as const;
export type DataFormat = (typeof DATA_FORMATS)[number];

export const IPC_COMPRESSIONS = ['none', 'lz4', 'zstd'] as const;
export type IpcCompression = (typeof IPC_COMPRESSIONS)[number];

export const PARQUET_COMPRESSIONS = ['none', 'snappy', 'gzip', 'zstd', 'lz4', 'brotli'] as const;
export type ParquetCompression = (typeof PARQUET_COMPRESSIONS)[number];

export interface ColumnInfo {
  name: string;
  pg_type: string;
  arrow_type: string;
  nullable: boolean;
  dictionary: boolean;
  group: string;
}

export interface ServerMeta {
  version: string;
  region: string | null;
  service: string | null;
  revision: string | null;
}

export interface ApiInfo {
  table: string;
  row_estimate: number;
  max_id: number;
  table_bytes: number;
  max_rows: number;
  columns: ColumnInfo[];
  formats: string[];
  ipc_compression: string[];
  parquet_compression: string[];
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
  totalMs: number | null;
  rowCount: number | null;
  columnCount: number | null;
  lastId: number | null;
  bodyBytes: number | null;
  format: string | null;
  cache: string | null;
  contentEncoding: string | null;
  contentLength: number | null;
}

export type TimingSource = 'resource-timing' | 'fetch';

/** One measured request, as stored in the history. */
export interface Measurement {
  id: number;
  /** Requests started by the same click share a batch id. */
  batchId: number;
  kind: 'run' | 'compare';
  at: string;
  target: string;
  format: DataFormat;
  variant: string;
  rowsRequested: number;
  offset: number;
  columnsKey: string;
  rowCount: number;
  columnCount: number;
  httpCompression: boolean;
  contentEncoding: string | null;
  cache: string | null;
  dbMs: number | null;
  encodeMs: number | null;
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
  useColumn: string | null;
  useValue: number;
}

/** First rows of a decoded payload, already formatted for display. */
export interface PreviewTable {
  columns: string[];
  rows: (string | null)[][];
}
