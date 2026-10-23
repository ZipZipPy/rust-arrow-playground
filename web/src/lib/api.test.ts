import { describe, expect, it } from 'vitest';
import {
  apiUrl,
  benchParams,
  dataParams,
  makeNonce,
  normalizeBaseUrl,
  parseServerTiming,
  readServerHeaders,
  targetLabel,
} from './api';
import type { DataRequest } from './types';

const base: DataRequest = {
  dataset: 'orders',
  format: 'json',
  rows: 10_000,
  offset: 0,
  columns: null,
  httpCompression: true,
  cache: false,
  dictionary: true,
  ipcCompression: 'zstd',
  parquetCompression: 'gzip',
  batchSize: 4096,
};

describe('normalizeBaseUrl', () => {
  it('treats blank as same origin', () => {
    expect(normalizeBaseUrl('  ')).toBe('');
  });

  it('drops trailing slashes and a trailing /api', () => {
    expect(normalizeBaseUrl('https://svc-abc.a.run.app/')).toBe('https://svc-abc.a.run.app');
    expect(normalizeBaseUrl('https://svc-abc.a.run.app/api/')).toBe('https://svc-abc.a.run.app');
    expect(normalizeBaseUrl('http://localhost:8080/prefix/api')).toBe('http://localhost:8080/prefix');
  });

  it('rejects non-URLs and non-http schemes', () => {
    expect(() => normalizeBaseUrl('localhost:8080')).toThrow();
    expect(() => normalizeBaseUrl('ftp://example.com')).toThrow(/http/);
    expect(() => normalizeBaseUrl('not a url')).toThrow(/full URL/);
  });

  it('labels targets', () => {
    expect(targetLabel('')).toBe('same origin');
    expect(targetLabel('https://svc.run.app')).toBe('svc.run.app');
  });
});

describe('apiUrl', () => {
  it('resolves same-origin paths against the page origin', () => {
    expect(apiUrl('', 'ping', undefined, 'http://localhost:5173')).toBe('http://localhost:5173/api/ping');
  });

  it('uses an absolute base as is', () => {
    const params = new URLSearchParams({ rows: '5' });
    expect(apiUrl('https://svc.run.app', 'data', params, 'http://ignored')).toBe('https://svc.run.app/api/data?rows=5');
  });
});

describe('dataParams', () => {
  it('sends only the parameters JSON uses', () => {
    expect(Object.fromEntries(dataParams(base))).toEqual({
      dataset: 'orders',
      format: 'json',
      rows: '10000',
      http_compression: 'true',
    });
  });

  it('adds Arrow options, offset, columns, cache and the nonce', () => {
    const params = dataParams(
      { ...base, dataset: 'order_lines', format: 'arrow', offset: 500, columns: ['line_id', 'line_total'], cache: true, httpCompression: false },
      'n1',
    );
    expect(params.toString()).toBe(
      'dataset=order_lines&format=arrow&rows=10000&offset=500&columns=line_id%2Cline_total&http_compression=false&cache=true' +
        '&dictionary=true&ipc_compression=zstd&batch_size=4096&_=n1',
    );
  });

  it('adds Parquet options', () => {
    expect(Object.fromEntries(dataParams({ ...base, format: 'parquet' }))).toEqual({
      dataset: 'orders',
      format: 'parquet',
      rows: '10000',
      http_compression: 'true',
      parquet_compression: 'gzip',
      batch_size: '4096',
    });
  });

  it('builds bench parameters', () => {
    expect(benchParams({ dataset: 'daily_sales', rows: 1000, offset: 0, columns: ['a'], cache: false }).toString()).toBe(
      'dataset=daily_sales&rows=1000&columns=a',
    );
    expect(benchParams({ dataset: 'orders', rows: 5, offset: 9, columns: null, cache: true }, 'n').toString()).toBe(
      'dataset=orders&rows=5&offset=9&cache=true&_=n',
    );
  });

  it('makes unique nonces', () => {
    const nonces = new Set(Array.from({ length: 1000 }, makeNonce));
    expect(nonces.size).toBe(1000);
  });
});

describe('server headers', () => {
  it('parses Server-Timing', () => {
    expect(
      parseServerTiming(
        'db;dur=12.345;desc="PostgreSQL", encode;dur=3.5;desc="arrow encode", compress;dur=0.750;desc="HTTP zstd", total;dur=16.900',
      ),
    ).toEqual({ db: 12.345, encode: 3.5, compress: 0.75, total: 16.9 });
    expect(parseServerTiming(null)).toEqual({});
  });

  it('reads X-* headers', () => {
    const headers = new Headers({
      'x-db-ms': '12.5',
      'x-encode-ms': '3.25',
      'x-compress-ms': '0.5',
      'x-total-ms': '16',
      'x-row-count': '1000',
      'x-column-count': '20',
      'x-last-id': '1000',
      'x-body-bytes': '123456',
      'x-encoded-bytes': '23456',
      'x-format': 'arrow',
      'x-dataset': 'order_lines',
      'x-cache': 'miss',
      'content-encoding': 'zstd',
      'content-length': '23456',
    });
    expect(readServerHeaders(headers)).toEqual({
      dbMs: 12.5,
      encodeMs: 3.25,
      compressMs: 0.5,
      totalMs: 16,
      rowCount: 1000,
      columnCount: 20,
      lastId: 1000,
      bodyBytes: 123456,
      encodedBytes: 23456,
      format: 'arrow',
      dataset: 'order_lines',
      cache: 'miss',
      contentEncoding: 'zstd',
      contentLength: 23456,
    });
  });

  it('falls back to Server-Timing when X-* headers are not exposed', () => {
    const parsed = readServerHeaders(new Headers({ 'server-timing': 'db;dur=1, encode;dur=2, compress;dur=0.25, total;dur=3.5' }));
    expect(parsed).toMatchObject({
      dbMs: 1,
      encodeMs: 2,
      compressMs: 0.25,
      totalMs: 3.5,
      rowCount: null,
      encodedBytes: null,
      contentEncoding: null,
    });
  });
});
