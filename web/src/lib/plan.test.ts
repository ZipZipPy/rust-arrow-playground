import { describe, expect, it } from 'vitest';
import { makeDataset } from '../test/serverInfo';
import { narrowestKeyRange, planCompare, planCompareServers, planRun, type ServerTarget } from './plan';
import { DEFAULT_SETTINGS, type Settings } from './settings';

const dataset = makeDataset({ columns: makeDataset().columns.filter((c) => c.name !== 'total') });
const settings = (overrides: Partial<Settings>): Settings => ({ ...DEFAULT_SETTINGS, dataset: 'orders', ...overrides });
const rolls = (...values: number[]) => () => values.shift()!;

describe('planRun', () => {
  it('runs the active format N times after an optional warm-up', () => {
    const plan = planRun(settings({ useArrow: true, arrowFlavor: 'parquet', iterations: 3, warmup: true, rowGroupSize: 4096 }), dataset);
    expect(plan.map((p) => [p.request.format, p.role])).toEqual([
      ['parquet', 'warmup'],
      ['parquet', 'measure'],
      ['parquet', 'measure'],
      ['parquet', 'measure'],
    ]);
    expect(plan[1]?.request.batchSize).toBe(4096);
  });

  it('uses JSON when Arrow is off, and omits the columns param for all columns', () => {
    const plan = planRun(settings({ useArrow: false, iterations: 1, warmup: false }), dataset);
    expect(plan).toHaveLength(1);
    expect(plan[0]?.request).toMatchObject({ format: 'json', columns: null, dataset: 'orders' });
  });

  it('passes selected columns in catalog order', () => {
    const plan = planRun(settings({ columns: ['created_at', 'id'], iterations: 1, warmup: false }), dataset);
    expect(plan[0]?.request.columns).toEqual(['id', 'created_at']);
  });

  it('randomizes the offset within the key range', () => {
    const plan = planRun(settings({ randomizeOffset: true, rows: 1000, iterations: 3, warmup: false }), dataset, rolls(0, 0.5, 0.999999));
    expect(plan.map((p) => p.request.offset)).toEqual([0, 499_500, 999_000]);
  });

  it('gives the warm-up its own random offset', () => {
    const plan = planRun(settings({ randomizeOffset: true, rows: 1000, iterations: 1, warmup: true }), dataset, rolls(0.1, 0.2));
    expect(plan.map((p) => [p.role, p.request.offset])).toEqual([
      ['warmup', 99_900],
      ['measure', 199_800],
    ]);
  });

  it('never primes the cache', () => {
    expect(planRun(settings({ cache: true, warmup: false }), dataset).every((p) => p.role === 'measure')).toBe(true);
  });
});

