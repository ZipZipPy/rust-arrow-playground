import { describe, expect, it } from 'vitest';
import { makeDataset } from '../test/serverInfo';
import { planCompare, planRun } from './plan';
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
