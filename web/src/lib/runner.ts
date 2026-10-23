import { targetLabel } from './api';
import { columnsKey, variantLabel } from './settings';
import { errorMessage } from './errors';
import { fetchUnmeasured, type MeasuredRequest, measureRequest, resetResourceTimings } from './measure';
import { type ParquetReader, pickUseColumn, previewRows, useColumns } from './decode';
import { loadParquet } from './parquet';
import type { PlannedRequest, PlanServer } from './plan';
import { serverName } from './servers';
import type { ColumnInfo, DataRequest, Measurement, OffsetMode, PreviewTable, UseScope } from './types';

export interface LatestPreview {
  dataset: string;
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
  /** Where requests go unless the plan names a server for them. */
  base: string;
  target: string;
  region: string | null;
  kind: Measurement['kind'];
  batchId: number;
  plan: PlannedRequest[];
  /** The selected columns, in catalog order. */
  columns: readonly ColumnInfo[];
  useScope: UseScope;
  offsetMode: OffsetMode;
  signal: AbortSignal;
  onProgress(progress: Progress): void;
  onMeasurement(measurement: Measurement): void;
  onPreview(preview: LatestPreview): void;
}

type MeasurementContext = Pick<
  Measurement,
  'batchId' | 'kind' | 'target' | 'region' | 'offsetMode' | 'useScope' | 'useColumn'
>;

let nextMeasurementId = Date.now();

export function toMeasurement(result: MeasuredRequest, request: DataRequest, context: MeasurementContext): Measurement {
  nextMeasurementId += 1;
  return {
    id: nextMeasurementId,
    ...context,
    at: new Date().toISOString(),
    dataset: request.dataset,
    format: request.format,
    variant: variantLabel(request),
    rowsRequested: request.rows,
    offset: request.offset,
    columnsKey: columnsKey(request.columns),
    rowCount: result.rowCount,
    columnCount: result.columnCount,
    httpCompression: request.httpCompression,
    contentEncoding: result.headers.contentEncoding,
    rowCache: request.cache,
    cache: result.headers.cache,
    dbMs: result.dbMs,
    encodeMs: result.encodeMs,
    compressMs: result.compressMs,
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

/** Runs one request of a plan; when the plan spans several servers, a failure names the server. */
async function onServer<T>(server: PlanServer | undefined, signal: AbortSignal, request: () => Promise<T>): Promise<T> {
  try {
    return await request();
  } catch (e) {
    if (!server || signal.aborted) throw e;
    throw new Error(`${serverName(server.label, server.base)}: ${errorMessage(e)}`, { cause: e });
  }
}

/**
 * Runs a plan sequentially. Warm-up requests are measured but not reported;
 * cache-priming requests are only fetched. A request that names its server
 * goes there and is reported with that server's target and region. Stops at
 * the first error (which is rethrown, naming the server if the request had
 * one) or when `signal` aborts.
 */
export async function executePlan(opts: ExecuteOptions): Promise<void> {
  const { plan, signal } = opts;
  // Load parquet-wasm up front so its download and compile never land in a timing.
  const parquet: ParquetReader | null = plan.some((p) => p.request.format === 'parquet') ? await loadParquet() : null;
  const used = useColumns(opts.useScope, opts.columns);
  const context: MeasurementContext = {
    batchId: opts.batchId,
    kind: opts.kind,
    target: opts.target,
    region: opts.region,
    offsetMode: opts.offsetMode,
    useScope: opts.useScope,
    useColumn: opts.useScope === 'column' ? pickUseColumn(opts.columns) : null,
  };
  resetResourceTimings();

  for (const [index, planned] of plan.entries()) {
    signal.throwIfAborted();
    opts.onProgress({ done: index, total: plan.length, current: planned });
    await settle();
    signal.throwIfAborted();

    const { server } = planned;
    const base = server?.base ?? opts.base;
    if (planned.role === 'prime') {
      await onServer(server, signal, () => fetchUnmeasured(base, planned.request, signal));
      continue;
    }
    const result = await onServer(server, signal, () =>
      measureRequest({ base, request: planned.request, useColumns: used, parquet, signal }),
    );
    if (planned.role === 'warmup') continue;

    const where = server ? { target: targetLabel(server.base), region: server.region } : {};
    opts.onMeasurement(toMeasurement(result, planned.request, { ...context, ...where }));
    opts.onPreview({
      dataset: planned.request.dataset,
      variant: variantLabel(planned.request),
      rowCount: result.rowCount,
      table: previewRows(result.decoded),
    });
  }
}
