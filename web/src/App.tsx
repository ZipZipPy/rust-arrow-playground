import { type RefObject, useCallback, useEffect, useRef, useState } from 'react';
import { Header } from './components/Header';
import { Hero } from './components/Hero';
import { BenchCard } from './components/results/BenchCard';
import { CompareCard } from './components/results/CompareCard';
import { HistoryCard } from './components/results/HistoryCard';
import { HowToRead } from './components/results/HowToRead';
import { LastRunCard } from './components/results/LastRunCard';
import { PreviewCard } from './components/results/PreviewCard';
import { SettingsPanel } from './components/settings/SettingsPanel';
import { TooltipProvider } from './components/Tooltip';
import { usePersistentState } from './hooks/usePersistentState';
import { useRtt } from './hooks/useRtt';
import { useRunner } from './hooks/useRunner';
import { useServerInfo } from './hooks/useServerInfo';
import { useTheme } from './hooks/useTheme';
import { fetchBench, normalizeBaseUrl, targetLabel } from './lib/api';
import { parseHistory, prependHistory } from './lib/history';
import { loadParquet } from './lib/parquet';
import { planCompare, planRun, type PlannedRequest } from './lib/plan';
import { executePlan, type LatestPreview } from './lib/runner';
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
import { STORAGE_KEYS } from './lib/storage';
import type { BenchReport, Measurement } from './lib/types';

interface BenchResult {
  dataset: string;
  report: BenchReport;
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

  const [storedSettings, setSettings] = usePersistentState(STORAGE_KEYS.settings, sanitizeSettings, DEFAULT_SETTINGS);
  const [history, setHistory] = usePersistentState<Measurement[]>(STORAGE_KEYS.history, parseHistory, []);
  const [runBatch, setRunBatch] = useState<Measurement[]>([]);
  const [compareBatch, setCompareBatch] = useState<Measurement[]>([]);
  const [bench, setBench] = useState<BenchResult | null>(null);
  const [preview, setPreview] = useState<LatestPreview | null>(null);

  const runRef = useRef<HTMLElement>(null);
  const compareRef = useRef<HTMLElement>(null);
  const benchRef = useRef<HTMLElement>(null);
  useRevealOnChange(runRef, runBatch[0]?.batchId);
  useRevealOnChange(compareRef, compareBatch[0]?.batchId);
  useRevealOnChange(benchRef, bench ?? undefined);

  const { info } = server;
  const settings = info ? fitToServer(storedSettings, info) : storedSettings;
  const dataset = info ? findDataset(info, settings.dataset) : null;
  const columns = dataset ? selectedColumns(settings, dataset.columns) : [];
  const blockedReason = !info
    ? server.loading
      ? 'Connecting to the server…'
      : 'The server is not reachable. Check the API base URL above.'
    : !dataset
      ? 'The server lists no datasets.'
      : columns.length === 0
        ? 'Select at least one column.'
        : null;

  // Fetch and compile parquet-wasm as soon as Parquet is picked, so the first run doesn't wait for it.
  const wantsParquet = settings.useArrow && settings.arrowFlavor === 'parquet';
  useEffect(() => {
    if (wantsParquet) loadParquet().catch(() => {});
  }, [wantsParquet]);

  // Changes apply to the dataset on screen, which may be the fallback for a stored one the server lacks.
  const updateSettings = useCallback(
    (patch: Partial<Settings>) => setSettings((s) => ({ ...(info ? resolveDataset(s, info) : s), ...patch })),
    [setSettings, info],
  );
  const selectDataset = useCallback(
    (name: string) => setSettings((s) => switchDataset(info ? resolveDataset(s, info) : s, name)),
    [setSettings, info],
  );

  const startPlan = (kind: 'run' | 'compare', plan: PlannedRequest[]) => {
    const setBatch = kind === 'run' ? setRunBatch : setCompareBatch;
    const batchId = Date.now();
    runner.start(kind, (signal, report) =>
      executePlan({
        base: apiBase,
        target: targetLabel(apiBase),
        kind,
        batchId,
        plan,
        columns,
        useScope: settings.useScope,
        offsetMode: offsetMode(settings),
        signal,
        onProgress: report,
        onMeasurement: (m) => {
          setBatch((b) => appendToBatch(b, m));
          setHistory((h) => prependHistory(h, m));
        },
        onPreview: setPreview,
      }),
    );
  };

  const onRun = () => {
    if (dataset) startPlan('run', planRun(settings, dataset));
  };
  const onCompare = () => {
    if (dataset) startPlan('compare', planCompare(settings, dataset));
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

  const hasResults = runBatch.length > 0 || compareBatch.length > 0 || bench !== null;

  return (
    <TooltipProvider>
      <div className="app">
        <Header
          apiBase={apiBase}
          onApiBaseChange={setApiBase}
          server={server}
          dataset={dataset}
          rtt={rtt}
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
            onRun={onRun}
            onCompare={onCompare}
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
                info={info}
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
