import { DATA_FORMATS, type Measurement, USE_SCOPES } from './types';

export const MAX_HISTORY = 500;

const NUMBER_FIELDS = [
  'id', 'batchId', 'rowsRequested', 'offset', 'rowCount', 'columnCount', 'ttfbMs', 'networkWaitMs',
  'downloadMs', 'decodeMs', 'useMs', 'otherMs', 'endToEndMs', 'payloadBytes', 'useValue',
] as const satisfies readonly (keyof Measurement)[];

const STRING_FIELDS = ['at', 'target', 'variant', 'columnsKey'] as const satisfies readonly (keyof Measurement)[];

function isMeasurement(value: unknown): value is Measurement {
  if (!value || typeof value !== 'object') return false;
  const m = value as Record<string, unknown>;
  return (
    DATA_FORMATS.includes(m.format as Measurement['format']) &&
    NUMBER_FIELDS.every((k) => typeof m[k] === 'number') &&
    STRING_FIELDS.every((k) => typeof m[k] === 'string')
  );
}

/**
 * Fills in fields added after a measurement was saved. Earlier versions only
 * served `orders`, summed one column in the "use" step and did not record the
 * start-id mode or the server's region.
 */
function upgrade(saved: Measurement): Measurement {
  const m = saved as Partial<Measurement> & Measurement;
  return {
    ...m,
    dataset: typeof m.dataset === 'string' ? m.dataset : 'orders',
    offsetMode: m.offsetMode === 'random' ? 'random' : 'fixed',
    rowCache: typeof m.rowCache === 'boolean' ? m.rowCache : m.cache === 'hit' || m.cache === 'miss',
    compressMs: typeof m.compressMs === 'number' ? m.compressMs : null,
    useScope: USE_SCOPES.includes(m.useScope) ? m.useScope : 'column',
    region: typeof m.region === 'string' ? m.region : null,
  };
}

/** Restores history saved by an earlier session, dropping anything malformed. */
export function parseHistory(raw: unknown): Measurement[] {
  return Array.isArray(raw) ? raw.filter(isMeasurement).slice(0, MAX_HISTORY).map(upgrade) : [];
}

/** Newest first, capped. */
export function prependHistory(history: readonly Measurement[], m: Measurement): Measurement[] {
  return [m, ...history].slice(0, MAX_HISTORY);
}
