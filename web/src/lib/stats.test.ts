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
  summarizeByServer,
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
    const m = makeMeasurement({
      dbMs: null,
      encodeMs: 2,
      compressMs: null,
      networkWaitMs: 3,
      downloadMs: 4,
      decodeMs: 5,
      useMs: 6,
      otherMs: 1,
    });
    expect(phaseTimes(m)).toEqual({ db: 0, encode: 2, compress: 0, network: 3, download: 4, decode: 5, use: 6, other: 1 });
    expect(phaseTimes(makeMeasurement({ compressMs: 7 })).compress).toBe(7);
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
  it('finds the latest batch of the other family with the same settings', () => {
    const history = [
      makeMeasurement({ format: 'arrow', variant: 'Arrow IPC', batchId: 3, endToEndMs: 10 }),
      makeMeasurement({ format: 'json', batchId: 2, rowsRequested: 5, endToEndMs: 999 }),
      makeMeasurement({ format: 'json', batchId: 1, endToEndMs: 100 }),
      makeMeasurement({ format: 'json', batchId: 1, endToEndMs: 300 }),
      makeMeasurement({ format: 'arrow', variant: 'Arrow IPC', batchId: 1, endToEndMs: 1 }),
    ];
    const json = counterpartSummary(history, makeMeasurement({ format: 'parquet', variant: 'Parquet · snappy' }));
    expect(json?.differences).toEqual([]);
    expect(json?.summary.variant).toBe('JSON');
    expect(json?.summary.count).toBe(2);
    expect(json?.summary.endToEnd.median).toBe(200);
    const arrow = counterpartSummary(history, makeMeasurement({ format: 'json' }));
    expect(arrow?.summary.endToEnd.median).toBe(10);
    expect(counterpartSummary(history, makeMeasurement({ format: 'json', target: 'svc.run.app' }))).toBeNull();
  });

  it('takes only the matching server’s requests from a Compare servers batch', () => {
    const history = [
      makeMeasurement({ kind: 'servers', format: 'json', batchId: 4, target: 'far.run.app', endToEndMs: 900 }),
      makeMeasurement({ kind: 'servers', format: 'json', batchId: 4, endToEndMs: 100 }),
      makeMeasurement({ kind: 'servers', format: 'json', batchId: 4, target: 'far.run.app', endToEndMs: 700 }),
    ];
    const json = counterpartSummary(history, makeMeasurement({ format: 'arrow', variant: 'Arrow IPC', target: 'far.run.app' }));
    expect(json?.summary.count).toBe(2);
    expect(json?.summary.endToEnd.median).toBe(800);
  });

  it.each([
    ['dataset', { dataset: 'order_lines' }, 'order_lines', 'orders'],
    ['rows', { rowsRequested: 5000 }, '5,000', '1,000'],
    ['columns', { columnsKey: 'id,total' }, 'id, total', 'all'],
    ['HTTP compression', { httpCompression: false }, 'off', 'on'],
    ['row cache', { rowCache: true }, 'on', 'off'],
    ['start id', { offsetMode: 'random' }, 'random', 'fixed'],
    ['touch data', { useScope: 'column' }, 'one column', 'every value'],
  ] as const)('does not pair runs with a different %s, and says what differs', (label, change, current, other) => {
    const history = [makeMeasurement({ format: 'json', batchId: 1 })];
    const result = counterpartSummary(history, makeMeasurement({ format: 'arrow', variant: 'Arrow IPC', batchId: 2, ...change }));
    expect(result?.differences).toEqual([{ label, current, other }]);
  });

  it('prefers an older batch with the same settings over a newer one that differs', () => {
    const history = [
      makeMeasurement({ format: 'json', batchId: 3, httpCompression: false, endToEndMs: 50 }),
      makeMeasurement({ format: 'json', batchId: 2, endToEndMs: 70 }),
    ];
    const result = counterpartSummary(history, makeMeasurement({ format: 'arrow', variant: 'Arrow IPC', batchId: 4 }));
    expect(result?.differences).toEqual([]);
    expect(result?.summary.endToEnd.median).toBe(70);
  });

  it('does not count fixed start ids or HTTP encodings the server picked as differences', () => {
    const history = [makeMeasurement({ format: 'json', offset: 100, contentEncoding: 'zstd', cache: 'miss', rowCache: true })];
    const ref = makeMeasurement({ format: 'arrow', offset: 900, contentEncoding: null, cache: 'hit', rowCache: true });
    expect(counterpartSummary(history, ref)?.differences).toEqual([]);
  });

  it('describes long column selections by count', () => {
    const history = [makeMeasurement({ format: 'json', columnsKey: 'a,b,c,d,e' })];
    const result = counterpartSummary(history, makeMeasurement({ format: 'arrow', columnsKey: 'a,b,c,d' }));
    expect(result?.differences).toEqual([{ label: 'columns', current: '4 picked', other: '5 picked' }]);
  });
});

