import {
  type Data,
  DataType,
  DateUnit,
  makeVector,
  Precision,
  type Table,
  TimeUnit,
  tableFromIPC,
  type Vector,
} from 'apache-arrow';
import { registerIpcCodecs } from './codecs';
import type { ColumnInfo, DataFormat, PreviewTable, UseScope } from './types';

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
const FLOAT_ARROW_TYPE = /^Float(16|32|64)$/;

/**
 * The column a one-column "use" step reads: an amount worth summing when
 * selected (`total`, `line_total`, otherwise the first floating-point
 * column), otherwise the first numeric column, otherwise the first column.
 * Chosen from the catalog so every format reads the same column.
 */
export function pickUseColumn(columns: readonly ColumnInfo[]): string | null {
  const preferences: ((c: ColumnInfo) => boolean)[] = [
    (c) => c.name === 'total',
    (c) => c.name.endsWith('_total'),
    (c) => FLOAT_ARROW_TYPE.test(c.arrow_type),
    (c) => NUMERIC_ARROW_TYPE.test(c.arrow_type),
    () => true,
  ];
  for (const prefer of preferences) {
    const found = columns.find(prefer);
    if (found) return found.name;
  }
  return null;
}

/** The columns the "use" step reads, in catalog order. */
export function useColumns(scope: UseScope, columns: readonly ColumnInfo[]): string[] {
  if (scope === 'all') return columns.map((c) => c.name);
  const name = pickUseColumn(columns);
  return name === null ? [] : [name];
}

/** A value's share of the use checksum: numbers as they are, strings by length, true as 1. */
function fold(value: unknown): number {
  switch (typeof value) {
    case 'number':
      return value;
    case 'bigint':
      return Number(value);
    case 'string':
      return value.length;
    case 'boolean':
      return value ? 1 : 0;
    default:
      if (value === null || value === undefined) return 0;
      return value instanceof Date ? value.getTime() : 1;
  }
}

// The JSON passes are written the way an app would read the rows, so JSON is
// not charged for a slow access pattern: a property lookup that sees many
// different names becomes megamorphic in V8 and costs 2-10x more per value.

/** One column: a loop whose lookup only ever sees that column's name. */
function useJsonColumn(rows: readonly JsonRow[], name: string): number {
  let sum = 0;
  for (let i = 0; i < rows.length; i++) sum += fold(rows[i]![name]);
  return sum;
}

/** Every property of every row: for-in walks V8's cached key list of same-shaped objects. */
function useJsonRows(rows: readonly JsonRow[]): number {
  let sum = 0;
  for (let i = 0; i < rows.length; i++) {
    const row = rows[i]!;
    for (const key in row) sum += fold(row[key]);
  }
  return sum;
}

function useJson(rows: readonly JsonRow[], columns: readonly string[]): number {
  if (columns.length === 1) return useJsonColumn(rows, columns[0]!);
  // The server sends exactly the selected columns, so "every column" is every property.
  const first = rows[0];
  if (first && columns.length === Object.keys(first).length && columns.every((c) => c in first)) return useJsonRows(rows);
  let sum = 0;
  for (const row of rows) {
    for (let c = 0; c < columns.length; c++) sum += fold(row[columns[c]!]);
  }
  return sum;
}

const MS_PER_DAY = 86_400_000;
const MS_PER_TIME_UNIT: Record<TimeUnit, number> = {
  [TimeUnit.SECOND]: 1000,
  [TimeUnit.MILLISECOND]: 1,
  [TimeUnit.MICROSECOND]: 1e-3,
  [TimeUnit.NANOSECOND]: 1e-6,
};

const isValid = (bitmap: Uint8Array, bit: number) => ((bitmap[bit >> 3]! >> (bit & 7)) & 1) === 1;

/** Fallback for types without a fast path: Arrow JS's own per-value getters. */
function useGeneric(data: Data): number {
  let sum = 0;
  for (const value of makeVector(data)) sum += fold(value);
  return sum;
}

/** Numbers straight from the typed array, scaled to milliseconds for dates and timestamps. */
function useNumbers(data: Data, nulls: Uint8Array | null, scale: number): number {
  const { length, offset } = data;
  let sum = 0;
  if (data.values instanceof BigInt64Array || data.values instanceof BigUint64Array) {
    const values = data.values;
    for (let i = 0; i < length; i++) {
      if (nulls === null || isValid(nulls, offset + i)) sum += Number(values[i]!) * scale;
    }
    return sum;
  }
  const values = data.values as ArrayLike<number>;
  for (let i = 0; i < length; i++) {
    if (nulls === null || isValid(nulls, offset + i)) sum += values[i]! * scale;
  }
  return sum;
}

