import { type RefObject, useCallback, useEffect, useRef, useState } from 'react';
import { flushSync } from 'react-dom';
import { Header } from './components/Header';
import { Hero } from './components/Hero';
import { BenchCard } from './components/results/BenchCard';
import { CompareCard } from './components/results/CompareCard';
import { HistoryCard } from './components/results/HistoryCard';
import { HowToRead } from './components/results/HowToRead';
import { LastRunCard } from './components/results/LastRunCard';
import { PreviewCard } from './components/results/PreviewCard';
import { type ComparedServer, ServersCard } from './components/results/ServersCard';
import { SettingsPanel } from './components/settings/SettingsPanel';
import { TooltipProvider } from './components/Tooltip';
import { usePersistentState } from './hooks/usePersistentState';
import { useRtt } from './hooks/useRtt';
import { useRunner } from './hooks/useRunner';
import { useServerInfo } from './hooks/useServerInfo';
import { useServers } from './hooks/useServers';
import { useTheme } from './hooks/useTheme';
import { fetchBench, fetchInfo, normalizeBaseUrl, targetLabel, withTimeout } from './lib/api';
import { errorMessage } from './lib/errors';
import { parseHistory, prependHistory } from './lib/history';
import { pingSamples } from './lib/measure';
import { loadParquet } from './lib/parquet';
import { planCompare, planCompareServers, planRun, type PlannedRequest } from './lib/plan';
import { executePlan, type LatestPreview, type Progress } from './lib/runner';
import { checkServers, type SavedServer, type ServerAnswer, SERVER_TIMEOUT_MS, serverLabel } from './lib/servers';
import {
  columnsParam,
  DEFAULT_SETTINGS,
  findDataset,
  fitToServer,
  offsetMode,
  randomOffset,
  resolveDataset,
  sanitizeSettings,
  selectedColumns,
  type Settings,
  switchDataset,
} from './lib/settings';
import { median } from './lib/stats';
import { STORAGE_KEYS } from './lib/storage';
import type { BenchReport, Measurement } from './lib/types';

interface BenchResult {
  dataset: string;
  report: BenchReport;
}

interface ServersBatch {
  servers: readonly ComparedServer[];
  measurements: Measurement[];
}

const BENCH_MAX_ROWS = 1_000_000;

const parseApiBase = (raw: unknown) => (typeof raw === 'string' ? normalizeBaseUrl(raw) : '');

/** Appends to the current batch, or starts over when a new batch begins (keeps the old one on screen until then). */
const appendToBatch = (batch: Measurement[], m: Measurement) => (batch[0]?.batchId === m.batchId ? [...batch, m] : [m]);

/** Brings a result card into view when a new result lands in it, unless its top is already visible. */
function useRevealOnChange(ref: RefObject<HTMLElement | null>, key: unknown) {
  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    const el = ref.current;
    if (!el || key === undefined) return;
    const { top } = el.getBoundingClientRect();
    if (top < 0 || top > window.innerHeight * 0.6) el.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }, [ref, key]);
}