describe('summarizeGroup', () => {
  it('reports browser work as the median of decode + use', () => {
    const [group] = summarizeByFormat([
      makeMeasurement({ decodeMs: 10, useMs: 1 }),
      makeMeasurement({ decodeMs: 1, useMs: 30 }),
      makeMeasurement({ decodeMs: 2, useMs: 2 }),
    ]);
    expect(group?.decodeMs).toBe(2);
    expect(group?.useMs).toBe(2);
    expect(group?.browserMs).toBe(11);
  });
});

describe('summarizeByServer', () => {
  const at = (target: string, region: string | null, format: 'json' | 'arrow' | 'parquet', endToEndMs: number, wireBytes: number) =>
    makeMeasurement({ kind: 'servers', target, region, format, variant: format, endToEndMs, wireBytes });

  it('groups by server in first-seen order, then by format in JSON, Arrow, Parquet order', () => {
    const items = [
      at('same origin', null, 'parquet', 30, 200),
      at('same origin', null, 'json', 100, 1000),
      at('same origin', null, 'arrow', 20, 400),
      at('far.run.app', 'us-central1', 'arrow', 220, 400),
      at('far.run.app', 'us-central1', 'json', 600, 1000),
      at('same origin', null, 'json', 120, 1000),
      at('far.run.app', 'us-central1', 'parquet', 180, 200),
      at('same origin', null, 'arrow', 30, 400),
      at('same origin', null, 'parquet', 40, 200),
    ];
    const [near, far] = summarizeByServer(items);
    expect(near).toMatchObject({ target: 'same origin', region: null, fastest: 'arrow' });
    expect(near?.formats.map((g) => [g.format, g.count, g.endToEnd.median])).toEqual([
      ['json', 2, 110],
      ['arrow', 2, 25],
      ['parquet', 2, 35],
    ]);
    expect(near?.arrowSpeedup).toBe(4.4);
    expect(near?.arrowSize).toBe(0.4);
    expect(far).toMatchObject({ target: 'far.run.app', region: 'us-central1', fastest: 'parquet' });
    expect(far?.arrowSpeedup).toBeCloseTo(600 / 220);
  });

  it('names no fastest format and no Arrow vs JSON ratios until there is something to compare', () => {
    const [only] = summarizeByServer([at('same origin', null, 'parquet', 30, 200)]);
    expect(only).toMatchObject({ fastest: null, arrowSpeedup: null, arrowSize: null });
    const [two] = summarizeByServer([at('same origin', null, 'parquet', 30, 200), at('same origin', null, 'json', 90, 900)]);
    expect(two).toMatchObject({ fastest: 'parquet', arrowSpeedup: null, arrowSize: null });
    expect(summarizeByServer([])).toEqual([]);
  });

  it('reports the rows each server returned', () => {
    const rows = (target: string, rowCount: number) => makeMeasurement({ kind: 'servers', target, rowCount });
    const summaries = summarizeByServer([rows('same origin', 50_000), rows('lb.example.com/eu', 100_000), rows('lb.example.com/us', 100_000)]);
    expect(summaries.map((s) => [s.target, s.rowCount])).toEqual([
      ['same origin', 50_000],
      ['lb.example.com/eu', 100_000],
      ['lb.example.com/us', 100_000],
    ]);
  });
});
