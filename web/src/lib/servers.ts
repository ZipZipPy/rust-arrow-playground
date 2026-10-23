// The servers Compare servers runs against: the page's own origin plus any
// others the user added, for example the same image on Cloud Run in other
// regions. Saved in this browser like the settings.
import { normalizeBaseUrl, targetLabel } from './api';
import { formatInt } from './format';
import type { ServerTarget } from './plan';
import { findDataset, type Settings } from './settings';
import type { ApiInfo, DatasetInfo } from './types';

export interface SavedServer {
  /** API base URL as `normalizeBaseUrl` returns it; '' is the page's own origin. */
  base: string;
  /** A name the user gave it; otherwise the label comes from the server's region or host. */
  label?: string;
  /** Whether it takes part in Compare servers. */
  selected: boolean;
}

/** The query parameter that adds servers: a comma-separated list of absolute base URLs. */
export const SERVERS_PARAM = 'servers';

/**
 * How long a server in the list may take to answer `/api/info` or its RTT
 * pings before it counts as not reachable. Long enough for a Cloud Run cold
 * start on another continent.
 */
export const SERVER_TIMEOUT_MS = 15_000;

const THIS_SERVER: SavedServer = { base: '', selected: true };

export const DEFAULT_SERVERS: readonly SavedServer[] = [THIS_SERVER];

/**
 * A base URL for the server list. The page's own origin becomes '' ("This
 * server"), so a link that lists it next to other servers does not compare
 * it with itself. Throws like `normalizeBaseUrl`.
 */
export function serverBase(raw: string, origin: string): string {
  const base = normalizeBaseUrl(raw);
  return base === normalizeBaseUrl(origin) ? '' : base;
}

/**
 * Accepts anything (e.g. stale localStorage) and returns a valid list with
 * "This server" first. With `origin`, a saved URL of the page's own origin
 * (added while the page ran elsewhere) also counts as "This server".
 */
export function parseSavedServers(raw: unknown, origin?: string): SavedServer[] {
  const seen = new Set<string>();
  const others: SavedServer[] = [];
  let self: SavedServer = THIS_SERVER;
  for (const item of Array.isArray(raw) ? raw : []) {
    if (!item || typeof item !== 'object') continue;
    const r = item as Partial<Record<keyof SavedServer, unknown>>;
    if (typeof r.base !== 'string') continue;
    let base: string;
    try {
      base = origin === undefined ? normalizeBaseUrl(r.base) : serverBase(r.base, origin);
    } catch {
      continue;
    }
    if (seen.has(base)) continue;
    seen.add(base);
    const server: SavedServer = { base, selected: typeof r.selected === 'boolean' ? r.selected : true };
    if (typeof r.label === 'string' && r.label.trim() !== '') server.label = r.label.trim();
    if (base === '') self = server;
    else others.push(server);
  }
  return [self, ...others];
}

/** Adds servers, or ticks them when they are already in the list; a label replaces the one they had. */
export function addServers(list: readonly SavedServer[], bases: readonly string[], label?: string): SavedServer[] {
  const next = list.map((s) => (bases.includes(s.base) ? { ...s, ...(label ? { label } : {}), selected: true } : s));
  for (const base of new Set(bases)) {
    if (next.some((s) => s.base === base)) continue;
    next.push(label ? { base, label, selected: true } : { base, selected: true });
  }
  return next;
}

/**
 * Adds the servers of a `servers` link and ticks exactly those, so the link
 * decides what is compared. The others stay in the list, unticked: for
 * example "This server" when a local UI opens a link to Cloud Run servers.
 */
export function importServers(list: readonly SavedServer[], bases: readonly string[]): SavedServer[] {
  return addServers(list, bases).map((s) => ({ ...s, selected: bases.includes(s.base) }));
}

/** Removes a server; "This server" always stays. */
export function removeServer(list: readonly SavedServer[], base: string): SavedServer[] {
  return base === '' ? [...list] : list.filter((s) => s.base !== base);
}

export function selectServer(list: readonly SavedServer[], base: string, selected: boolean): SavedServer[] {
  return list.map((s) => (s.base === base ? { ...s, selected } : s));
}

export interface ServersParam {
  /** Normalized and deduplicated. */
  bases: string[];
  /** Entries that are not absolute http(s) URLs, as written. */
  skipped: string[];
}

/** The servers in the `servers` query parameter. */
export function serversFromQuery(search: string, origin: string): ServersParam {
  const value = new URLSearchParams(search).get(SERVERS_PARAM) ?? '';
  const bases = new Set<string>();
  const skipped: string[] = [];
  for (const entry of value.split(',')) {
    if (entry.trim() === '') continue;
    try {
      bases.add(serverBase(entry, origin));
    } catch {
      skipped.push(entry.trim());
    }
  }
  return { bases: [...bases], skipped };
}

