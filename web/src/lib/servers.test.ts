import { describe, expect, it } from 'vitest';
import { makeDataset, makeInfo, ORDER_LINES } from '../test/serverInfo';
import {
  addServers,
  checkServers,
  DEFAULT_SERVERS,
  importServers,
  parseSavedServers,
  removeServer,
  type SavedServer,
  selectServer,
  serverDetail,
  serverLabel,
  serverName,
  serversFromQuery,
  withoutServersParam,
} from './servers';
import { DEFAULT_SETTINGS, type Settings } from './settings';

const ORIGIN = 'https://playground-abc-as.a.run.app';
const US = 'https://playground-abc-uc.a.run.app';
const EU = 'https://playground-abc-ew.a.run.app';

describe('parseSavedServers', () => {
  it('starts with this server when nothing is saved', () => {
    expect(parseSavedServers(null)).toEqual(DEFAULT_SERVERS);
    expect(parseSavedServers('nope')).toEqual(DEFAULT_SERVERS);
    expect(parseSavedServers({ base: US })).toEqual(DEFAULT_SERVERS);
  });

  it('keeps saved servers, labels and ticks, with this server first', () => {
    const saved = [
      { base: US, label: ' Iowa ', selected: false },
      { base: '', selected: false },
      { base: EU, selected: true },
    ];
    expect(parseSavedServers(JSON.parse(JSON.stringify(saved)))).toEqual([
      { base: '', selected: false },
      { base: US, label: 'Iowa', selected: false },
      { base: EU, selected: true },
    ]);
  });

  it('adds this server when it is missing, and drops malformed entries and duplicates', () => {
    const parsed = parseSavedServers([
      null,
      'https://x.run.app',
      { base: 42 },
      { base: 'ftp://files.example.com' },
      { base: 'not a url' },
      { base: `${US}/api/` },
      { base: US, label: 7, selected: 'yes' },
    ]);
    expect(parsed).toEqual([{ base: '', selected: true }, { base: US, selected: true }]);
  });

  it('counts a saved URL of the page’s own origin as this server', () => {
    const saved = [{ base: '', selected: false }, { base: US, selected: true }, { base: ORIGIN, label: 'Singapore', selected: true }];
    expect(parseSavedServers(saved, US)).toEqual([
      { base: '', selected: false },
      { base: ORIGIN, label: 'Singapore', selected: true },
    ]);
  });
});

describe('server list changes', () => {
  const list: SavedServer[] = [{ base: '', selected: false }, { base: US, selected: false }];

  it('adds new servers ticked and ticks the ones already listed', () => {
    expect(addServers(list, ['', EU, EU])).toEqual([
      { base: '', selected: true },
      { base: US, selected: false },
      { base: EU, selected: true },
    ]);
    expect(addServers(list, [EU], 'Belgium').at(-1)).toEqual({ base: EU, label: 'Belgium', selected: true });
  });

  it('gives a listed server the label it is added again with', () => {
    const labelled = addServers([...list, { base: EU, label: 'EU', selected: false }], [US], 'Iowa');
    expect(addServers(labelled, [''], 'Bangkok')[0]).toEqual({ base: '', label: 'Bangkok', selected: true });
    expect(labelled.slice(1)).toEqual([
      { base: US, label: 'Iowa', selected: true },
      { base: EU, label: 'EU', selected: false },
    ]);
    expect(addServers(labelled, [US])[1]).toEqual({ base: US, label: 'Iowa', selected: true });
  });

  it('removes any server but this one', () => {
    expect(removeServer(list, US)).toEqual([{ base: '', selected: false }]);
    expect(removeServer(list, '')).toEqual(list);
  });

  it('ticks and unticks one server', () => {
    expect(selectServer(list, US, true)).toEqual([{ base: '', selected: false }, { base: US, selected: true }]);
  });
});

describe('serversFromQuery', () => {
  it('reads a comma-separated list of base URLs and normalizes each', () => {
    const search = `?servers=${encodeURIComponent(`${US}/api/, ${EU}/,${US}`)}`;
    expect(serversFromQuery(search, ORIGIN)).toEqual({ bases: [US, EU], skipped: [] });
  });

  it('turns this page’s own origin into this server, so it is not compared with itself', () => {
    expect(serversFromQuery(`?servers=${ORIGIN},${US}`, ORIGIN).bases).toEqual(['', US]);
    expect(serversFromQuery(`?servers=${ORIGIN}/`, `${ORIGIN}`).bases).toEqual(['']);
  });

  it('returns the entries that are not absolute http(s) URLs, as written', () => {
    expect(serversFromQuery('?servers=,nope, localhost:9080 ,ftp://x.example.com,http://localhost:9080', ORIGIN)).toEqual({
      bases: ['http://localhost:9080'],
      skipped: ['nope', 'localhost:9080', 'ftp://x.example.com'],
    });
    expect(serversFromQuery('?other=1', ORIGIN)).toEqual({ bases: [], skipped: [] });
    expect(serversFromQuery('', ORIGIN)).toEqual({ bases: [], skipped: [] });
  });
});

