import { describe, expect, it } from 'vitest';
import { makeMeasurement } from '../test/measurements';
import {
  counterpartSummary,
  median,
  medianOf,
  phaseTimes,
  ratio,
  speedup,
  summarize,
  summarizeByFormat,
} from './stats';

describe('median / summarize', () => {
  it('handles odd, even and empty inputs', () => {
    expect(median([5, 1, 3])).toBe(3);
    expect(median([4, 1, 3, 2])).toBe(2.5);
    expect(median([])).toBeNaN();
  });

  it('does not reorder the input', () => {
    const values = [3, 1, 2];
    median(values);
    expect(values).toEqual([3, 1, 2]);
  });

  it('summarizes min, max, mean', () => {
    expect(summarize([2, 8, 4, 6])).toEqual({ count: 4, median: 5, min: 2, max: 8, mean: 5 });
    expect(summarize([]).count).toBe(0);
  });

  it('ignores missing values in medianOf', () => {
    expect(medianOf([null, 10, 20, null, 30])).toBe(20);
    expect(medianOf([null, null])).toBeNull();
  });
});

describe('ratios', () => {
  it('computes speed-up as baseline / value', () => {
    expect(speedup(300, 100)).toBe(3);
    expect(speedup(100, 400)).toBe(0.25);
    expect(speedup(0, 100)).toBeNull();
  });

  it('computes size ratio as value / baseline', () => {
    expect(ratio(25, 100)).toBe(0.25);
    expect(ratio(null, 100)).toBeNull();
    expect(ratio(10, 0)).toBeNull();
  });
});

describe('phaseTimes', () => {
  it('maps a measurement to stacked-bar phases, treating missing server times as 0', () => {
    const m = makeMeasurement({ dbMs: null, encodeMs: 2, networkWaitMs: 3, downloadMs: 4, decodeMs: 5, useMs: 6, otherMs: 1 });
    expect(phaseTimes(m)).toEqual({ db: 0, encode: 2, network: 3, download: 4, decode: 5, use: 6, other: 1 });
  });
});

describe('summarizeByFormat', () => {
  it('groups in first-seen order and takes medians', () => {
    const items = [
      makeMeasurement({ format: 'json', endToEndMs: 100, wireBytes: 1000, decodeMs: 50 }),
      makeMeasurement({ format: 'arrow', endToEndMs: 20, wireBytes: 300, decodeMs: 2 }),
      makeMeasurement({ format: 'json', endToEndMs: 300, wireBytes: 1000, decodeMs: 70 }),
      makeMeasurement({ format: 'arrow', endToEndMs: 40, wireBytes: null, decodeMs: 4 }),
      makeMeasurement({ format: 'json', endToEndMs: 200, wireBytes: 1200, decodeMs: 60 }),
    ];
    const [json, arrow] = summarizeByFormat(items);
    expect(json?.format).toBe('json');
    expect(json?.count).toBe(3);
    expect(json?.endToEnd).toMatchObject({ median: 200, min: 100, max: 300 });
    expect(json?.wireBytes).toBe(1000);
    expect(json?.decodeMs).toBe(60);
    expect(arrow?.endToEnd.median).toBe(30);
    expect(arrow?.wireBytes).toBe(300);
  });
});

describe('counterpartSummary', () => {
  it('finds the latest batch of the other family with the same shape', () => {
    const history = [
      makeMeasurement({ format: 'arrow', variant: 'Arrow IPC', batchId: 3, endToEndMs: 10 }),
      makeMeasurement({ format: 'json', batchId: 2, rowsRequested: 5, endToEndMs: 999 }),
      makeMeasurement({ format: 'json', batchId: 1, endToEndMs: 100 }),
      makeMeasurement({ format: 'json', batchId: 1, endToEndMs: 300 }),
      makeMeasurement({ format: 'arrow', variant: 'Arrow IPC', batchId: 1, endToEndMs: 1 }),
    ];
    const json = counterpartSummary(history, { format: 'parquet', rowsRequested: 1000, columnsKey: 'all', target: 'same origin' });
    expect(json?.variant).toBe('JSON');
    expect(json?.count).toBe(2);
    expect(json?.endToEnd.median).toBe(200);
    const arrow = counterpartSummary(history, { format: 'json', rowsRequested: 1000, columnsKey: 'all', target: 'same origin' });
    expect(arrow?.endToEnd.median).toBe(10);
    expect(counterpartSummary(history, { format: 'json', rowsRequested: 1000, columnsKey: 'id', target: 'same origin' })).toBeNull();
  });
});