function useBools(data: Data, nulls: Uint8Array | null): number {
  const { length, offset } = data;
  const bits = data.values as Uint8Array;
  let sum = 0;
  for (let i = 0; i < length; i++) {
    const bit = offset + i;
    if (nulls === null || isValid(nulls, bit)) sum += (bits[bit >> 3]! >> (bit & 7)) & 1;
  }
  return sum;
}

// fatal: invalid UTF-8 throws instead of turning into U+FFFD, so byte and
// character positions stay in step; ignoreBOM keeps a leading U+FEFF in its value.
const strictUtf8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });

/**
 * Builds every string of a Utf8 chunk. One TextDecoder call per value costs
 * more than the decoding itself for short strings, so the whole buffer is
 * decoded once (one native pass, like JSON.parse) and sliced into the same
 * strings a value-by-value decode would give.
 */
function useUtf8(data: Data, nulls: Uint8Array | null): number {
  const { length, offset } = data;
  const offsets = data.valueOffsets as Int32Array;
  const bytes = data.values as Uint8Array;
  const first = offsets[0]!;
  const last = offsets[length]!;
  let text: string;
  try {
    text = strictUtf8.decode(bytes.subarray(first, last));
  } catch {
    return useGeneric(data);
  }
  const ascii = text.length === last - first;
  let sum = 0;
  let byte = first;
  let start = 0;
  let end = 0;
  for (let i = 0; i < length; i++) {
    const next = offsets[i + 1]!;
    if (ascii) {
      end = next - first;
    } else {
      // Each UTF-8 sequence is one UTF-16 unit, or two from 4 bytes up; continuation bytes add none.
      for (; byte < next; byte++) {
        const b = bytes[byte]!;
        if ((b & 0xc0) !== 0x80) end += b >= 0xf0 ? 2 : 1;
      }
    }
    if (nulls === null || isValid(nulls, offset + i)) sum += text.slice(start, end).length;
    start = end;
  }
  return sum;
}

/** Each distinct value is built once, then looked up by key, as a consumer holding the dictionary would. */
function useDictionary(data: Data, nulls: Uint8Array | null, built: Map<Vector, unknown[]>): number {
  const { dictionary } = data;
  if (!dictionary) return 0;
  let entries = built.get(dictionary);
  if (!entries) {
    entries = [...dictionary];
    built.set(dictionary, entries);
  }
  const { length, offset } = data;
  const keys = data.values as ArrayLike<number | bigint>;
  let sum = 0;
  for (let i = 0; i < length; i++) {
    if (nulls === null || isValid(nulls, offset + i)) sum += fold(entries[Number(keys[i]!)]);
  }
  return sum;
}

function useArrowData(data: Data, dictionaries: Map<Vector, unknown[]>): number {
  const { type, nullBitmap } = data;
  const nulls = data.nullCount > 0 && nullBitmap !== undefined && nullBitmap.length > 0 ? nullBitmap : null;
  if (DataType.isDictionary(type)) return useDictionary(data, nulls, dictionaries);
  if (DataType.isUtf8(type)) return useUtf8(data, nulls);
  if (DataType.isBool(type)) return useBools(data, nulls);
  if (DataType.isInt(type)) return useNumbers(data, nulls, 1);
  if (DataType.isFloat(type) && type.precision !== Precision.HALF) return useNumbers(data, nulls, 1);
  if (DataType.isTimestamp(type)) return useNumbers(data, nulls, MS_PER_TIME_UNIT[type.unit]);
  if (DataType.isDate(type)) return useNumbers(data, nulls, type.unit === DateUnit.DAY ? MS_PER_DAY : 1);
  return useGeneric(data);
}

/**
 * The "use" step: reads every value of `columns` once, as the JavaScript value
 * a consumer works with (number, string, boolean, epoch milliseconds), and
 * folds it into a checksum so the work cannot be skipped. JSON built these
 * values while parsing, so for JSON this is a quick pass; Arrow and Parquet
 * build them here, from typed arrays and UTF-8 buffers. For one numeric
 * column the checksum is the column's sum. JSON carries dates and timestamps
 * as text, so checksums over time columns differ between formats.
 */
export function useValues(decoded: Decoded, columns: readonly string[]): number {
  if (decoded.kind === 'json') return useJson(decoded.rows, columns);
  const dictionaries = new Map<Vector, unknown[]>();
  let sum = 0;
  for (const name of columns) {
    const column = decoded.table.getChild(name);
    if (!column) continue;
    for (const data of column.data) sum += useArrowData(data, dictionaries);
  }
  return sum;
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