describe('planCompare', () => {
  it('interleaves formats and rotates the starting format each round', () => {
    const plan = planCompare(settings({ iterations: 3, warmup: false }), dataset);
    expect(plan.map((p) => p.request.format)).toEqual([
      'json', 'arrow', 'parquet',
      'arrow', 'parquet', 'json',
      'parquet', 'json', 'arrow',
    ]);
    expect(plan.every((p) => p.role === 'measure' && p.request.dataset === 'orders')).toBe(true);
  });

  it('warms up every format once at its own offset and shares the offset within a round', () => {
    const plan = planCompare(settings({ iterations: 2, warmup: true, randomizeOffset: true, rows: 1000 }), dataset, rolls(0.3, 0.1, 0.2));
    const warmups = plan.filter((p) => p.role === 'warmup');
    expect(warmups.map((p) => p.request.format)).toEqual(['json', 'arrow', 'parquet']);
    const round0 = plan.filter((p) => p.round === 0).map((p) => p.request.offset);
    const round1 = plan.filter((p) => p.round === 1).map((p) => p.request.offset);
    expect(new Set(round0).size).toBe(1);
    expect(new Set(round1).size).toBe(1);
    expect(new Set([warmups[0]!.request.offset, round0[0], round1[0]]).size).toBe(3);
  });

  it('applies per-format options', () => {
    const plan = planCompare(
      settings({ iterations: 1, warmup: false, arrowBatchSize: 1024, rowGroupSize: 262_144, ipcCompression: 'lz4' }),
      dataset,
    );
    const byFormat = Object.fromEntries(plan.map((p) => [p.request.format, p.request]));
    expect(byFormat.arrow?.batchSize).toBe(1024);
    expect(byFormat.arrow?.ipcCompression).toBe('lz4');
    expect(byFormat.parquet?.batchSize).toBe(262_144);
  });

  describe('with the server row cache on', () => {
    it('loads each round’s rows before the round, so every measured request is a hit', () => {
      const plan = planCompare(
        settings({ cache: true, iterations: 2, warmup: false, randomizeOffset: true, rows: 1000 }),
        dataset,
        rolls(0.1, 0.2),
      );
      expect(plan.map((p) => [p.role, p.round])).toEqual([
        ['prime', 0], ['measure', 0], ['measure', 0], ['measure', 0],
        ['prime', 1], ['measure', 1], ['measure', 1], ['measure', 1],
      ]);
      for (const round of [0, 1]) {
        const requests = plan.filter((p) => p.round === round).map((p) => p.request);
        expect(new Set(requests.map((r) => `${r.dataset}|${r.offset}|${r.rows}|${r.columns}|${r.cache}`)).size).toBe(1);
      }
    });

    it('loads rows only once when every round reads the same rows', () => {
      const fixed = planCompare(settings({ cache: true, iterations: 3, warmup: false, offset: 7 }), dataset);
      expect(fixed.filter((p) => p.role === 'prime').map((p) => p.round)).toEqual([0]);
      // The warm-up already fetched the fixed offset.
      const warm = planCompare(settings({ cache: true, iterations: 3, warmup: true, offset: 7 }), dataset);
      expect(warm.some((p) => p.role === 'prime')).toBe(false);
    });

    it('does not prime when the cache is off', () => {
      const plan = planCompare(settings({ cache: false, iterations: 2, warmup: false, randomizeOffset: true }), dataset);
      expect(plan.some((p) => p.role === 'prime')).toBe(false);
    });
  });
});

