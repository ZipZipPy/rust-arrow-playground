import { describe, expect, it } from 'vitest';
import { readServerHeaders } from './api';
import { type ClientMarks, computeSizes, computeTimings, type ResourceTimingLike, serverTimes } from './timing';

const marks: ClientMarks = { start: 1000, headers: 1060, body: 1100, decoded: 1130, used: 1135 };

const entry: ResourceTimingLike = {
  startTime: 1000.5,
  requestStart: 1002,
  responseStart: 1052,
  responseEnd: 1097,
  transferSize: 20_300,
  encodedBodySize: 20_000,
  decodedBodySize: 90_000,
};

const server = { dbMs: 20, encodeMs: 10, totalMs: 32 };

describe('computeTimings', () => {
  it('uses Resource Timing marks when present', () => {
    const t = computeTimings(marks, entry, server);
    expect(t.timingSource).toBe('resource-timing');
    expect(t.ttfbMs).toBe(50);
    expect(t.networkWaitMs).toBe(18);
    expect(t.downloadMs).toBe(45);
    expect(t.decodeMs).toBe(30);
    expect(t.useMs).toBe(5);
    expect(t.endToEndMs).toBe(135);
    // 135 - (20 + 10 + 18 + 45 + 30 + 5)
    expect(t.otherMs).toBe(7);
  });

  it('falls back to fetch marks when the entry is missing', () => {
    const t = computeTimings(marks, null, server);
    expect(t.timingSource).toBe('fetch');
    expect(t.ttfbMs).toBe(60);
    expect(t.downloadMs).toBe(40);
    expect(t.networkWaitMs).toBe(28);
  });

  it('falls back when the entry is zeroed (cross-origin without Timing-Allow-Origin)', () => {
    const zeroed = { ...entry, requestStart: 0, responseStart: 0, transferSize: 0, encodedBodySize: 0, decodedBodySize: 0 };
    expect(computeTimings(marks, zeroed, server).timingSource).toBe('fetch');
  });

  it('never reports negative network wait or other time', () => {
    const t = computeTimings(marks, entry, { dbMs: 80, encodeMs: 40, totalMs: 130 });
    expect(t.networkWaitMs).toBe(0);
    expect(t.otherMs).toBe(0);
  });

  it('works without any server timing', () => {
    const t = computeTimings(marks, entry, { dbMs: null, encodeMs: null, totalMs: null });
    expect(t.networkWaitMs).toBe(50);
  });
});

describe('serverTimes', () => {
  it('prefers headers and falls back to the entry serverTiming', () => {
    const headers = readServerHeaders(new Headers({ 'x-db-ms': '5' }));
    const withTiming = { ...entry, serverTiming: [{ name: 'encode', duration: 7 }, { name: 'total', duration: 13 }] };
    expect(serverTimes(headers, withTiming)).toEqual({ dbMs: 5, encodeMs: 7, totalMs: 13 });
    expect(serverTimes(headers, null)).toEqual({ dbMs: 5, encodeMs: null, totalMs: null });
  });
});

describe('computeSizes', () => {
  const headers = readServerHeaders(
    new Headers({ 'x-body-bytes': '90000', 'content-encoding': 'gzip', 'content-length': '20000' }),
  );

  it('uses encodedBodySize and transferSize from the entry', () => {
    expect(computeSizes(entry, headers, 90_000)).toEqual({ wireBytes: 20_000, transferBytes: 20_300, payloadBytes: 90_000 });
  });

  it('falls back to Content-Length for compressed bodies', () => {
    expect(computeSizes(null, headers, 90_000)).toEqual({ wireBytes: 20_000, transferBytes: null, payloadBytes: 90_000 });
  });

  it('falls back to the body length for uncompressed bodies', () => {
    const plain = readServerHeaders(new Headers());
    expect(computeSizes(null, plain, 1234)).toEqual({ wireBytes: 1234, transferBytes: null, payloadBytes: 1234 });
  });

  it('reports unknown wire size for compressed chunked bodies without an entry', () => {
    const chunked = readServerHeaders(new Headers({ 'content-encoding': 'br' }));
    expect(computeSizes(null, chunked, 5000).wireBytes).toBeNull();
  });
});
