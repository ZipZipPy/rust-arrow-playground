import { zstdCompressSync } from 'node:zlib';
import { type Codec, CompressionType, compressionRegistry, DataType, tableFromIPC, tableToIPC } from 'apache-arrow';
import lz4js from 'lz4js';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { parquet } from '../test/parquetNode';
import {
  expectedRatingSum,
  expectedTotal,
  sampleRows,
  toArrowTable,
  toBatchedArrowTable,
  toJsonRows,
} from '../test/sampleData';
import { lz4FrameCodec, zstdCodec } from './codecs';
import {
  type Decoded,
  decodeArrowIpc,
  decodeJson,
  decodeParquet,
  decodePayload,
  pickUseColumn,
  previewRows,
  rowCount,
  useColumnValue,
} from './decode';
import type { ColumnInfo } from './types';

const ROWS = sampleRows(2_500);
const JSON_BYTES = new TextEncoder().encode(JSON.stringify(toJsonRows(ROWS)));

function expectSameAsSample(decoded: Decoded) {
  expect(rowCount(decoded)).toBe(ROWS.length);
  expect(useColumnValue(decoded, 'total')).toBeCloseTo(expectedTotal(ROWS), 6);
  expect(useColumnValue(decoded, 'rating')).toBe(expectedRatingSum(ROWS));
  // The preview of every format must match the JSON the server would send.
  expect(previewRows(decoded, 20)).toEqual(previewRows(decodeJson(JSON_BYTES), 20));
}

// Encoders exist only to produce compressed test streams; decoding goes
// through the codecs the app registers.
const withEncoders: Record<'lz4' | 'zstd', [CompressionType, Codec]> = {
  lz4: [CompressionType.LZ4_FRAME, { ...lz4FrameCodec, encode: (b) => Uint8Array.from(lz4js.compress(b)) }],
  zstd: [CompressionType.ZSTD, { ...zstdCodec, encode: (b) => new Uint8Array(zstdCompressSync(b)) }],
};

describe('JSON', () => {
  it('decodes rows and touches the use column', () => {
    const decoded = decodeJson(JSON_BYTES);
    expect(decoded.kind).toBe('json');
    expectSameAsSample(decoded);
  });

  it('formats the preview like the server JSON', () => {
    const preview = previewRows(decodeJson(JSON_BYTES), 3);
    expect(preview.columns).toEqual([
      'id', 'created_at', 'ship_date', 'country', 'quantity', 'discount', 'total', 'is_returned', 'rating', 'notes',
    ]);
    expect(preview.rows).toHaveLength(3);
    expect(preview.rows[0]?.[1]).toMatch(/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{6}Z$/);
  });

  it('rejects non-array JSON', () => {
    expect(() => decodeJson(new TextEncoder().encode('{"error":"nope"}'))).toThrow(/array/);
  });

  it('counts non-null values when the use column is not numeric', () => {
    const decoded = decodeJson(JSON_BYTES);
    expect(useColumnValue(decoded, 'notes')).toBe(ROWS.filter((r) => r.notes !== null).length);
    expect(useColumnValue(decoded, null)).toBe(0);
  });
});

