import { describe, expect, it } from 'vitest';
import { makeMeasurement } from '../test/measurements';
import { toCsv, toMarkdown } from './export';

describe('toCsv', () => {
  it('writes a header and one line per measurement', () => {
    const csv = toCsv([makeMeasurement({ variant: 'Arrow IPC · zstd', dbMs: 1.23456 }), makeMeasurement()]);
    const lines = csv.trimEnd().split('\n');
    expect(lines).toHaveLength(3);
    expect(lines[0]).toMatch(/^time,target,kind,variant,rows,/);
    expect(lines[1]).toContain(',Arrow IPC · zstd,');
    expect(lines[1]).toContain(',1.235,');
  });

  it('quotes cells with commas, quotes and newlines, and leaves nulls empty', () => {
    const csv = toCsv([makeMeasurement({ target: 'a,"b"\nc', dbMs: null, cache: null })]);
    const row = csv.split('\n').slice(1).join('\n');
    expect(row).toContain('"a,""b""\nc"');
    expect(row).toContain(',,'); // null cells
  });
});

describe('toMarkdown', () => {
  it('writes a table with escaped pipes', () => {
    const md = toMarkdown([makeMeasurement({ variant: 'a|b' })]);
    const lines = md.trimEnd().split('\n');
    expect(lines).toHaveLength(3);
    expect(lines[0]).toMatch(/^\| time \| target \|/);
    expect(lines[1]).toMatch(/^\| --- \|/);
    expect(lines[2]).toContain('a\\|b');
  });

  it('writes just the header for no rows', () => {
    expect(toMarkdown([]).trimEnd().split('\n')).toHaveLength(2);
  });
});
