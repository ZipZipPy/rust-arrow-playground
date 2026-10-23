import { activeFormat, buildRequest, columnsParam, randomOffset, type Settings } from './settings';
import type { DataFormat, DataRequest, DatasetInfo } from './types';

/**
 * - `warmup` primes connections, database pages and the JIT; not recorded.
 * - `prime` loads a round's rows into the server row cache before the round,
 *   so every measured request in it is a cache hit; not recorded.
 * - `measure` is recorded.
 */
export type PlanRole = 'warmup' | 'prime' | 'measure';

export interface PlannedRequest {
  request: DataRequest;
  role: PlanRole;
  round: number;
}

export const COMPARE_FORMATS: readonly DataFormat[] = ['json', 'arrow', 'parquet'];

/** Any format fills the row cache (its key has no format); Arrow is quick to encode and decode. */
const PRIME_FORMAT: DataFormat = 'arrow';

function nextOffset(s: Settings, dataset: DatasetInfo, random: () => number): number {
  return s.randomizeOffset ? randomOffset(dataset, s.rows, random) : s.offset;
}

/**
 * With random start ids the warm-up draws its own, so round 0 is not the only
 * round whose rows were read just before.
 */
function warmupPlan(s: Settings, dataset: DatasetInfo, formats: readonly DataFormat[], random: () => number) {
  if (!s.warmup) return { requests: [] as PlannedRequest[], offset: null };
  const columns = columnsParam(s, dataset.columns);
  const offset = nextOffset(s, dataset, random);
  const requests = formats.map((format): PlannedRequest => ({
    request: buildRequest(s, format, offset, columns),
    role: 'warmup',
    round: -1,
  }));
  return { requests, offset };
}

/** Run: the current settings, optional warm-up, then N iterations. */
export function planRun(s: Settings, dataset: DatasetInfo, random: () => number = Math.random): PlannedRequest[] {
  const columns = columnsParam(s, dataset.columns);
  const format = activeFormat(s);
  const plan = warmupPlan(s, dataset, [format], random).requests;
  for (let round = 0; round < s.iterations; round++) {
    plan.push({ request: buildRequest(s, format, nextOffset(s, dataset, random), columns), role: 'measure', round });
  }
  return plan;
}

/**
 * Compare: every format N times, interleaved so slow drifts (other traffic,
 * thermal throttling, GC pressure) hit all formats alike. The starting format
 * rotates each round to cancel out ordering effects, and all formats in a
 * round read the same rows. With the server row cache on, a round whose rows
 * were not fetched yet is primed first; otherwise its first format alone
 * would pay the database time.
 */
export function planCompare(s: Settings, dataset: DatasetInfo, random: () => number = Math.random): PlannedRequest[] {
  const columns = columnsParam(s, dataset.columns);
  const warmup = warmupPlan(s, dataset, COMPARE_FORMATS, random);
  const plan = warmup.requests;
  const fetched = new Set<number>(warmup.offset === null ? [] : [warmup.offset]);
  for (let round = 0; round < s.iterations; round++) {
    const offset = nextOffset(s, dataset, random);
    if (s.cache && !fetched.has(offset)) {
      plan.push({ request: buildRequest(s, PRIME_FORMAT, offset, columns), role: 'prime', round });
    }
    fetched.add(offset);
    for (let i = 0; i < COMPARE_FORMATS.length; i++) {
      const format = COMPARE_FORMATS[(i + round) % COMPARE_FORMATS.length]!;
      plan.push({ request: buildRequest(s, format, offset, columns), role: 'measure', round });
    }
  }
  return plan;
}
