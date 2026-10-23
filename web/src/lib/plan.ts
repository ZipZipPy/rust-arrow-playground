import { activeFormat, buildRequest, columnsParam, randomOffset, type Settings } from './settings';
import type { DataFormat, DataRequest, DatasetInfo } from './types';

/**
 * - `warmup` primes connections, database pages and the JIT; not recorded.
 * - `prime` loads a round's rows into the server row cache before the round,
 *   so every measured request in it is a cache hit; not recorded.
 * - `measure` is recorded.
 */
export type PlanRole = 'warmup' | 'prime' | 'measure';

/** Where a request goes when a plan spans several servers. */
export interface PlanServer {
  base: string;
  /** Short name for the UI: the region, the host, or "This server". */
  label: string;
  region: string | null;
}

export interface PlannedRequest {
  request: DataRequest;
  role: PlanRole;
  round: number;
  /** Set by plans that span several servers; otherwise the request goes to the runner's base. */
  server?: PlanServer;
}

/** A server taking part in Compare servers, with its own entry for the dataset. */
export interface ServerTarget {
  server: PlanServer;
  dataset: DatasetInfo;
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

/**
 * The key range every server has: the dataset entry with the smallest
 * `max_key`, so a random start id leaves rows on all of them. A server that
 * reports no `max_key` counts as empty, which pins random start ids to 0.
 */
export function narrowestKeyRange(datasets: readonly DatasetInfo[]): Pick<DatasetInfo, 'max_key' | 'row_estimate'> {
  const key = (d: DatasetInfo) => d.max_key ?? 0;
  const narrowest = datasets.reduce<DatasetInfo | null>((a, d) => (a === null || key(d) < key(a) ? d : a), null);
  return { max_key: narrowest?.max_key ?? null, row_estimate: narrowest?.row_estimate ?? null };
}

/**
 * Compare servers: Compare's interleaving, run against several servers. Every
 * server warms up first, then each round visits the servers in an order that
 * rotates like the formats do, and every request of a round, on every server
 * and in every format, reads the same rows. The server row cache is always
 * on and each server's rows are primed before it is measured: far servers
 * reach the database across regions, and that query time would bury the
 * network differences this mode is about.
 */
export function planCompareServers(
  s: Settings,
  targets: readonly ServerTarget[],
  random: () => number = Math.random,
): PlannedRequest[] {
  const first = targets[0];
  if (!first) return [];
  const cached: Settings = { ...s, cache: true };
  const dataset = { ...first.dataset, ...narrowestKeyRange(targets.map((t) => t.dataset)) };
  const columns = columnsParam(cached, dataset.columns);
  const warmup = warmupPlan(cached, dataset, COMPARE_FORMATS, random);
  const plan = targets.flatMap(({ server }) => warmup.requests.map((p): PlannedRequest => ({ ...p, server })));
  const warmed = warmup.offset === null ? [] : [warmup.offset];
  const fetched = new Map(targets.map(({ server }) => [server.base, new Set<number>(warmed)]));
  for (let round = 0; round < s.iterations; round++) {
    const offset = nextOffset(cached, dataset, random);
    for (let j = 0; j < targets.length; j++) {
      const { server } = targets[(j + round) % targets.length]!;
      const cachedOffsets = fetched.get(server.base)!;
      if (!cachedOffsets.has(offset)) {
        plan.push({ request: buildRequest(cached, PRIME_FORMAT, offset, columns), role: 'prime', round, server });
        cachedOffsets.add(offset);
      }
      for (let i = 0; i < COMPARE_FORMATS.length; i++) {
        const format = COMPARE_FORMATS[(i + round) % COMPARE_FORMATS.length]!;
        plan.push({ request: buildRequest(cached, format, offset, columns), role: 'measure', round, server });
      }
    }
  }
  return plan;
}
