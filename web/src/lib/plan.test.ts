import { describe, expect, it } from 'vitest';
import { planCompare, planRun } from './plan';
import { DEFAULT_SETTINGS, type Settings } from './settings';
import type { ApiInfo, ColumnInfo } from './types';

const column = (name: string, group: string): ColumnInfo => ({
  name, group, pg_type: 'x', arrow_type: 'Int64', nullable: false, dictionary: false,
});

const info: ApiInfo = {
  table: 'orders',
  row_estimate: 1_000_000,
  max_id: 1_000_000,
  table_bytes: 1,
  max_rows: 1_000_000,
  columns: [column('id', 'numeric'), column('country', 'text'), column('created_at', 'time')],
  formats: ['json', 'arrow', 'parquet'],
  ipc_compression: ['none', 'lz4', 'zstd'],
  parquet_compression: ['none', 'snappy'],
  server: { version: '0.1.0', region: null, service: null, revision: null },
};

const settings = (overrides: Partial<Settings>): Settings => ({ ...DEFAULT_SETTINGS, ...overrides });

describe('planRun', () => {
  it('runs the active format N times after an optional warm-up', () => {
    const plan = planRun(settings({ useArrow: true, arrowFlavor: 'parquet', iterations: 3, warmup: true, rowGroupSize: 4096 }), info);
    expect(plan.map((p) => [p.request.format, p.warmup])).toEqual([
      ['parquet', true],
      ['parquet', false],
      ['parquet', false],
      ['parquet', false],
    ]);
    expect(plan[1]?.request.batchSize).toBe(4096);
  });

  it('uses JSON when Arrow is off, and omits the columns param for all columns', () => {
    const plan = planRun(settings({ useArrow: false, iterations: 1, warmup: false }), info);
    expect(plan).toHaveLength(1);
    expect(plan[0]?.request).toMatchObject({ format: 'json', columns: null });
  });

  it('passes selected columns in catalog order', () => {
    const plan = planRun(settings({ columns: ['created_at', 'id'], iterations: 1, warmup: false }), info);
    expect(plan[0]?.request.columns).toEqual(['id', 'created_at']);
  });

  it('randomizes the offset within the table', () => {
    const rolls = [0, 0.5, 0.999999];
    const plan = planRun(settings({ randomizeOffset: true, rows: 1000, iterations: 3, warmup: false }), info, () => rolls.shift()!);
    expect(plan.map((p) => p.request.offset)).toEqual([0, 499_500, 999_000]);
  });
});

describe('planCompare', () => {
  it('interleaves formats and rotates the starting format each round', () => {
    const plan = planCompare(settings({ iterations: 3, warmup: false }), info);
    expect(plan.map((p) => p.request.format)).toEqual([
      'json', 'arrow', 'parquet',
      'arrow', 'parquet', 'json',
      'parquet', 'json', 'arrow',
    ]);
  });

  it('warms up every format once and shares the offset within a round', () => {
    const rolls = [0.1, 0.2];
    const plan = planCompare(settings({ iterations: 2, warmup: true, randomizeOffset: true, rows: 1000 }), info, () => rolls.shift()!);
    expect(plan.filter((p) => p.warmup).map((p) => p.request.format)).toEqual(['json', 'arrow', 'parquet']);
    const round0 = plan.filter((p) => p.round === 0).map((p) => p.request.offset);
    const round1 = plan.filter((p) => p.round === 1).map((p) => p.request.offset);
    expect(new Set(round0).size).toBe(1);
    expect(new Set(round1).size).toBe(1);
    expect(round0[0]).not.toBe(round1[0]);
  });

  it('applies per-format options', () => {
    const plan = planCompare(
      settings({ iterations: 1, warmup: false, arrowBatchSize: 1024, rowGroupSize: 262_144, ipcCompression: 'lz4' }),
      info,
    );
    const byFormat = Object.fromEntries(plan.map((p) => [p.request.format, p.request]));
    expect(byFormat.arrow?.batchSize).toBe(1024);
    expect(byFormat.arrow?.ipcCompression).toBe('lz4');
    expect(byFormat.parquet?.batchSize).toBe(262_144);
  });
});