describe('importServers', () => {
  it('adds the link’s servers without duplicates and ticks exactly those', () => {
    const saved = parseSavedServers([
      { base: '', selected: true },
      { base: US, label: 'Iowa', selected: false },
      { base: EU, selected: true },
    ]);
    const far = 'https://playground-abc-ne.a.run.app';
    expect(importServers(saved, serversFromQuery(`?servers=${US},${far}`, ORIGIN).bases)).toEqual([
      { base: '', selected: false },
      { base: US, label: 'Iowa', selected: true },
      { base: EU, selected: false },
      { base: far, selected: true },
    ]);
  });

  it('keeps this server ticked when the link lists the page’s own origin', () => {
    const imported = importServers(DEFAULT_SERVERS, serversFromQuery(`?servers=${ORIGIN},${US}`, ORIGIN).bases);
    expect(imported).toEqual([{ base: '', selected: true }, { base: US, selected: true }]);
  });
});

describe('withoutServersParam', () => {
  it('removes only the servers parameter', () => {
    expect(withoutServersParam(`${ORIGIN}/?servers=${US}`)).toBe(`${ORIGIN}/`);
    expect(withoutServersParam(`${ORIGIN}/?a=1&servers=${US},${EU}&b=2#top`)).toBe(`${ORIGIN}/?a=1&b=2#top`);
  });
});

describe('serverLabel / serverDetail', () => {
  it('prefers the user’s label, then the region, then the host', () => {
    expect(serverLabel({ base: US, label: 'Iowa' }, 'us-central1')).toBe('Iowa');
    expect(serverLabel({ base: US }, 'us-central1')).toBe('us-central1');
    expect(serverLabel({ base: US }, null)).toBe('playground-abc-uc.a.run.app');
  });

  it('calls the page’s origin this server, with its region when it has one', () => {
    expect(serverLabel({ base: '' }, null)).toBe('This server');
    expect(serverLabel({ base: '' }, 'asia-southeast1')).toBe('This server (asia-southeast1)');
  });

  it('adds the region and host the label does not already show', () => {
    expect(serverDetail('us-central1', 'us-central1', US)).toBe('playground-abc-uc.a.run.app');
    expect(serverDetail('Iowa', 'us-central1', US)).toBe('us-central1 · playground-abc-uc.a.run.app');
    expect(serverDetail('playground-abc-uc.a.run.app', null, US)).toBe('');
    expect(serverDetail('This server', null, '')).toBe('same origin');
  });

  it('names a server in messages with its host unless the label is the host', () => {
    expect(serverName('us-central1', US)).toBe('us-central1 (playground-abc-uc.a.run.app)');
    expect(serverName('playground-abc-uc.a.run.app', US)).toBe('playground-abc-uc.a.run.app');
    expect(serverName('This server (asia-southeast1)', '')).toBe('This server (asia-southeast1)');
    expect(serverName('eu', 'https://lb.example.com/eu')).toBe('eu (lb.example.com/eu)');
  });
});

