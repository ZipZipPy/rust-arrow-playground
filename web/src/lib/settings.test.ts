import { describe, expect, it } from 'vitest';
import {
  activeFormat,
  columnsForGroup,
  columnsKey,
  DEFAULT_SETTINGS,
  fitToServer,
  randomOffset,
  sanitizeSettings,
  variantLabel,
} from './settings';
import type { ApiInfo, ColumnInfo } from './types';

const column = (name: string, group: string): ColumnInfo => ({
  name, group, pg_type: 'x', arrow_type: 'Int64', nullable: false, dictionary: false,
});
const catalog = [column('id', 'numeric'), column('total', 'numeric'), column('country', 'text')];

describe('sanitizeSettings', () => {
  it('returns defaults for garbage', () => {
    expect(sanitizeSettings(null)).toEqual(DEFAULT_SETTINGS);
    expect(sanitizeSettings('nope')).toEqual(DEFAULT_SETTINGS);
  });

  it('keeps valid fields, clamps numbers and rejects unknown enum values', () => {
    const s = sanitizeSettings({
      useArrow: false,
      rows: 2.6,
      iterations: 99,
      arrowBatchSize: 10,
      ipcCompression: 'snappy',
      parquetCompression: 'brotli',
      columns: ['id', 3],
    });
    expect(s).toMatchObject({
      useArrow: false,
      rows: 3,
      iterations: 20,
      arrowBatchSize: 1024,
      ipcCompression: 'none',
      parquetCompression: 'brotli',
      columns: null,
    });
  });
});

describe('fitToServer', () => {
  const info = { max_rows: 50_000, columns: catalog } as ApiInfo;

  it('caps rows at max_rows and drops unknown columns', () => {
    const s = fitToServer({ ...DEFAULT_SETTINGS, rows: 100_000, columns: ['id', 'gone'] }, info);
    expect(s.rows).toBe(50_000);
    expect(s.columns).toEqual(['id']);
  });

  it('collapses a full selection to null (all)', () => {
    expect(fitToServer({ ...DEFAULT_SETTINGS, columns: ['country', 'id', 'total'] }, info).columns).toBeNull();
  });
});

describe('helpers', () => {
  it('derives the active format from the switch', () => {
    expect(activeFormat({ ...DEFAULT_SETTINGS, useArrow: false, arrowFlavor: 'parquet' })).toBe('json');
    expect(activeFormat({ ...DEFAULT_SETTINGS, useArrow: true, arrowFlavor: 'parquet' })).toBe('parquet');
  });

  it('lists columns for presets', () => {
    expect(columnsForGroup(catalog, 'numeric')).toEqual(['id', 'total']);
    expect(columnsForGroup(catalog, null)).toBeNull();
  });

  it('labels variants', () => {
    const opts = { ipcCompression: 'none', dictionary: false, parquetCompression: 'snappy' } as const;
    expect(variantLabel({ ...opts, format: 'json' })).toBe('JSON');
    expect(variantLabel({ ...opts, format: 'arrow' })).toBe('Arrow IPC');
    expect(variantLabel({ ...opts, format: 'arrow', ipcCompression: 'zstd', dictionary: true })).toBe('Arrow IPC · zstd · dict');
    expect(variantLabel({ ...opts, format: 'parquet' })).toBe('Parquet · snappy');
  });

  it('keys column selections', () => {
    expect(columnsKey(null)).toBe('all');
    expect(columnsKey(['a', 'b'])).toBe('a,b');
  });

  it('keeps random offsets inside the table', () => {
    expect(randomOffset(1000, 100, () => 0.999999)).toBe(900);
    expect(randomOffset(50, 100, () => 0.7)).toBe(0);
  });
});