describe('Arrow IPC stream', () => {
  it('decodes an uncompressed stream', () => {
    expectSameAsSample(decodeArrowIpc(tableToIPC(toArrowTable(ROWS), 'stream')));
  });

  it('decodes several record batches with nulls spread over them', () => {
    const table = toBatchedArrowTable(ROWS, 1_024);
    expect(table.batches).toHaveLength(3);
    const decoded = decodeArrowIpc(tableToIPC(table, 'stream'));
    expect(decoded.kind === 'arrow' && decoded.table.batches.length).toBe(3);
    expectSameAsSample(decoded);
  });

  it('decodes dictionary-encoded columns', () => {
    const decoded = decodeArrowIpc(tableToIPC(toArrowTable(ROWS, true), 'stream'));
    if (decoded.kind !== 'arrow') throw new Error('expected arrow');
    expect(DataType.isDictionary(decoded.table.getChild('country')!.type)).toBe(true);
    expectSameAsSample(decoded);
    expect(useColumnValue(decoded, 'country')).toBe(ROWS.length);
  });

  describe.each(['lz4', 'zstd'] as const)('with %s body compression', (codec) => {
    const [type, codecWithEncoder] = withEncoders[codec];
    beforeAll(() => compressionRegistry.set(type, codecWithEncoder));
    afterAll(() => compressionRegistry.set(type, codec === 'lz4' ? lz4FrameCodec : zstdCodec));

    it('decodes through the registered codec', () => {
      const plain = tableToIPC(toBatchedArrowTable(ROWS, 1_000, true), 'stream');
      const compressed = tableToIPC(toBatchedArrowTable(ROWS, 1_000, true), 'stream', type);
      expect(compressed.byteLength).toBeLessThan(plain.byteLength);
      // Swap back to decode-only codecs to prove decoding does not need the encoder.
      compressionRegistry.set(type, codec === 'lz4' ? lz4FrameCodec : zstdCodec);
      expectSameAsSample(decodeArrowIpc(compressed));
    });
  });

  it('decodes the Arrow IPC file format as well', () => {
    expectSameAsSample(decodeArrowIpc(tableToIPC(toArrowTable(ROWS), 'file')));
  });

  it('decodes an empty stream (schema only)', () => {
    const empty = tableToIPC(toArrowTable([]), 'stream');
    const decoded = decodeArrowIpc(empty);
    expect(rowCount(decoded)).toBe(0);
    expect(useColumnValue(decoded, 'total')).toBe(0);
    expect(previewRows(decoded).rows).toEqual([]);
  });
});

describe('Parquet', () => {
  const wasmTable = () => parquet.Table.fromIPCStream(tableToIPC(toBatchedArrowTable(ROWS, 1_000), 'stream'));

  it.each([
    ['none', parquet.Compression.UNCOMPRESSED],
    ['snappy', parquet.Compression.SNAPPY],
    ['gzip', parquet.Compression.GZIP],
    ['zstd', parquet.Compression.ZSTD],
    ['lz4 (raw)', parquet.Compression.LZ4_RAW],
    ['brotli', parquet.Compression.BROTLI],
  ])('decodes a %s-compressed file', (_, compression) => {
    const props = new parquet.WriterPropertiesBuilder().setCompression(compression).setMaxRowGroupSize(1_000).build();
    const file = parquet.writeParquet(wasmTable(), props);
    expect(new TextDecoder().decode(file.subarray(0, 4))).toBe('PAR1');
    const decoded = decodeParquet(file, parquet);
    expectSameAsSample(decoded);
    if (decoded.kind !== 'arrow') throw new Error('expected arrow');
    expect(DataType.isTimestamp(decoded.table.getChild('created_at')!.type)).toBe(true);
  });

  it('is reachable through decodePayload and needs a loaded reader', () => {
    const file = parquet.writeParquet(wasmTable());
    expect(rowCount(decodePayload('parquet', file, parquet))).toBe(ROWS.length);
    expect(() => decodePayload('parquet', file, null)).toThrow(/not loaded/);
  });
});

describe('decodePayload', () => {
  it('dispatches on format', () => {
    expect(decodePayload('json', JSON_BYTES, null).kind).toBe('json');
    expect(decodePayload('arrow', tableToIPC(toArrowTable(ROWS)), null).kind).toBe('arrow');
  });

  it('keeps the original buffer usable (no detaching)', () => {
    const ipc = tableToIPC(toArrowTable(ROWS));
    decodeArrowIpc(ipc);
    expect(tableFromIPC(ipc).numRows).toBe(ROWS.length);
  });
});

describe('pickUseColumn', () => {
  const col = (name: string, arrow_type: string): ColumnInfo => ({
    name, arrow_type, pg_type: '', nullable: false, dictionary: false, group: 'numeric',
  });

  it('prefers total, then the first numeric column, then the first column', () => {
    expect(pickUseColumn([col('id', 'Int64'), col('total', 'Float64')])).toBe('total');
    expect(pickUseColumn([col('country', 'Utf8'), col('quantity', 'Int16')])).toBe('quantity');
    expect(pickUseColumn([col('created_at', 'Timestamp(Microsecond, "UTC")'), col('is_returned', 'Boolean')])).toBe('created_at');
    expect(pickUseColumn([])).toBeNull();
  });
});
