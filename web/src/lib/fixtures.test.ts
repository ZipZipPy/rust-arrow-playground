// Decodes payloads produced by the Rust server (JSON, Arrow IPC with every body
// compression and dictionary option, Parquet with every codec) through the
// same functions the browser uses. Skipped when the fixture directory is absent.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { basename, extname, join, relative } from 'node:path';
import { describe, expect, it } from 'vitest';
import { parquet } from '../test/parquetNode';
import { type Decoded, decodeArrowIpc, decodeJson, decodeParquet, previewRows, rowCount, useColumnValue } from './decode';

const DIR = process.env.FIXTURES_DIR ?? '/tmp/claude-0/fixtures';
const ARROW_EXTENSIONS = new Set(['.arrow', '.arrows', '.ipc', '.feather']);

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}

interface Fixture {
  name: string;
  kind: 'json' | 'arrow' | 'parquet';
  decode: () => Decoded;
}

function classify(path: string): Fixture | null {
  const ext = extname(path).toLowerCase();
  const name = relative(DIR, path);
  const bytes = () => new Uint8Array(readFileSync(path));
  if (ext === '.json') {
    // Manifests and other metadata are JSON too; only arrays of rows are payloads.
    const text = readFileSync(path, 'utf8').trimStart();
    return text.startsWith('[') ? { name, kind: 'json', decode: () => decodeJson(bytes()) } : null;
  }
  if (ARROW_EXTENSIONS.has(ext)) return { name, kind: 'arrow', decode: () => decodeArrowIpc(bytes()) };
  if (ext === '.parquet') return { name, kind: 'parquet', decode: () => decodeParquet(bytes(), parquet) };
  return null;
}

const fixtures = existsSync(DIR) ? walk(DIR).map(classify).filter((f): f is Fixture => f !== null) : [];

describe.skipIf(fixtures.length === 0)(`server fixtures in ${DIR}`, () => {
  const decoded = new Map(fixtures.map((f) => [f.name, f.decode()]));
  const jsonByRows = new Map<number, Decoded>();
  for (const f of fixtures) {
    const d = decoded.get(f.name)!;
    if (f.kind === 'json' && !jsonByRows.has(rowCount(d))) jsonByRows.set(rowCount(d), d);
  }

  it('found JSON, Arrow and Parquet payloads', () => {
    const kinds = new Set(fixtures.map((f) => f.kind));
    console.info(`decoded ${fixtures.length} fixtures:`, fixtures.map((f) => `${f.name} (${rowCount(decoded.get(f.name)!)} rows)`).join(', '));
    expect(kinds).toEqual(new Set(['json', 'arrow', 'parquet']));
  });

  it.each(fixtures.map((f) => [f.name, f] as const))('%s decodes to rows matching the JSON fixture', (_, f) => {
    const d = decoded.get(f.name)!;
    const rows = rowCount(d);
    expect(rows).toBeGreaterThan(0);
    const json = jsonByRows.get(rows);
    if (!json || json === d) return;

    // Same values, formatted the same way, for the columns both files carry.
    const mine = previewRows(d, 20);
    const theirs = previewRows(json, 20);
    const shared = mine.columns.filter((c) => theirs.columns.includes(c));
    expect(shared.length).toBeGreaterThan(0);
    const pickColumns = (p: typeof mine) => p.rows.map((r) => shared.map((c) => r[p.columns.indexOf(c)]));
    expect(pickColumns(mine), basename(f.name)).toEqual(pickColumns(theirs));

    for (const column of ['total', 'id', 'rating'].filter((c) => shared.includes(c))) {
      expect(useColumnValue(d, column)).toBeCloseTo(useColumnValue(json, column), 3);
    }
  });
});
