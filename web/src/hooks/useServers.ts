import { useCallback, useEffect, useRef, useState } from 'react';
import { fetchInfo, withTimeout } from '../lib/api';
import { errorMessage } from '../lib/errors';
import { pingSamples } from '../lib/measure';
import {
  addServers,
  DEFAULT_SERVERS,
  importServers,
  parseSavedServers,
  removeServer,
  type SavedServer,
  selectServer,
  SERVER_TIMEOUT_MS,
  SERVERS_PARAM,
  type ServersParam,
  serversFromQuery,
  withoutServersParam,
} from '../lib/servers';
import { median } from '../lib/stats';
import { STORAGE_KEYS } from '../lib/storage';
import type { ApiInfo } from '../lib/types';
import { usePersistentState } from './usePersistentState';

export interface ServerRtt {
  medianMs: number | null;
  measuring: boolean;
  error: string | null;
}

/** What one saved server answered: its `/api/info` and its round-trip time. */
export interface ServerStatus {
  info: ApiInfo | null;
  error: string | null;
  loading: boolean;
  rtt: ServerRtt;
}

export interface ServersState {
  list: readonly SavedServer[];
  status: (base: string) => ServerStatus;
  add: (base: string, label?: string) => void;
  remove: (base: string) => void;
  select: (base: string, selected: boolean) => void;
  /** Loads `/api/info` again, then measures the RTT again if the server is ticked. */
  refresh: (base: string) => void;
  /** Records an RTT measured elsewhere: Compare servers measures it again just before its requests. */
  reportRtt: (base: string, medianMs: number) => void;
  /** Whether the page was opened with a `servers` parameter that listed anything. */
  imported: boolean;
  /** Entries of that parameter that are not http(s) URLs, until the user adds a server. */
  skipped: readonly string[];
}

const NO_RTT: ServerRtt = { medianMs: null, measuring: false, error: null };
const PENDING: ServerStatus = { info: null, error: null, loading: true, rtt: NO_RTT };

/**
 * Reads the `servers` query parameter. It leaves the address bar once it
 * added a server; a parameter that added none stays, so it can be corrected.
 */
function takeServersParam(): ServersParam | null {
  if (!new URLSearchParams(location.search).has(SERVERS_PARAM)) return null;
  const param = serversFromQuery(location.search, location.origin);
  if (param.bases.length > 0) history.replaceState(history.state, '', withoutServersParam(location.href));
  return param;
}

/**
 * The saved server list for Compare servers, with each server's `/api/info`
 * and, for ticked servers, its RTT (measured like the header's, once the
 * server answers). A server that does not answer within `SERVER_TIMEOUT_MS`
 * counts as not reachable. While `paused` (a run is in progress) no RTT is
 * measured and measurements in flight stop, so pings never overlap measured
 * requests; the stopped ones start again after the run.
 */
