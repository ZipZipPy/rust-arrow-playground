import type { ServerHeaders, TimingSource } from './types';

/** The Resource Timing fields we use (a subset of PerformanceResourceTiming). */
export interface ResourceTimingLike {
  startTime: number;
  requestStart: number;
  responseStart: number;
  responseEnd: number;
  transferSize: number;
  encodedBodySize: number;
  decodedBodySize: number;
  serverTiming?: readonly { name: string; duration: number }[];
}

/** `performance.now()` readings taken around one request. */
export interface ClientMarks {
  /** Just before `fetch()`. */
  start: number;
  /** `fetch()` resolved: status and headers are in. */
  headers: number;
  /** `arrayBuffer()` resolved: the whole body is in memory. */
  body: number;
  /** Parsing / Arrow decoding finished. */
  decoded: number;
  /** The "use" pass over the rows finished. */
  used: number;
}

export interface Timings {
  ttfbMs: number;
  networkWaitMs: number;
  downloadMs: number;
  decodeMs: number;
  useMs: number;
  /** End to end minus every other phase: connection setup, queuing, server overhead outside db/encode/compress, JS scheduling. */
  otherMs: number;
  endToEndMs: number;
  timingSource: TimingSource;
}

export interface Sizes {
  wireBytes: number | null;
  transferBytes: number | null;
  payloadBytes: number;
}

export interface ServerTimes {
  dbMs: number | null;
  encodeMs: number | null;
  compressMs: number | null;
  totalMs: number | null;
}

/**
 * Cross-origin entries without `Timing-Allow-Origin` (or entries for requests
 * the browser served specially) report zeros; only trust ordered, non-zero marks.
 */
export function hasNetworkTimings(entry: ResourceTimingLike | null): entry is ResourceTimingLike {
  return (
    entry !== null &&
    entry.requestStart > 0 &&
    entry.responseStart >= entry.requestStart &&
    entry.responseEnd >= entry.responseStart
  );
}

const nonNegative = (value: number) => Math.max(0, value);

/** Server phases from headers, falling back to the entry's `serverTiming`. */
export function serverTimes(headers: ServerHeaders, entry: ResourceTimingLike | null): ServerTimes {
  const fromEntry = (name: string) => entry?.serverTiming?.find((t) => t.name === name)?.duration ?? null;
  return {
    dbMs: headers.dbMs ?? fromEntry('db'),
    encodeMs: headers.encodeMs ?? fromEntry('encode'),
    compressMs: headers.compressMs ?? fromEntry('compress'),
    totalMs: headers.totalMs ?? fromEntry('total'),
  };
}

/**
 * Splits one request into phases. The server compresses the whole body before
 * it sends the first byte (and reports that time), so time to first byte
 * covers db + encode + compress, and download is the transfer alone (plus the
 * browser decompressing the body as it streams in).
 */
export function computeTimings(marks: ClientMarks, entry: ResourceTimingLike | null, server: ServerTimes): Timings {
  const precise = hasNetworkTimings(entry);
  const ttfbMs = precise ? entry.responseStart - entry.requestStart : marks.headers - marks.start;
  const downloadMs = precise ? entry.responseEnd - entry.responseStart : marks.body - marks.headers;
  const serverWork = (server.dbMs ?? 0) + (server.encodeMs ?? 0) + (server.compressMs ?? 0);
  const networkWaitMs = nonNegative(ttfbMs - (server.totalMs ?? serverWork));
  const decodeMs = marks.decoded - marks.body;
  const useMs = marks.used - marks.decoded;
  const endToEndMs = marks.used - marks.start;
  const accounted = serverWork + networkWaitMs + downloadMs + decodeMs + useMs;
  return {
    ttfbMs,
    networkWaitMs,
    downloadMs,
    decodeMs,
    useMs,
    otherMs: nonNegative(endToEndMs - accounted),
    endToEndMs,
    timingSource: precise ? 'resource-timing' : 'fetch',
  };
}

/**
 * Wire bytes are the body as it crossed the network (after HTTP compression);
 * payload bytes are the body as the server produced it. Without Resource
 * Timing the server's X-Encoded-Bytes says what it sent.
 */
export function computeSizes(entry: ResourceTimingLike | null, headers: ServerHeaders, bodyLength: number): Sizes {
  const encoded = entry && entry.encodedBodySize > 0 ? entry.encodedBodySize : null;
  const fallbackWire = headers.encodedBytes ?? (headers.contentEncoding ? headers.contentLength : bodyLength);
  return {
    wireBytes: encoded ?? fallbackWire,
    transferBytes: entry && entry.transferSize > 0 ? entry.transferSize : null,
    payloadBytes: headers.bodyBytes ?? (entry && entry.decodedBodySize > 0 ? entry.decodedBodySize : bodyLength),
  };
}
