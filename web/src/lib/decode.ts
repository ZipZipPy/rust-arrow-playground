import { DataType, Precision, type Table, tableFromIPC } from 'apache-arrow';
import { registerIpcCodecs } from './codecs';
import type { ColumnInfo, DataFormat, PreviewTable } from './types';

registerIpcCodecs();

export type JsonRow = Record<string, unknown>;

/** A decoded response: plain row objects for JSON, an Arrow table otherwise. */
export type Decoded = { kind: 'json'; rows: JsonRow[] } | { kind: 'arrow'; table: Table };

/** The part of parquet-wasm the decoder needs (injected so Node tests can supply their own build). */
export interface ParquetReader {
  readParquet(bytes: Uint8Array, options?: { batchSize?: number }): { intoIPCStream(): Uint8Array };
}

// parquet-wasm defaults to 1024-row batches; larger batches mean far fewer
// Arrow JS objects to build without changing the work done per value.
const PARQUET_READ_BATCH = 65_536;

const utf8 = new TextDecoder();

export function decodeJson(bytes: Uint8Array): Decoded {
  const parsed: unknown = JSON.parse(utf8.decode(bytes));
  if (!Array.isArray(parsed)) throw new Error('Expected a JSON array of rows');
  return { kind: 'json', rows: parsed as JsonRow[] };
}

export function decodeArrowIpc(bytes: Uint8Array): Decoded {
  return { kind: 'arrow', table: tableFromIPC(bytes) };
}

export function decodeParquet(bytes: Uint8Array, reader: ParquetReader): Decoded {
  // `intoIPCStream` consumes (frees) the wasm-side table.
  const ipc = reader.readParquet(bytes, { batchSize: PARQUET_READ_BATCH }).intoIPCStream();
  return { kind: 'arrow', table: tableFromIPC(ipc) };
}

export function decodePayload(format: DataFormat, bytes: Uint8Array, parquet: ParquetReader | null): Decoded {
  switch (format) {
    case 'json':
      return decodeJson(bytes);
    case 'arrow':
      return decodeArrowIpc(bytes);
    case 'parquet':
      if (!parquet) throw new Error('Parquet decoder is not loaded');
      return decodeParquet(bytes, parquet);
  }
}

export function rowCount(decoded: Decoded): number {
  return decoded.kind === 'json' ? decoded.rows.length : decoded.table.numRows;
}

export function columnNames(decoded: Decoded): string[] {
  if (decoded.kind === 'arrow') return decoded.table.schema.fields.map((f) => f.name);
  const first = decoded.rows[0];
  return first ? Object.keys(first) : [];
}

const NUMERIC_ARROW_TYPE = /^(U?Int(8|16|32|64)|Float(16|32|64))$/;

/**
 * The column the "use" step touches: `total` when selected, otherwise the
 * first numeric column, otherwise the first column (whose non-null values are
 * counted). Chosen from the catalog so every format touches the same column.
 */
export function pickUseColumn(columns: readonly ColumnInfo[]): string | null {
  const total = columns.find((c) => c.name === 'total');
  if (total) return total.name;
  const numeric = columns.find((c) => NUMERIC_ARROW_TYPE.test(c.arrow_type));
  return (numeric ?? columns[0])?.name ?? null;
}

function sumJson(rows: JsonRow[], name: string): number {
  let sum = 0;
  let numeric = false;
  for (const row of rows) {
    const value = row[name];
    if (typeof value === 'number') {
      sum += value;
      numeric = true;
    }
  }
  if (numeric) return sum;
  let present = 0;
  for (const row of rows) if (row[name] != null) present++;
  return present;
}

function sumArrow(table: Table, name: string): number {
  const column = table.getChild(name);
  if (!column) return 0;
  if (!DataType.isInt(column.type) && !DataType.isFloat(column.type)) {
    return column.length - column.nullCount;
  }
  let sum = 0;
  for (const chunk of column.data) {
    const { length, offset, nullBitmap } = chunk;
    const values = chunk.values as ArrayLike<number | bigint>;
    const isBigInt = values instanceof BigInt64Array || values instanceof BigUint64Array;
    const hasNulls = chunk.nullCount > 0 && nullBitmap !== undefined && nullBitmap.length > 0;
    for (let i = 0; i < length; i++) {
      if (hasNulls) {
        const bit = offset + i;
        if (((nullBitmap[bit >> 3]! >> (bit & 7)) & 1) === 0) continue;
      }
      sum += isBigInt ? Number(values[i]) : (values[i] as number);
    }
  }
  return sum;
}

/** Touches every row of one column, so the timing includes actually reading the data. */
export function useColumnValue(decoded: Decoded, name: string | null): number {
  if (name === null) return 0;
  return decoded.kind === 'json' ? sumJson(decoded.rows, name) : sumArrow(decoded.table, name);
}

function pad(n: number, width: number): string {
  return String(n).padStart(width, '0');
}

/**
 * Arrow JS returns timestamps as fractional epoch milliseconds. Print them the
 * way the server's JSON does: RFC 3339 UTC with six fractional digits.
 */
function formatTimestampMs(ms: number): string {
  const whole = Math.floor(ms);
  const micros = Math.min(999, Math.round((ms - whole) * 1000));
  return `${new Date(whole).toISOString().slice(0, -1)}${pad(micros, 3)}Z`;
}

/** Shortest decimal that round-trips through float32, as serde_json prints an f32. */
function formatFloat32(value: number): string {
  for (let digits = 1; digits <= 9; digits++) {
    const candidate = Number(value.toPrecision(digits));
    if (Math.fround(candidate) === value) return String(candidate);
  }
  return String(value);
}

function formatArrowCell(type: DataType, value: unknown): string | null {
  if (value === null || value === undefined) return null;
  if (DataType.isTimestamp(type)) return formatTimestampMs(Number(value));
  if (DataType.isDate(type)) return new Date(Number(value)).toISOString().slice(0, 10);
  if (DataType.isFloat(type) && type.precision === Precision.SINGLE) return formatFloat32(Number(value));
  return String(value);
}

function formatJsonCell(value: unknown): string | null {
  if (value === null || value === undefined) return null;
  return typeof value === 'object' ? JSON.stringify(value) : String(value);
}

export function previewRows(decoded: Decoded, limit = 20): PreviewTable {
  const columns = columnNames(decoded);
  if (decoded.kind === 'json') {
    return {
      columns,
      rows: decoded.rows.slice(0, limit).map((row) => columns.map((c) => formatJsonCell(row[c]))),
    };
  }
  const { table } = decoded;
  const n = Math.min(limit, table.numRows);
  const vectors = table.schema.fields.map((field, i) => ({ field, vector: table.getChildAt(i)! }));
  const rows: (string | null)[][] = [];
  for (let r = 0; r < n; r++) {
    rows.push(vectors.map(({ field, vector }) => formatArrowCell(field.type, vector.get(r))));
  }
  return { columns, rows };
}