export function useServers(paused: boolean): ServersState {
  const [list, setList] = usePersistentState<readonly SavedServer[]>(
    STORAGE_KEYS.servers,
    (raw) => parseSavedServers(raw, location.origin),
    DEFAULT_SERVERS,
  );
  const [imported, setImported] = useState(false);
  const [skipped, setSkipped] = useState<readonly string[]>([]);
  const [statuses, setStatuses] = useState<Readonly<Record<string, ServerStatus>>>({});
  const infoRequests = useRef(new Map<string, AbortController>());
  // RTT measurements in flight, and the servers measured since their last refresh.
  const rttRequests = useRef(new Map<string, AbortController>());
  const rttMeasured = useRef(new Set<string>());

  useEffect(() => {
    const param = takeServersParam();
    if (!param) return;
    if (param.bases.length > 0) setList((l) => importServers(l, param.bases));
    setSkipped(param.skipped);
    setImported(param.bases.length > 0 || param.skipped.length > 0);
  }, [setList]);

  const update = useCallback((base: string, change: (s: ServerStatus) => ServerStatus) => {
    setStatuses((all) => ({ ...all, [base]: change(all[base] ?? PENDING) }));
  }, []);

  const loadInfo = useCallback(
    (base: string) => {
      infoRequests.current.get(base)?.abort();
      const controller = new AbortController();
      infoRequests.current.set(base, controller);
      const answer = (change: (s: ServerStatus) => ServerStatus) => {
        if (!controller.signal.aborted) update(base, change);
      };
      // Like useServerInfo, a reload keeps showing what the server said last until it answers.
      update(base, (s) => ({ ...s, error: null, loading: true }));
      withTimeout(SERVER_TIMEOUT_MS, controller.signal, (signal) => fetchInfo(base, signal)).then(
        (info) => answer((s) => ({ ...s, info, error: null, loading: false })),
        (e: unknown) => answer((s) => ({ ...s, info: null, error: errorMessage(e), loading: false })),
      );
    },
    [update],
  );

  const measureRtt = useCallback(
    (base: string) => {
      rttRequests.current.get(base)?.abort();
      const controller = new AbortController();
      rttRequests.current.set(base, controller);
      const answer = (rtt: ServerRtt) => {
        if (controller.signal.aborted) return;
        rttRequests.current.delete(base);
        rttMeasured.current.add(base);
        update(base, (s) => ({ ...s, rtt }));
      };
      update(base, (s) => ({ ...s, rtt: { ...s.rtt, measuring: true, error: null } }));
      withTimeout(SERVER_TIMEOUT_MS, controller.signal, (signal) => pingSamples(base, signal)).then(
        (samples) => answer({ medianMs: median(samples), measuring: false, error: null }),
        (e: unknown) => answer({ medianMs: null, measuring: false, error: errorMessage(e) }),
      );
    },
    [update],
  );

  // Load /api/info of servers as they are added; stop asking removed ones.
  useEffect(() => {
    const bases = new Set(list.map((s) => s.base));
    for (const base of bases) if (!infoRequests.current.has(base)) loadInfo(base);
    for (const requests of [infoRequests.current, rttRequests.current]) {
      for (const [base, controller] of requests) {
        if (bases.has(base)) continue;
        controller.abort();
        requests.delete(base);
      }
    }
    for (const base of rttMeasured.current) if (!bases.has(base)) rttMeasured.current.delete(base);
  }, [list, loadInfo]);

  // Measure each ticked server's RTT once, after it answered.
  useEffect(() => {
    if (paused) return;
    for (const { base, selected } of list) {
      const s = statuses[base];
      const pending = !rttRequests.current.has(base) && !rttMeasured.current.has(base);
      if (selected && s?.info && !s.loading && pending) measureRtt(base);
    }
  }, [list, statuses, paused, measureRtt]);

  // A run is starting: stop the measurements still in flight.
  useEffect(() => {
    if (!paused) return;
    for (const [base, controller] of rttRequests.current) {
      controller.abort();
      update(base, (s) => ({ ...s, rtt: NO_RTT }));
    }
    rttRequests.current.clear();
  }, [paused, update]);

  useEffect(() => {
    const requests = [infoRequests.current, rttRequests.current];
    return () => {
      for (const map of requests) {
        for (const controller of map.values()) controller.abort();
        map.clear();
      }
    };
  }, []);

  const add = useCallback(
    (base: string, label?: string) => {
      setList((l) => addServers(l, [base], label));
      setSkipped([]);
    },
    [setList],
  );
  const remove = useCallback((base: string) => setList((l) => removeServer(l, base)), [setList]);
  const select = useCallback(
    (base: string, selected: boolean) => setList((l) => selectServer(l, base, selected)),
    [setList],
  );
  const refresh = useCallback(
    (base: string) => {
      rttRequests.current.get(base)?.abort();
      rttRequests.current.delete(base);
      rttMeasured.current.delete(base);
      update(base, (s) => ({ ...s, rtt: NO_RTT }));
      loadInfo(base);
    },
    [loadInfo, update],
  );
  const reportRtt = useCallback(
    (base: string, medianMs: number) => {
      rttRequests.current.get(base)?.abort();
      rttRequests.current.delete(base);
      rttMeasured.current.add(base);
      update(base, (s) => ({ ...s, rtt: { medianMs, measuring: false, error: null } }));
    },
    [update],
  );

  return {
    list,
    status: (base) => statuses[base] ?? PENDING,
    add,
    remove,
    select,
    refresh,
    reportRtt,
    imported,
    skipped,
  };
}