describe('planCompareServers', () => {
  const near: ServerTarget = { server: { base: '', label: 'This server', region: null }, dataset };
  const far: ServerTarget = {
    server: { base: 'https://far.run.app', label: 'us-central1', region: 'us-central1' },
    dataset: { ...dataset, max_key: 50_000, row_estimate: 50_000 },
  };
  const third: ServerTarget = { server: { base: 'https://eu.run.app', label: 'europe-west1', region: 'europe-west1' }, dataset };
  const measured = (plan: ReturnType<typeof planCompareServers>) => plan.filter((p) => p.role === 'measure');

  it('visits every server each round in rotating order, with every format in rotating order', () => {
    const plan = planCompareServers(settings({ iterations: 3, warmup: false }), [near, far, third]);
    const order = (round: number) => {
      const requests = measured(plan).filter((p) => p.round === round);
      return requests.map((p) => `${p.server?.label}:${p.request.format}`);
    };
    expect(order(0)).toEqual([
      'This server:json', 'This server:arrow', 'This server:parquet',
      'us-central1:json', 'us-central1:arrow', 'us-central1:parquet',
      'europe-west1:json', 'europe-west1:arrow', 'europe-west1:parquet',
    ]);
    expect(order(1)).toEqual([
      'us-central1:arrow', 'us-central1:parquet', 'us-central1:json',
      'europe-west1:arrow', 'europe-west1:parquet', 'europe-west1:json',
      'This server:arrow', 'This server:parquet', 'This server:json',
    ]);
    expect(order(2).map((o) => o.split(':')[0])).toEqual([
      'europe-west1', 'europe-west1', 'europe-west1',
      'This server', 'This server', 'This server',
      'us-central1', 'us-central1', 'us-central1',
    ]);
  });

  it('sends every request to a named server and always through the row cache', () => {
    const plan = planCompareServers(settings({ cache: false, iterations: 2, warmup: true }), [near, far]);
    expect(plan.every((p) => p.server !== undefined && p.request.cache)).toBe(true);
    expect(measured(plan)).toHaveLength(2 * 2 * 3);
  });

  it('primes each server before its part of every round whose rows it has not fetched yet', () => {
    const plan = planCompareServers(
      settings({ iterations: 2, warmup: false, randomizeOffset: true, rows: 1000 }),
      [near, far],
      rolls(0.1, 0.2),
    );
    expect(plan.map((p) => [p.role, p.round, p.server?.label])).toEqual([
      ['prime', 0, 'This server'], ['measure', 0, 'This server'], ['measure', 0, 'This server'], ['measure', 0, 'This server'],
      ['prime', 0, 'us-central1'], ['measure', 0, 'us-central1'], ['measure', 0, 'us-central1'], ['measure', 0, 'us-central1'],
      ['prime', 1, 'us-central1'], ['measure', 1, 'us-central1'], ['measure', 1, 'us-central1'], ['measure', 1, 'us-central1'],
      ['prime', 1, 'This server'], ['measure', 1, 'This server'], ['measure', 1, 'This server'], ['measure', 1, 'This server'],
    ]);
  });

  it('primes a fixed start id once per server, and not at all after the warm-up', () => {
    const fixed = planCompareServers(settings({ iterations: 3, warmup: false, offset: 7 }), [near, far]);
    expect(fixed.filter((p) => p.role === 'prime').map((p) => [p.round, p.server?.label])).toEqual([
      [0, 'This server'],
      [0, 'us-central1'],
    ]);
    const warm = planCompareServers(settings({ iterations: 3, warmup: true, offset: 7 }), [near, far]);
    expect(warm.some((p) => p.role === 'prime')).toBe(false);
  });

  it('warms up every format on every server first, at one start id of its own', () => {
    const plan = planCompareServers(
      settings({ iterations: 1, warmup: true, randomizeOffset: true, rows: 1000 }),
      [near, far],
      rolls(0.5, 0.25),
    );
    const warmups = plan.filter((p) => p.role === 'warmup');
    expect(plan.slice(0, 6)).toEqual(warmups);
    expect(warmups.map((p) => `${p.server?.label}:${p.request.format}`)).toEqual([
      'This server:json', 'This server:arrow', 'This server:parquet',
      'us-central1:json', 'us-central1:arrow', 'us-central1:parquet',
    ]);
    expect(new Set(warmups.map((p) => p.request.offset)).size).toBe(1);
    expect(measured(plan)[0]?.request.offset).not.toBe(warmups[0]?.request.offset);
  });

  it('reads the same rows on every server and in every format within a round', () => {
    const plan = planCompareServers(
      settings({ iterations: 3, warmup: true, randomizeOffset: true, rows: 1000, columns: ['id', 'country'] }),
      [near, far, third],
      rolls(0.9, 0.1, 0.5, 0.7),
    );
    for (const round of [-1, 0, 1, 2]) {
      const requests = plan.filter((p) => p.round === round).map((p) => p.request);
      expect(new Set(requests.map((r) => `${r.dataset}|${r.offset}|${r.rows}|${r.columns}|${r.cache}`)).size).toBe(1);
    }
  });

  it('draws random start ids that leave rows on the server with the fewest keys', () => {
    const plan = planCompareServers(
      settings({ iterations: 2, warmup: true, randomizeOffset: true, rows: 1000 }),
      [near, far],
      rolls(0.999999, 0.999999, 0),
    );
    const offsets = plan.map((p) => p.request.offset);
    expect(Math.max(...offsets)).toBe(49_000);
    expect(Math.min(...offsets)).toBe(0);
  });

  it('plans nothing without servers', () => {
    expect(planCompareServers(settings({}), [])).toEqual([]);
  });
});

describe('narrowestKeyRange', () => {
  it('takes the dataset with the smallest max key, counting an unknown one as empty', () => {
    const big = { ...dataset, max_key: 10_000_005, row_estimate: 2_500_000 };
    const small = { ...dataset, max_key: 500_005, row_estimate: 125_000 };
    expect(narrowestKeyRange([big, small])).toEqual({ max_key: 500_005, row_estimate: 125_000 });
    expect(narrowestKeyRange([big, { ...dataset, max_key: null, row_estimate: null }])).toEqual({ max_key: null, row_estimate: null });
    expect(narrowestKeyRange([])).toEqual({ max_key: null, row_estimate: null });
  });
});
