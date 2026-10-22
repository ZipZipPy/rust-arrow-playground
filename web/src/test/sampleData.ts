// A small `orders`-like dataset built twice: as the JSON rows the server sends
// and as an Arrow table with the catalog's types, so tests can check that every
// decoding path yields the same values.
import {
  Bool,
  DateDay,
  Dictionary,
  Float32,
  Float64,
  Int16,
  Int32,
  Table,
  TimestampMicrosecond,
  Utf8,
  makeData,
  makeVector,
  vectorFromArray,
} from 'apache-arrow';
import type { JsonRow } from '../lib/decode';

const COUNTRIES = ['GB', 'US', 'DE', 'TH', 'JP'];
const DISCOUNTS = [0, 0.05, 0.1, 0.15, 0.2, 0.25];
const BASE_MICROS = 1_735_732_800_123_456n; // 2025-01-01T12:00:00.123456Z
const DAY_MS = 86_400_000;

export interface SampleRow {
  id: bigint;
  createdMicros: bigint;
  shipDay: number | null;
  country: string;
  quantity: number;
  discount: number;
  total: number;
  isReturned: boolean;
  rating: number | null;
  notes: string | null;
}

export function sampleRows(count: number, firstId = 1): SampleRow[] {
  return Array.from({ length: count }, (_, i) => {
    const id = firstId + i;
    const createdMicros = BASE_MICROS + BigInt(id) * 3_600_000_123n;
    const createdDay = Math.floor(Number(createdMicros / 1000n) / DAY_MS);
    return {
      id: BigInt(id),
      createdMicros,
      shipDay: id % 7 === 0 ? null : createdDay + (id % 10) + 1,
      country: COUNTRIES[id % COUNTRIES.length]!,
      quantity: (id % 20) + 1,
      discount: DISCOUNTS[id % DISCOUNTS.length]!,
      total: Math.round(id * 13.37) / 100,
      isReturned: id % 20 === 0,
      rating: id % 5 === 0 ? null : (id % 5) + 1,
      notes: id % 3 === 0 ? `note ${id}` : null,
    };
  });
}

function isoMicros(micros: bigint): string {
  const ms = Number(micros / 1000n);
  const frac = String(micros % 1_000_000n).padStart(6, '0');
  return `${new Date(ms).toISOString().slice(0, 19)}.${frac}Z`;
}

export function toJsonRows(rows: SampleRow[]): JsonRow[] {
  return rows.map((r) => ({
    id: Number(r.id),
    created_at: isoMicros(r.createdMicros),
    ship_date: r.shipDay === null ? null : new Date(r.shipDay * DAY_MS).toISOString().slice(0, 10),
    country: r.country,
    quantity: r.quantity,
    discount: r.discount,
    total: r.total,
    is_returned: r.isReturned,
    rating: r.rating,
    notes: r.notes,
  }));
}

function validity(values: readonly unknown[]): Uint8Array {
  const bitmap = new Uint8Array(Math.ceil(values.length / 8));
  values.forEach((v, i) => {
    if (v !== null) bitmap[i >> 3]! |= 1 << (i & 7);
  });
  return bitmap;
}

function nullable<T>(values: (T | null)[], fill: T): { filled: T[]; nullBitmap: Uint8Array } {
  return { filled: values.map((v) => v ?? fill), nullBitmap: validity(values) };
}

export function toArrowTable(rows: SampleRow[], dictionary = false): Table {
  const length = rows.length;
  const ship = nullable(rows.map((r) => r.shipDay), 0);
  const rating = nullable(rows.map((r) => r.rating), 0);
  return new Table({
    id: makeVector(BigInt64Array.from(rows.map((r) => r.id))),
    created_at: makeVector(
      makeData({ type: new TimestampMicrosecond('UTC'), length, data: BigInt64Array.from(rows.map((r) => r.createdMicros)) }),
    ),
    ship_date: makeVector(
      makeData({ type: new DateDay(), length, data: Int32Array.from(ship.filled), nullBitmap: ship.nullBitmap }),
    ),
    country: vectorFromArray(
      rows.map((r) => r.country),
      dictionary ? new Dictionary(new Utf8(), new Int32()) : new Utf8(),
    ),
    quantity: vectorFromArray(rows.map((r) => r.quantity), new Int16()),
    discount: vectorFromArray(rows.map((r) => r.discount), new Float32()),
    total: vectorFromArray(rows.map((r) => r.total), new Float64()),
    is_returned: vectorFromArray(rows.map((r) => r.isReturned), new Bool()),
    rating: makeVector(makeData({ type: new Int16(), length, data: Int16Array.from(rating.filled), nullBitmap: rating.nullBitmap })),
    notes: vectorFromArray(rows.map((r) => r.notes), new Utf8()),
  });
}

/** Several record batches, like a server response with a small batch_size. */
export function toBatchedArrowTable(rows: SampleRow[], batchSize: number, dictionary = false): Table {
  const tables: Table[] = [];
  for (let i = 0; i < rows.length; i += batchSize) tables.push(toArrowTable(rows.slice(i, i + batchSize), dictionary));
  const [first, ...rest] = tables;
  if (!first) throw new Error('no rows');
  return first.concat(...rest);
}

export const expectedTotal = (rows: SampleRow[]) => rows.reduce((sum, r) => sum + r.total, 0);
export const expectedRatingSum = (rows: SampleRow[]) => rows.reduce((sum, r) => sum + (r.rating ?? 0), 0);

/** Columns whose values are the same JavaScript value in every format (dates are text in JSON). */
export const NON_TIME_COLUMNS = ['id', 'country', 'quantity', 'discount', 'total', 'is_returned', 'rating', 'notes'];
export const TIME_COLUMNS = ['created_at', 'ship_date'];

/**
 * The "use" checksum over every value of the non-time columns: numbers as
 * they are, strings by length, true as 1, null as 0.
 */
export const expectedNonTimeChecksum = (rows: SampleRow[]) =>
  rows.reduce(
    (sum, r) =>
      sum +
      Number(r.id) +
      r.country.length +
      r.quantity +
      r.discount +
      r.total +
      (r.isReturned ? 1 : 0) +
      (r.rating ?? 0) +
      (r.notes?.length ?? 0),
    0,
  );

/** Time columns: epoch milliseconds from Arrow, the length of the RFC 3339 / ISO date text from JSON. */
export const expectedTimeChecksum = (rows: SampleRow[], kind: 'json' | 'arrow') =>
  rows.reduce((sum, r) => {
    if (kind === 'json') return sum + 27 + (r.shipDay === null ? 0 : 10);
    return sum + Number(r.createdMicros) / 1000 + (r.shipDay === null ? 0 : r.shipDay * DAY_MS);
  }, 0);
