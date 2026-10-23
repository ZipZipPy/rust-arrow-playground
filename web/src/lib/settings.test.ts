import { describe, expect, it } from 'vitest';
import { column, DAILY_SALES, makeDataset, makeInfo, ORDER_LINES } from '../test/serverInfo';
import {
  activeFormat,
  buildRequest,
  columnGroups,
  columnsForGroup,
  columnsKey,
  DEFAULT_SETTINGS,
  fitToServer,
  offsetMode,
  randomOffset,
  resolveDataset,
  sanitizeSettings,
  type Settings,
  switchDataset,
  variantLabel,
} from './settings';
import { readStored, writeStored } from './storage';

const catalog = [column('id', 'numeric'), column('total', 'numeric'), column('country', 'text')];
const settings = (overrides: Partial<Settings>): Settings => ({ ...DEFAULT_SETTINGS, ...overrides });

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
      useScope: 'everything',
      dataset: 42,
    });
    expect(s).toMatchObject({
      useArrow: false,
      rows: 3,
      iterations: 20,
      arrowBatchSize: 1024,
      ipcCompression: 'none',
      parquetCompression: 'brotli',
      columns: null,
      useScope: 'all',
      dataset: '',
    });
  });

  it('defaults to every value and the server’s default dataset', () => {
    expect(DEFAULT_SETTINGS.useScope).toBe('all');
    expect(DEFAULT_SETTINGS.dataset).toBe('');
  });

  it('keeps the dataset, the touch scope and other datasets’ choices', () => {
    const s = sanitizeSettings({
      dataset: 'order_lines',
      useScope: 'column',
      otherDatasets: { orders: { columns: ['id'], offset: 5.4 }, daily_sales: { columns: 'x', offset: -3 }, bad: 7 },
    });
    expect(s.dataset).toBe('order_lines');
    expect(s.useScope).toBe('column');
    expect(s.otherDatasets).toEqual({ orders: { columns: ['id'], offset: 5 }, daily_sales: { columns: null, offset: 0 } });
  });

  it('survives a localStorage round trip', () => {
    const store = new Map<string, string>();
    const storage = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => store.set(k, v) };
    const previous = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
    Object.defineProperty(globalThis, 'localStorage', { value: storage, configurable: true });
    try {
      const chosen = switchDataset(settings({ dataset: 'orders', columns: ['id'], offset: 7 }), 'order_lines');
      writeStored('k', chosen);
      expect(readStored('k', sanitizeSettings, DEFAULT_SETTINGS)).toEqual(chosen);
    } finally {
      if (previous) Object.defineProperty(globalThis, 'localStorage', previous);
      else Reflect.deleteProperty(globalThis, 'localStorage');
    }
  });
});

describe('datasets', () => {
  const info = makeInfo();

  it('uses the server’s default_dataset until one is picked, keeping earlier columns', () => {
    // Settings saved before datasets existed: columns of the orders table, no dataset.
    const s = resolveDataset(settings({ columns: ['id', 'total'], offset: 10 }), info);
    expect(s).toMatchObject({ dataset: 'orders', columns: ['id', 'total'], offset: 10 });
    expect(resolveDataset(settings({}), makeInfo({ default_dataset: 'daily_sales' })).dataset).toBe('daily_sales');
  });

  it('falls back to default_dataset when the stored one is gone', () => {
    const s = resolveDataset(settings({ dataset: 'old_view', columns: ['x'], offset: 99 }), info);
    expect(s).toMatchObject({ dataset: 'orders', columns: null, offset: 0 });
    // The first dataset when even the default is missing.
    expect(resolveDataset(settings({ dataset: 'old_view' }), makeInfo({ default_dataset: 'nope' })).dataset).toBe('orders');
    expect(resolveDataset(settings({ dataset: 'order_lines' }), info).dataset).toBe('order_lines');
  });

  it('remembers columns and start id per dataset', () => {
    const orders = settings({ dataset: 'orders', columns: ['id'], offset: 500 });
    const lines = switchDataset(orders, 'order_lines');
    expect(lines).toMatchObject({ dataset: 'order_lines', columns: null, offset: 0 });
    const back = switchDataset({ ...lines, columns: ['line_id'], offset: 40 }, 'orders');
    expect(back).toMatchObject({ dataset: 'orders', columns: ['id'], offset: 500 });
    expect(switchDataset(back, 'order_lines')).toMatchObject({ columns: ['line_id'], offset: 40 });
    expect(switchDataset(back, 'orders')).toBe(back);
  });

  it('fits columns to the selected dataset’s catalog', () => {
    const s = fitToServer(settings({ dataset: 'order_lines', columns: ['line_id', 'id', 'line_total'] }), info);
    expect(s.columns).toEqual(['line_id', 'line_total']);
    // A selection naming none of these columns shows all of them rather than none.
    expect(fitToServer(settings({ dataset: 'order_lines', columns: ['id'] }), info).columns).toBeNull();
    // An empty selection stays empty, so the UI can ask for a column.
    expect(fitToServer(settings({ dataset: 'order_lines', columns: [] }), info).columns).toEqual([]);
  });

  it('derives column presets from the dataset', () => {
    expect(columnGroups(makeDataset().columns)).toEqual(['numeric', 'text', 'time']);
    expect(columnGroups([column('a', 'text'), column('b', 'geo'), column('c', 'numeric')])).toEqual(['numeric', 'text', 'geo']);
    expect(columnsForGroup(ORDER_LINES.columns, 'numeric')).toEqual(['line_id', 'line_total']);
    expect(columnsForGroup(DAILY_SALES.columns, 'time')).toEqual(['day']);
  });

  it('puts the dataset on every request', () => {
    expect(buildRequest(settings({ dataset: 'daily_sales' }), 'arrow', 3, null)).toMatchObject({ dataset: 'daily_sales', offset: 3 });
  });
});

describe('fitToServer', () => {
  const info = makeInfo({ max_rows: 50_000, datasets: [makeDataset({ columns: catalog })] });

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
    expect(offsetMode({ randomizeOffset: true })).toBe('random');
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

  it('keeps random offsets inside the key range', () => {
    expect(randomOffset({ max_key: 1000, row_estimate: 1000 }, 100, () => 0.999999)).toBe(900);
    expect(randomOffset({ max_key: 50, row_estimate: 50 }, 100, () => 0.7)).toBe(0);
    expect(randomOffset({ max_key: null, row_estimate: null }, 100, () => 0.7)).toBe(0);
  });

  it('scales the span by key density for sparse keys', () => {
    // order_lines: about one line id in four exists, so 1,000 rows span about 4,000 keys.
    expect(randomOffset({ max_key: 10_000, row_estimate: 2_500 }, 1_000, () => 0.999999)).toBe(6_000);
    expect(randomOffset({ max_key: 1000, row_estimate: null }, 100, () => 0.999999)).toBe(900);
  });
});
