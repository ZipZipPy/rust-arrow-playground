import { activeFormat, buildRequest, columnsParam, randomOffset, type Settings } from './settings';
import type { ApiInfo, DataFormat, DataRequest } from './types';

export interface PlannedRequest {
  request: DataRequest;
  /** Warm-up requests prime connections, caches and the JIT; they are not recorded. */
  warmup: boolean;
  round: number;
}

export const COMPARE_FORMATS: readonly DataFormat[] = ['json', 'arrow', 'parquet'];

function offsetsPerRound(s: Settings, info: ApiInfo, rounds: number, random: () => number): number[] {
  return Array.from({ length: rounds }, () => (s.randomizeOffset ? randomOffset(info.max_id, s.rows, random) : s.offset));
}

/** Run: the current settings, optional warm-up, then N iterations. */
export function planRun(s: Settings, info: ApiInfo, random: () => number = Math.random): PlannedRequest[] {
  const columns = columnsParam(s, info.columns);
  const format = activeFormat(s);
  const offsets = offsetsPerRound(s, info, s.iterations, random);
  const plan: PlannedRequest[] = [];
  if (s.warmup) plan.push({ request: buildRequest(s, format, offsets[0]!, columns), warmup: true, round: -1 });
  offsets.forEach((offset, round) => plan.push({ request: buildRequest(s, format, offset, columns), warmup: false, round }));
  return plan;
}

/**
 * Compare: every format N times, interleaved so slow drifts (other traffic,
 * thermal throttling, GC pressure) hit all formats alike. The starting format
 * rotates each round to cancel out ordering effects, and all formats in a
 * round read the same rows.
 */
export function planCompare(s: Settings, info: ApiInfo, random: () => number = Math.random): PlannedRequest[] {
  const columns = columnsParam(s, info.columns);
  const offsets = offsetsPerRound(s, info, s.iterations, random);
  const plan: PlannedRequest[] = [];
  if (s.warmup) {
    for (const format of COMPARE_FORMATS) {
      plan.push({ request: buildRequest(s, format, offsets[0]!, columns), warmup: true, round: -1 });
    }
  }
  offsets.forEach((offset, round) => {
    for (let i = 0; i < COMPARE_FORMATS.length; i++) {
      const format = COMPARE_FORMATS[(i + round) % COMPARE_FORMATS.length]!;
      plan.push({ request: buildRequest(s, format, offset, columns), warmup: false, round });
    }
  });
  return plan;
}