describe('checkServers', () => {
  const settings = (overrides: Partial<Settings> = {}): Settings => ({ ...DEFAULT_SETTINGS, dataset: 'orders', rows: 10_000, ...overrides });
  const here: SavedServer = { base: '', selected: true };
  const us: SavedServer = { base: US, selected: true };
  const eu: SavedServer = { base: EU, selected: true };
  const usInfo = makeInfo({ server: { version: '0.2.0', region: 'us-central1', service: 'arrow-playground', revision: null } });

  it('returns a target per server, named after its region', () => {
    const targets = checkServers(
      [
        { server: here, info: makeInfo(), error: null },
        { server: us, info: usInfo, error: null },
      ],
      settings(),
    );
    expect(targets.map((t) => t.server)).toEqual([
      { base: '', label: 'This server', region: null },
      { base: US, label: 'us-central1', region: 'us-central1' },
    ]);
    expect(targets.every((t) => t.dataset.name === 'orders')).toBe(true);
  });

  it('names every server that is unreachable or lacks the dataset', () => {
    const noLines = makeInfo({ datasets: [makeDataset()] });
    const run = () =>
      checkServers(
        [
          { server: here, info: makeInfo(), error: null },
          { server: eu, info: noLines, error: null },
          { server: us, info: null, error: 'Network error: Failed to fetch. Is the server running and reachable (CORS)?' },
        ],
        settings({ dataset: 'order_lines' }),
      );
    expect(run).toThrow(
      'Compare servers cannot start: playground-abc-ew.a.run.app has no dataset “order_lines”; ' +
        'playground-abc-uc.a.run.app is not reachable: Network error: Failed to fetch. Is the server running and reachable (CORS)?',
    );
    expect(run).not.toThrow('?.');
  });

  it('refuses columns, row counts and fixed start ids a server cannot serve', () => {
    const small = makeInfo({
      max_rows: 5_000,
      datasets: [makeDataset({ max_key: 50_000, columns: makeDataset().columns.filter((c) => c.name !== 'country') })],
      server: { ...usInfo.server },
    });
    const run = (s: Settings) => () =>
      checkServers(
        [
          { server: here, info: makeInfo(), error: null },
          { server: us, info: small, error: null },
        ],
        s,
      );
    expect(run(settings({ columns: ['id', 'country'] }))).toThrow('us-central1 (playground-abc-uc.a.run.app) lacks the column country');
    expect(run(settings())).toThrow('us-central1 (playground-abc-uc.a.run.app) returns at most 5,000 rows');
    expect(run(settings({ rows: 1000, offset: 60_000 }))).toThrow(
      'start id 60,000 leaves no rows on us-central1 (playground-abc-uc.a.run.app) (largest id: 50,000)',
    );
    // A random start id is drawn from the smallest key range instead.
    expect(run(settings({ rows: 1000, offset: 60_000, randomizeOffset: true }))()).toHaveLength(2);
    expect(run(settings({ rows: 1000, offset: 40_000 }))()).toHaveLength(2);
  });

  it('accepts a dataset whose largest key is unknown', () => {
    const info = makeInfo({ datasets: [{ ...ORDER_LINES, max_key: null }] });
    expect(checkServers([{ server: us, info, error: null }], settings({ dataset: 'order_lines', offset: 5 }))).toHaveLength(1);
  });

  it('refuses servers whose data is not seeded or still loading', () => {
    const loading = makeInfo({ seed: { ...makeInfo().seed!, state: 'loading' }, server: { ...usInfo.server } });
    const run = () =>
      checkServers(
        [
          { server: here, info: makeInfo({ seed: null }), error: null },
          { server: us, info: loading, error: null },
        ],
        settings(),
      );
    expect(run).toThrow(
      'Compare servers cannot start: This server has no data yet (never seeded); ' +
        'us-central1 (playground-abc-uc.a.run.app) is still loading its data.',
    );
  });

  describe('rows to return', () => {
    // A local database with 50,000 orders next to a cloud server with 1,000,000.
    const small = makeInfo({ datasets: [makeDataset({ row_estimate: 50_000, max_key: 50_000 })] });
    const run = (s: Settings) => () =>
      checkServers(
        [
          { server: here, info: small, error: null },
          { server: us, info: usInfo, error: null },
        ],
        s,
      );

    it('refuses servers that would send fewer rows than the others', () => {
      expect(run(settings({ rows: 100_000 }))).toThrow(
        'Compare servers cannot start: This server would return about 50,000 rows, against 100,000 from ' +
          'us-central1 (playground-abc-uc.a.run.app).',
      );
      expect(run(settings({ rows: 100_000, randomizeOffset: true }))).toThrow('This server would return about 50,000 rows');
      expect(run(settings({ rows: 10_000, offset: 45_000 }))).toThrow('This server would return about 5,000 rows');
    });

    it('accepts servers that each have the requested rows', () => {
      expect(run(settings({ rows: 50_000 }))()).toHaveLength(2);
      expect(run(settings({ rows: 10_000, offset: 30_000 }))()).toHaveLength(2);
      expect(run(settings({ rows: 40_000, randomizeOffset: true }))()).toHaveLength(2);
    });

    it('counts sparse keys with the key density', () => {
      // 2.5M lines over keys up to 10,000,005: the last 100,000 keys hold about 25,000 lines.
      const lines = (max_key: number) => makeInfo({ datasets: [{ ...ORDER_LINES, max_key, row_estimate: max_key / 4 }] });
      const check = (rows: number) => () =>
        checkServers(
          [
            { server: here, info: lines(10_000_000), error: null },
            { server: us, info: lines(20_000_000), error: null },
          ],
          settings({ dataset: 'order_lines', rows, offset: 9_900_000 }),
        );
      expect(check(25_000)()).toHaveLength(2);
      expect(check(100_000)).toThrow('This server would return about 25,000 rows, against 100,000');
    });

    it('ignores small differences between row estimates', () => {
      const close = makeInfo({ datasets: [makeDataset({ row_estimate: 996_000, max_key: 1_000_000 })] });
      const answers = [
        { server: here, info: close, error: null },
        { server: us, info: usInfo, error: null },
      ];
      expect(checkServers(answers, settings({ rows: 1_000_000 }))).toHaveLength(2);
    });
  });
});