/** The page URL without the `servers` parameter, so the address bar stays clean after the import. */
export function withoutServersParam(href: string): string {
  const url = new URL(href);
  url.searchParams.delete(SERVERS_PARAM);
  return url.href;
}

/**
 * The name shown for a server: the user's label, else its region from
 * `/api/info`, else its host. The page's own origin is "This server".
 */
export function serverLabel(server: Pick<SavedServer, 'base' | 'label'>, region: string | null | undefined): string {
  if (server.label) return server.label;
  if (server.base === '') return region ? `This server (${region})` : 'This server';
  return region || targetLabel(server.base);
}

/** The region and host, leaving out what the label already says. */
export function serverDetail(label: string, region: string | null, base: string): string {
  const host = targetLabel(base);
  const parts = [region && !label.includes(region) ? region : null, label === host ? null : host];
  return parts.filter(Boolean).join(' · ');
}

/** A server's name in messages: its label, plus the host when the label does not already say it. */
export function serverName(label: string, base: string): string {
  const host = targetLabel(base);
  return label === host || base === '' ? label : `${label} (${host})`;
}

/** One ticked server and what its `/api/info` answered just before Compare servers starts. */
export interface ServerAnswer {
  server: SavedServer;
  info: ApiInfo | null;
  /** Why `/api/info` failed. */
  error: string | null;
}

/**
 * About how many of the requested rows a dataset returns after `offset`,
 * from its key range and key density as in `randomOffset`; null when its
 * largest key is unknown.
 */
function rowsAfter(dataset: Pick<DatasetInfo, 'max_key' | 'row_estimate'>, offset: number, rows: number): number | null {
  if (dataset.max_key === null) return null;
  const density = dataset.row_estimate && dataset.max_key > 0 ? Math.min(1, dataset.row_estimate / dataset.max_key) : 1;
  return Math.min(rows, Math.max(0, Math.floor((dataset.max_key - offset) * density)));
}

/** Row estimates come from table statistics, so differences this small between servers are noise. */
const ROWS_TOLERANCE = 0.01;

/**
 * Checks that every ticked server can serve the current settings and returns
 * the plan's targets. Throws one error that names every server that cannot:
 * not reachable, data not seeded or still loading, no such dataset or
 * columns, a lower row limit, a fixed start id past its last key, or fewer
 * rows to return than the others (then the servers would not send the same
 * rows, and their times would not compare).
 */
export function checkServers(answers: readonly ServerAnswer[], s: Settings): ServerTarget[] {
  const problems: string[] = [];
  const targets: ServerTarget[] = [];
  // A random start id leaves the requested rows on every server that has that many
  // (see narrowestKeyRange), so count those from 0.
  const from = s.randomizeOffset ? 0 : s.offset;
  const available: { who: string; rows: number }[] = [];
  for (const { server, info, error } of answers) {
    const name = serverLabel(server, info?.server.region);
    const who = serverName(name, server.base);
    if (!info) {
      problems.push(`${who} is not reachable: ${error ?? 'no answer'}`);
      continue;
    }
    if (info.seed?.state !== 'complete') {
      problems.push(info.seed ? `${who} is still loading its data` : `${who} has no data yet (never seeded)`);
      continue;
    }
    const dataset = findDataset(info, s.dataset);
    if (!dataset) {
      problems.push(`${who} has no dataset “${s.dataset}”`);
      continue;
    }
    const known = new Set(dataset.columns.map((c) => c.name));
    const missing = (s.columns ?? []).filter((c) => !known.has(c));
    if (missing.length > 0) {
      problems.push(`${who} lacks the column${missing.length === 1 ? '' : 's'} ${missing.join(', ')}`);
    }
    if (s.rows > info.max_rows) problems.push(`${who} returns at most ${formatInt(info.max_rows)} rows`);
    if (!s.randomizeOffset && s.offset > 0 && dataset.max_key !== null && s.offset >= dataset.max_key) {
      const largest = `largest ${dataset.key_column}: ${formatInt(dataset.max_key)}`;
      problems.push(`start id ${formatInt(s.offset)} leaves no rows on ${who} (${largest})`);
    } else {
      const rows = rowsAfter(dataset, from, s.rows);
      if (rows !== null) available.push({ who, rows });
    }
    targets.push({ server: { base: server.base, label: name, region: info.server.region }, dataset });
  }
  const most = available.reduce<{ who: string; rows: number } | null>((a, b) => (a && a.rows >= b.rows ? a : b), null);
  for (const { who, rows } of available) {
    if (most && rows < most.rows * (1 - ROWS_TOLERANCE)) {
      problems.push(`${who} would return about ${formatInt(rows)} rows, against ${formatInt(most.rows)} from ${most.who}`);
    }
  }
  if (problems.length > 0) {
    const text = problems.join('; ');
    throw new Error(`Compare servers cannot start: ${text}${/[.?!]$/.test(text) ? '' : '.'}`);
  }
  return targets;
}
