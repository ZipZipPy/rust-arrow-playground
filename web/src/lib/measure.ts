import { apiUrl, dataParams, errorFromResponse, makeNonce, readServerHeaders } from './api';
import { type Decoded, type ParquetReader, decodePayload, rowCount, useValues } from './decode';
import {
  type ClientMarks,
  computeSizes,
  computeTimings,
  hasNetworkTimings,
  type ResourceTimingLike,
  type Sizes,
  serverTimes,
  type Timings,
} from './timing';
import type { DataRequest, ServerHeaders } from './types';

const RESOURCE_BUFFER_SIZE = 1000;
const ENTRY_POLL_MS = 5;
const ENTRY_POLL_ATTEMPTS = 40;

let resourceBufferReady = false;

/**
 * Enlarges the Resource Timing buffer and keeps it from filling up: once full,
 * the browser silently drops new entries and every later request would fall
 * back to the less precise fetch-based timings. Entries are looked up by their
 * unique URL, so clearing old ones is always safe.
 */
export function prepareResourceTimingBuffer(): void {
  if (resourceBufferReady || typeof performance === 'undefined') return;
  resourceBufferReady = true;
  performance.setResourceTimingBufferSize?.(RESOURCE_BUFFER_SIZE);
  performance.addEventListener?.('resourcetimingbufferfull', () => performance.clearResourceTimings());
}

/** Starts a batch of requests with an empty Resource Timing buffer. */
export function resetResourceTimings(): void {
  prepareResourceTimingBuffer();
  performance.clearResourceTimings?.();
}

const delay = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/**
 * The entry is queued once the body has been read, but browsers add it
 * asynchronously, so poll briefly. Runs after timing is done, so the wait is
 * never measured.
 */
export async function findResourceEntry(url: string): Promise<ResourceTimingLike | null> {
  if (typeof performance === 'undefined' || !performance.getEntriesByName) return null;
  for (let attempt = 0; attempt < ENTRY_POLL_ATTEMPTS; attempt++) {
    const entries = performance.getEntriesByName(url, 'resource');
    const entry = entries[entries.length - 1] as PerformanceResourceTiming | undefined;
    if (entry) return entry;
    await delay(ENTRY_POLL_MS);
  }
  return null;
}

export interface MeasuredRequest {
  url: string;
  headers: ServerHeaders;
  timings: Timings;
  sizes: Sizes;
  dbMs: number | null;
  encodeMs: number | null;
  compressMs: number | null;
  serverTotalMs: number | null;
  rowCount: number;
  columnCount: number;
  useValue: number;
  decoded: Decoded;
}

export interface MeasureOptions {
  base: string;
  request: DataRequest;
  /** The columns the "use" step reads (see `useColumns`). */
  useColumns: readonly string[];
  parquet: ParquetReader | null;
  signal?: AbortSignal;
}

/**
 * Fetches one `/api/data` response and times each phase separately:
 * download (`arrayBuffer`), decode (parse / Arrow / Parquet) and use (one
 * read of every value of `useColumns`). Errors from the server are thrown as
 * ApiError.
 */
export async function measureRequest(opts: MeasureOptions): Promise<MeasuredRequest> {
  prepareResourceTimingBuffer();
  const url = apiUrl(opts.base, 'data', dataParams(opts.request, makeNonce()));

  const start = performance.now();
  const response = await fetch(url, { cache: 'no-store', signal: opts.signal });
  const headersAt = performance.now();
  if (!response.ok) throw await errorFromResponse(response);
  const buffer = await response.arrayBuffer();
  const bodyAt = performance.now();
  const bytes = new Uint8Array(buffer);
  const decoded = decodePayload(opts.request.format, bytes, opts.parquet);
  const decodedAt = performance.now();
  const useValue = useValues(decoded, opts.useColumns);
  const usedAt = performance.now();

  const marks: ClientMarks = { start, headers: headersAt, body: bodyAt, decoded: decodedAt, used: usedAt };
  const headers = readServerHeaders(response.headers);
  const entry = await findResourceEntry(url);
  const server = serverTimes(headers, entry);
  const columnCount =
    decoded.kind === 'arrow' ? decoded.table.numCols : (headers.columnCount ?? Object.keys(decoded.rows[0] ?? {}).length);

  return {
    url,
    headers,
    timings: computeTimings(marks, entry, server),
    sizes: computeSizes(entry, headers, bytes.byteLength),
    dbMs: server.dbMs,
    encodeMs: server.encodeMs,
    compressMs: server.compressMs,
    serverTotalMs: server.totalMs,
    rowCount: rowCount(decoded),
    columnCount,
    useValue,
    decoded,
  };
}

/** Fetches and drains a response without timing or decoding it (to fill the server row cache). */
export async function fetchUnmeasured(base: string, request: DataRequest, signal?: AbortSignal): Promise<void> {
  const response = await fetch(apiUrl(base, 'data', dataParams(request, makeNonce())), { cache: 'no-store', signal });
  if (!response.ok) throw await errorFromResponse(response);
  await response.arrayBuffer();
}

export const PING_COUNT = 5;

/** `PING_COUNT` sequential round trips to one server, for its RTT. */
export async function pingSamples(base: string, signal?: AbortSignal): Promise<number[]> {
  const samples: number[] = [];
  for (let i = 0; i < PING_COUNT; i++) samples.push(await pingOnce(base, signal));
  return samples;
}

/** One round-trip sample against `/api/ping` (204, empty body). */
export async function pingOnce(base: string, signal?: AbortSignal): Promise<number> {
  prepareResourceTimingBuffer();
  const url = apiUrl(base, 'ping', new URLSearchParams({ _: makeNonce() }));
  const start = performance.now();
  const response = await fetch(url, { cache: 'no-store', signal });
  if (!response.ok) throw await errorFromResponse(response);
  await response.arrayBuffer();
  const end = performance.now();
  const entry = await findResourceEntry(url);
  // requestStart → responseStart excludes connection setup, which is what RTT means here.
  return hasNetworkTimings(entry) ? entry.responseStart - entry.requestStart : end - start;
}
