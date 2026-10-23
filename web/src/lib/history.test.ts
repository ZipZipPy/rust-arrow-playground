import { describe, expect, it } from 'vitest';
import { makeMeasurement } from '../test/measurements';
import { MAX_HISTORY, parseHistory, prependHistory } from './history';

describe('parseHistory', () => {
  it('keeps current measurements as they are', () => {
    const m = makeMeasurement({ dataset: 'order_lines', useScope: 'all', offsetMode: 'random', rowCache: true, compressMs: 2 });
    expect(parseHistory(JSON.parse(JSON.stringify([m])))).toEqual([m]);
  });

  it('fills in fields that measurements saved before datasets lack', () => {
    const { dataset, offsetMode, rowCache, compressMs, useScope, ...old } = makeMeasurement({ cache: 'hit' });
    const [upgraded] = parseHistory([{ ...old, useColumn: 'total' }]);
    expect(upgraded).toMatchObject({
      dataset: 'orders',
      offsetMode: 'fixed',
      rowCache: true,
      compressMs: null,
      useScope: 'column',
      useColumn: 'total',
    });
    expect(parseHistory([{ ...old, cache: 'off' }])[0]?.rowCache).toBe(false);
  });

  it('drops malformed entries and caps the length', () => {
    expect(parseHistory('nope')).toEqual([]);
    expect(parseHistory([{ format: 'xml' }, null, makeMeasurement()])).toHaveLength(1);
    const many = Array.from({ length: MAX_HISTORY + 5 }, () => makeMeasurement());
    expect(parseHistory(many)).toHaveLength(MAX_HISTORY);
    expect(prependHistory(many.slice(0, MAX_HISTORY), makeMeasurement())).toHaveLength(MAX_HISTORY);
  });
});
