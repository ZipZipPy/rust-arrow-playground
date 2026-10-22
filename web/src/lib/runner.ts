import { columnsKey, variantLabel } from './settings';
import { type MeasuredRequest, measureRequest, resetResourceTimings } from './measure';
import { type ParquetReader, previewRows } from './decode';
import { loadParquet } from './parquet';
import type { PlannedRequest } from './plan';
import type { DataRequest, Measurement, PreviewTable } from './types';

export interface LatestPreview {
  variant: string;
  rowCount: number;
  table: PreviewTable;
}

export interface Progress {
  done: number;
  total: number;
  current: PlannedRequest;
}

export interface ExecuteOptions {
  base: string;
  target: string;
  kind: Measurement['kind'];
  batchId: number;
  plan: PlannedRequest[];
  useColumn: string | null;
  signal: AbortSignal;
  onProgress(progress: Progress): void;
  onMeasurement(measurement: Measurement): void;
  onPreview(preview: LatestPreview): void;
}

let nextMeasurementId = Date.now();

export function toMeasurement(
  result: MeasuredRequest,
  request: DataRequest,
  context: Pick<Measurement, 'batchId' | 'kind' | 'target' | 'useColumn'>,
): Measurement {
  nextMeasurementId += 1;
  return {
    id: nextMeasurementId,
    ...context,
    at: new Date().toISOString(),
    format: request.format,
    variant: variantLabel(request),
    rowsRequested: request.rows,
    offset: request.offset,
    columnsKey: columnsKey(request.columns),
    rowCount: result.rowCount,
    columnCount: result.columnCount,
    httpCompression: request.httpCompression,
    contentEncoding: result.headers.contentEncoding,
    cache: result.headers.cache,
    dbMs: result.dbMs,
    encodeMs: result.encodeMs,
    serverTotalMs: result.serverTotalMs,
    ...result.timings,
    ...result.sizes,
    useValue: result.useValue,
  };
}

const SETTLE_TIMEOUT_MS = 250;

/**
 * Waits until the UI has rendered and painted the previous result. Otherwise
 * React would re-render (history, preview, charts) while the next response is
 * arriving, and that main-thread work would show up as download or "other"
 * time. rAF does not fire in background tabs, hence the timeout.
 */
function settle(): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, SETTLE_TIMEOUT_MS);
    requestAnimationFrame(() =>
      requestAnimationFrame(() =>
        setTimeout(() => {
          clearTimeout(timer);
          resolve();
        }, 0),
      ),
    );
  });
}

/**
 * Runs a plan sequentially. Warm-up requests are measured but not reported.
 * Stops at the first error (which is rethrown) or when `signal` aborts.
 */
export async function executePlan(opts: ExecuteOptions): Promise<void> {
  const { plan, signal } = opts;
  // Load parquet-wasm up front so its download and compile never land in a timing.
  const parquet: ParquetReader | null = plan.some((p) => p.request.format === 'parquet') ? await loadParquet() : null;
  resetResourceTimings();

  for (const [index, planned] of plan.entries()) {
    signal.throwIfAborted();
    opts.onProgress({ done: index, total: plan.length, current: planned });
    await settle();
    signal.throwIfAborted();

    const result = await measureRequest({
      base: opts.base,
      request: planned.request,
      useColumn: opts.useColumn,
      parquet,
      signal,
    });
    if (planned.warmup) continue;

    opts.onMeasurement(
      toMeasurement(result, planned.request, {
        batchId: opts.batchId,
        kind: opts.kind,
        target: opts.target,
        useColumn: opts.useColumn,
      }),
    );
    opts.onPreview({ variant: variantLabel(planned.request), rowCount: result.rowCount, table: previewRows(result.decoded) });
  }
}
