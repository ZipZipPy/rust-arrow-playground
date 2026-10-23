import type { ApiInfo, BenchReport, DataRequest, ServerHeaders } from './types';

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

/**
 * Normalizes the user-entered API base URL. An empty string means "same
 * origin". A trailing `/api` is dropped because every path already includes it.
 * Throws on anything that is not an http(s) URL.
 */
export function normalizeBaseUrl(raw: string): string {
  const trimmed = raw.trim();
  if (trimmed === '') return '';
  let url: URL;
  try {
    url = new URL(trimmed);
  } catch {
    throw new Error('Enter a full URL such as https://my-service.run.app');
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error('Only http:// and https:// URLs are supported');
  }
  const path = url.pathname.replace(/\/+$/, '').replace(/\/api$/, '');
  return `${url.origin}${path}`;
}

/**
 * Short label for where requests go, used in the header and history: the
 * host, plus the path when the base has one, so two servers behind one host
 * stay apart.
 */
export function targetLabel(base: string): string {
  if (base === '') return 'same origin';
  try {
    const url = new URL(base);
    return `${url.host}${url.pathname.replace(/\/+$/, '')}`;
  } catch {
    return base;
  }
}

/** Absolute URL of an API endpoint. `origin` is only needed for a same-origin base. */
export function apiUrl(
  base: string,
  path: string,
  params?: URLSearchParams,
  origin: string = globalThis.location?.origin ?? 'http://localhost',
): string {
  const url = new URL(`${base}/api/${path}`, origin);
  if (params) url.search = params.toString();
  return url.href;
}

/** Query parameters for `/api/data`, limited to the ones the format uses. */
export function dataParams(req: DataRequest, nonce?: string): URLSearchParams {
  const params = new URLSearchParams();
  params.set('dataset', req.dataset);
  params.set('format', req.format);
  params.set('rows', String(req.rows));
  if (req.offset > 0) params.set('offset', String(req.offset));
  if (req.columns !== null) params.set('columns', req.columns.join(','));
  params.set('http_compression', String(req.httpCompression));
  if (req.cache) params.set('cache', 'true');
  if (req.format === 'arrow') {
    params.set('dictionary', String(req.dictionary));
    params.set('ipc_compression', req.ipcCompression);
    params.set('batch_size', String(req.batchSize));
  } else if (req.format === 'parquet') {
    params.set('parquet_compression', req.parquetCompression);
    params.set('batch_size', String(req.batchSize));
  }
  if (nonce !== undefined) params.set('_', nonce);
  return params;
}

export interface BenchParams {
  dataset: string;
  rows: number;
  offset: number;
  columns: string[] | null;
  cache: boolean;
}

export function benchParams(p: BenchParams, nonce?: string): URLSearchParams {
  const params = new URLSearchParams();
  params.set('dataset', p.dataset);
  params.set('rows', String(p.rows));
  if (p.offset > 0) params.set('offset', String(p.offset));
  if (p.columns !== null) params.set('columns', p.columns.join(','));
  if (p.cache) params.set('cache', 'true');
  if (nonce !== undefined) params.set('_', nonce);
  return params;
}

let nonceCounter = 0;

/** Unique cache-busting value; also makes each Resource Timing entry unique. */
export function makeNonce(): string {
  nonceCounter += 1;
  return `${Date.now().toString(36)}${nonceCounter.toString(36)}${Math.random().toString(36).slice(2, 6)}`;
}

/** Reads the `{"error": "..."}` body the server sends with 4xx/5xx responses. */
export async function errorFromResponse(res: Response): Promise<ApiError> {
  let message = `${res.status} ${res.statusText}`.trim();
  try {
    const body: unknown = await res.json();
    if (body && typeof body === 'object' && 'error' in body && typeof body.error === 'string') {
      message = `${res.status}: ${body.error}`;
    }
  } catch {
    // Not JSON; keep the status line.
  }
  return new ApiError(res.status, message);
}

async function getJson<T>(url: string, signal?: AbortSignal): Promise<T> {
  const res = await fetch(url, { cache: 'no-store', signal });
  if (!res.ok) throw await errorFromResponse(res);
  return (await res.json()) as T;
}

/**
 * Runs `task` with a signal that also aborts after `ms`, and reports that
 * case as "no answer within N s" instead of the browser's abort message.
 */
export async function withTimeout<T>(
  ms: number,
  signal: AbortSignal | undefined,
  task: (signal: AbortSignal) => Promise<T>,
): Promise<T> {
  const timeout = AbortSignal.timeout(ms);
  try {
    return await task(signal ? AbortSignal.any([signal, timeout]) : timeout);
  } catch (e) {
    if (timeout.aborted && !signal?.aborted) throw new Error(`no answer within ${ms / 1000} s`, { cause: e });
    throw e;
  }
}

export function fetchInfo(base: string, signal?: AbortSignal): Promise<ApiInfo> {
  return getJson<ApiInfo>(apiUrl(base, 'info', new URLSearchParams({ _: makeNonce() })), signal);
}

export function fetchBench(base: string, p: BenchParams, signal?: AbortSignal): Promise<BenchReport> {
  return getJson<BenchReport>(apiUrl(base, 'bench', benchParams(p, makeNonce())), signal);
}

function numberHeader(headers: Headers, name: string): number | null {
  const raw = headers.get(name);
  if (raw === null || raw.trim() === '') return null;
  const value = Number(raw);
  return Number.isFinite(value) ? value : null;
}

/** Parses `Server-Timing` into `{name: durationMs}`. */
export function parseServerTiming(header: string | null): Record<string, number> {
  const out: Record<string, number> = {};
  if (!header) return out;
  for (const metric of header.split(',')) {
    const [name, ...params] = metric.split(';').map((s) => s.trim());
    if (!name) continue;
    for (const param of params) {
      const match = /^dur=([\d.]+)$/.exec(param);
      if (match?.[1]) out[name] = Number(match[1]);
    }
  }
  return out;
}

export function readServerHeaders(headers: Headers): ServerHeaders {
  const timing = parseServerTiming(headers.get('server-timing'));
  return {
    dbMs: numberHeader(headers, 'x-db-ms') ?? timing.db ?? null,
    encodeMs: numberHeader(headers, 'x-encode-ms') ?? timing.encode ?? null,
    compressMs: numberHeader(headers, 'x-compress-ms') ?? timing.compress ?? null,
    totalMs: numberHeader(headers, 'x-total-ms') ?? timing.total ?? null,
    rowCount: numberHeader(headers, 'x-row-count'),
    columnCount: numberHeader(headers, 'x-column-count'),
    lastId: numberHeader(headers, 'x-last-id'),
    bodyBytes: numberHeader(headers, 'x-body-bytes'),
    encodedBytes: numberHeader(headers, 'x-encoded-bytes'),
    format: headers.get('x-format'),
    dataset: headers.get('x-dataset'),
    cache: headers.get('x-cache'),
    contentEncoding: headers.get('content-encoding'),
    contentLength: numberHeader(headers, 'content-length'),
  };
}