export function App() {
  const { theme, cycleTheme } = useTheme();
  const [apiBase, setApiBase] = usePersistentState(STORAGE_KEYS.apiBase, parseApiBase, '');
  const server = useServerInfo(apiBase);
  const runner = useRunner();
  const busy = runner.activity !== null;
  const rtt = useRtt(apiBase, server.info !== null);
  const servers = useServers(busy);
  const [editingConnection, setEditingConnection] = useState(false);
  const serverListRef = useRef<HTMLHeadingElement>(null);

  // A link with servers to compare opens the list, so the imported servers are in view.
  useEffect(() => {
    if (servers.imported) setEditingConnection(true);
  }, [servers.imported]);

  const [storedSettings, setSettings] = usePersistentState(STORAGE_KEYS.settings, sanitizeSettings, DEFAULT_SETTINGS);
  const [history, setHistory] = usePersistentState<Measurement[]>(STORAGE_KEYS.history, parseHistory, []);
  const [runBatch, setRunBatch] = useState<Measurement[]>([]);
  const [compareBatch, setCompareBatch] = useState<Measurement[]>([]);
  const [serversBatch, setServersBatch] = useState<ServersBatch | null>(null);
  const [bench, setBench] = useState<BenchResult | null>(null);
  const [preview, setPreview] = useState<LatestPreview | null>(null);

  const runRef = useRef<HTMLElement>(null);
  const compareRef = useRef<HTMLElement>(null);
  const serversRef = useRef<HTMLElement>(null);
  const benchRef = useRef<HTMLElement>(null);
  useRevealOnChange(runRef, runBatch[0]?.batchId);
  useRevealOnChange(compareRef, compareBatch[0]?.batchId);
  useRevealOnChange(serversRef, serversBatch?.measurements[0]?.batchId);
  useRevealOnChange(benchRef, bench ?? undefined);

  // Compare servers runs against the ticked servers that answer and leaves out the ones that do not.
  const ticked = servers.list.filter((s) => s.selected);
  const waiting = ticked.filter((s) => servers.status(s.base).loading);
  const answering = ticked.filter((s) => !servers.status(s.base).loading && servers.status(s.base).info !== null);
  const silent = ticked.filter((s) => !servers.status(s.base).loading && servers.status(s.base).info === null);
  const names = (list: readonly SavedServer[]) =>
    list.map((s) => serverLabel(s, servers.status(s.base).info?.server.region)).join(', ');

  const { info } = server;
  // When the API server does not answer, the settings follow the first ticked server that does, so Compare
  // servers still works, for example in a local UI opened with a link to Cloud Run servers.
  const firstAnswer = answering[0] ? servers.status(answering[0].base).info : null;
  const settingsInfo = info ?? (server.loading ? null : firstAnswer);
  const settings = settingsInfo ? fitToServer(storedSettings, settingsInfo) : storedSettings;
  const dataset = settingsInfo ? findDataset(settingsInfo, settings.dataset) : null;
  const columns = dataset ? selectedColumns(settings, dataset.columns) : [];
  const connectReason = server.loading
    ? 'Connecting to the server…'
    : 'The server is not reachable. Check the API base URL above.';
  const settingsBlockedReason = !settingsInfo
    ? connectReason
    : !dataset
      ? 'The server lists no datasets.'
      : columns.length === 0
        ? 'Select at least one column.'
        : null;
  const blockedReason = info ? settingsBlockedReason : connectReason;

  const serversBlockedReason =
    ticked.length < 2
      ? 'Compare servers needs at least two ticked servers.'
      : waiting.length > 0
        ? `Waiting for ${names(waiting)} to answer…`
        : answering.length < 2
          ? 'Compare servers needs at least two ticked servers that answer.'
          : settingsBlockedReason;
  const serversNote =
    silent.length > 0
      ? `${names(silent)} ${silent.length === 1 ? 'does not answer and is' : 'do not answer and are'} left out.`
      : null;

  // Fetch and compile parquet-wasm as soon as Parquet is picked, so the first run doesn't wait for it.
  const wantsParquet = settings.useArrow && settings.arrowFlavor === 'parquet';
  useEffect(() => {
    if (wantsParquet) loadParquet().catch(() => {});
  }, [wantsParquet]);

  // Changes apply to the dataset on screen, which may be the fallback for a stored one the server lacks.
  const updateSettings = useCallback(
    (patch: Partial<Settings>) =>
      setSettings((s) => ({ ...(settingsInfo ? resolveDataset(s, settingsInfo) : s), ...patch })),
    [setSettings, settingsInfo],
  );
  const selectDataset = useCallback(
    (name: string) => setSettings((s) => switchDataset(settingsInfo ? resolveDataset(s, settingsInfo) : s, name)),
    [setSettings, settingsInfo],
  );

  const execute = (
    kind: Measurement['kind'],
    plan: PlannedRequest[],
    signal: AbortSignal,
    report: (progress: Progress) => void,
    addToBatch: (m: Measurement) => void,
  ) =>
    executePlan({
      base: apiBase,
      target: targetLabel(apiBase),
      region: info?.server.region ?? null,
      kind,
      batchId: Date.now(),
      plan,
      columns,
      useScope: settings.useScope,
      offsetMode: offsetMode(settings),
      signal,
      onProgress: report,
      onMeasurement: (m) => {
        addToBatch(m);
        setHistory((h) => prependHistory(h, m));
      },
      onPreview: setPreview,
    });

  const startPlan = (kind: 'run' | 'compare', plan: PlannedRequest[]) => {
    const setBatch = kind === 'run' ? setRunBatch : setCompareBatch;
    runner.start(kind, (signal, report) =>
      execute(kind, plan, signal, report, (m) => setBatch((b) => appendToBatch(b, m))),
    );
  };

  const onRun = () => {
    if (dataset) startPlan('run', planRun(settings, dataset));
  };
  const onCompare = () => {
    if (dataset) startPlan('compare', planCompare(settings, dataset));
  };
  const onCompareServers = () => {
    if (!dataset) return;
    runner.start('servers', async (signal, report) => {
      // Ask every server again: the list may show an old answer, and the plan needs each one's key range.
      const answers = await Promise.all(
        answering.map((server) =>
          withTimeout(SERVER_TIMEOUT_MS, signal, (s) => fetchInfo(server.base, s)).then(
            (answer): ServerAnswer => ({ server, info: answer, error: null }),
            (e: unknown): ServerAnswer => ({ server, info: null, error: errorMessage(e) }),
          ),
        ),
      );
      signal.throwIfAborted();
      const targets = checkServers(answers, settings);
      // Measure each RTT now, one server at a time and before any data request, so pings never overlap them.
      const compared: ComparedServer[] = [];
      for (const { server: target } of targets) {
        const rttMs = await withTimeout(SERVER_TIMEOUT_MS, signal, (s) => pingSamples(target.base, s)).then(
          (samples) => median(samples),
          () => null,
        );
        signal.throwIfAborted();
        if (rttMs !== null) servers.reportRtt(target.base, rttMs);
        compared.push({ ...target, rttMs });
      }
      await execute('servers', planCompareServers(settings, targets), signal, report, (m) =>
        setServersBatch((b) =>
          b?.measurements[0]?.batchId === m.batchId
            ? { ...b, measurements: [...b.measurements, m] }
            : { servers: compared, measurements: [m] },
        ),
      );
    });
  };
  const onChooseServers = () => {
    // Open the list right away, so it can take the focus: it sits in the header, far above the hero.
    flushSync(() => setEditingConnection(true));
    serverListRef.current?.focus({ preventScroll: true });
    window.scrollTo({ top: 0, behavior: 'smooth' });
  };
  const onBench = () => {
    if (!info || !dataset) return;
    const rows = Math.min(settings.rows, info.max_rows, BENCH_MAX_ROWS);
    runner.start('bench', async (signal) => {
      const report = await fetchBench(
        apiBase,
        {
          dataset: dataset.name,
          rows,
          offset: settings.randomizeOffset ? randomOffset(dataset, rows) : settings.offset,
          columns: columnsParam(settings, dataset.columns),
          cache: settings.cache,
        },
        signal,
      );
      setBench({ dataset: dataset.name, report });
    });
  };

  const hasResults = runBatch.length > 0 || compareBatch.length > 0 || serversBatch !== null || bench !== null;

  return (
    <TooltipProvider>
      <div className="app">
        <Header
          apiBase={apiBase}
          onApiBaseChange={setApiBase}
          server={server}
          dataset={info ? dataset : null}
          rtt={rtt}
          servers={servers}
          serverListRef={serverListRef}
          editing={editingConnection}
          onEditingChange={setEditingConnection}
          busy={busy}
          theme={theme}
          onCycleTheme={cycleTheme}
        />
        <main className="main">
          <Hero
            settings={settings}
            onChange={updateSettings}
            columnCount={columns.length}
            ready={blockedReason === null}
            blockedReason={blockedReason}
            activity={runner.activity}
            servers={{ count: answering.length, blockedReason: serversBlockedReason, note: serversNote }}
            onRun={onRun}
            onCompare={onCompare}
            onCompareServers={onCompareServers}
            onChooseServers={onChooseServers}
            onBench={onBench}
            onCancel={runner.cancel}
          />

          {runner.error && (
            <div className="alert" role="alert">
              <span>{runner.error}</span>
              <button type="button" className="button button-small button-ghost" onClick={runner.dismissError}>
                Dismiss
              </button>
            </div>
          )}

          <div className="workspace">
            <aside className="sidebar">
              <SettingsPanel
                settings={settings}
                info={settingsInfo}
                dataset={dataset}
                onChange={updateSettings}
                onDatasetChange={selectDataset}
                disabled={busy}
              />
            </aside>

            <div className="results">
              {!hasResults && (
                <section className="card empty">
                  <h2>Try it</h2>
                  <ol>
                    <li>
                      Press <strong>Run</strong> with <strong>Use Arrow</strong> on, then turn it off and run again: the
                      result card compares the two.
                    </li>
                    <li>
                      Pick another <strong>dataset</strong> in the settings to see what a 7-table join or an aggregate
                      view costs the database, and how much of the difference the format still makes.
                    </li>
                    <li>
                      Or press <strong>Compare</strong> to run JSON, Arrow IPC and Parquet side by side, interleaved.
                    </li>
                    <li>
                      <strong>Compare servers</strong> runs the same comparison against several servers, for example
                      one in your country and one abroad. Add them under <strong>Server</strong> at the top.
                    </li>
                    <li>
                      <strong>Server benchmark</strong> shows encoding cost and size on the server alone, without the
                      network or the browser.
                    </li>
                  </ol>
                </section>
              )}
              {runBatch.length > 0 && (
                <LastRunCard ref={runRef} batch={runBatch} history={history} running={runner.activity?.kind === 'run'} />
              )}
              {compareBatch.length > 0 && (
                <CompareCard ref={compareRef} batch={compareBatch} running={runner.activity?.kind === 'compare'} />
              )}
              {serversBatch && (
                <ServersCard
                  ref={serversRef}
                  servers={serversBatch.servers}
                  batch={serversBatch.measurements}
                  running={runner.activity?.kind === 'servers'}
                />
              )}
              {bench && <BenchCard ref={benchRef} report={bench.report} dataset={bench.dataset} />}
              {preview && <PreviewCard preview={preview} />}
              <HistoryCard history={history} onClear={() => setHistory([])} />
              <HowToRead defaultOpen={!hasResults} />
            </div>
          </div>
        </main>
        <footer className="footer">
          Timings use the browser’s Resource Timing API; server timings come from <code>Server-Timing</code> /{' '}
          <code>X-*</code> headers. Settings and history stay in this browser.
        </footer>
      </div>
    </TooltipProvider>
  );
}
