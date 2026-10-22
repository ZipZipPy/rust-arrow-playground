import { DATA_FORMATS, type Measurement } from './types';

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

/** Restores history saved by an earlier session, dropping anything malformed. */
export function parseHistory(raw: unknown): Measurement[] {
  return Array.isArray(raw) ? raw.filter(isMeasurement).slice(0, MAX_HISTORY) : [];
}

/** Newest first, capped. */
export function prependHistory(history: readonly Measurement[], m: Measurement): Measurement[] {
  return [m, ...history].slice(0, MAX_HISTORY);
}
