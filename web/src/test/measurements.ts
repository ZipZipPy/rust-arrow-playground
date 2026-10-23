import type { Measurement } from '../lib/types';

let nextId = 1;

export function makeMeasurement(overrides: Partial<Measurement> = {}): Measurement {
  const id = nextId++;
  return {
    id,
    batchId: 1,
    kind: 'run',
    at: '2026-01-02T03:04:05.000Z',
    target: 'same origin',
    region: null,
    dataset: 'orders',
    format: 'json',
    variant: 'JSON',
    rowsRequested: 1000,
    offset: 0,
    offsetMode: 'fixed',
    columnsKey: 'all',
    rowCount: 1000,
    columnCount: 20,
    httpCompression: true,
    contentEncoding: 'gzip',
    rowCache: false,
    cache: 'off',
    dbMs: 10,
    encodeMs: 5,
    compressMs: 1,
    serverTotalMs: 16,
    ttfbMs: 20,
    networkWaitMs: 4,
    downloadMs: 8,
    decodeMs: 12,
    useMs: 1,
    otherMs: 2,
    endToEndMs: 42,
    wireBytes: 50_000,
    transferBytes: 50_300,
    payloadBytes: 380_000,
    timingSource: 'resource-timing',
    useScope: 'all',
    useColumn: null,
    useValue: 123.45,
    ...overrides,
  };
}
