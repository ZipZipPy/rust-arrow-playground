import type { Ref } from 'react';
import type { ThemeChoice } from '../hooks/useTheme';
import type { RttState } from '../hooks/useRtt';
import type { ServerInfoState } from '../hooks/useServerInfo';
import type { ServersState } from '../hooks/useServers';
import type { DatasetInfo } from '../lib/types';
import { targetLabel } from '../lib/api';
import { formatBytes, formatCompact, formatMs } from '../lib/format';
import { ConnectionPanel } from './ConnectionPanel';
import { InfoTip } from './InfoTip';

interface HeaderProps {
  apiBase: string;
  onApiBaseChange: (base: string) => void;
  server: ServerInfoState;
  /** The selected dataset, for the size chip. */
  dataset: DatasetInfo | null;
  rtt: RttState;
  /** The servers Compare servers runs against, listed in the connection panel. */
  servers: ServersState;
  /** The heading of that list, for moving the focus to it. */
  serverListRef?: Ref<HTMLHeadingElement>;
  /** Whether the connection panel is open. */
  editing: boolean;
  onEditingChange: (editing: boolean) => void;
  busy: boolean;
  theme: ThemeChoice;
  onCycleTheme: () => void;
}

const THEME_LABEL: Record<ThemeChoice, string> = { system: 'Auto', light: 'Light', dark: 'Dark' };

function ThemeIcon({ theme }: { theme: ThemeChoice }) {
  if (theme === 'light') {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <circle cx="12" cy="12" r="4.5" />
        <path d="M12 2.5v2.5M12 19v2.5M2.5 12H5M19 12h2.5M5.3 5.3l1.8 1.8M16.9 16.9l1.8 1.8M5.3 18.7l1.8-1.8M16.9 7.1l1.8-1.8" />
      </svg>
    );
  }
  if (theme === 'dark') {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5Z" />
      </svg>
    );
  }
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 3.5v17a8.5 8.5 0 0 0 0-17Z" fill="currentColor" />
    </svg>
  );
}

function datasetTitle(d: DatasetInfo): string {
  const rows = d.row_estimate === null ? 'unknown number of rows' : `${d.row_estimate.toLocaleString('en-US')} rows (estimate)`;
  const key = d.max_key === null ? '' : `, max ${d.key_column} ${d.max_key.toLocaleString('en-US')}`;
  return `${d.label}: ${rows}${key}`;
}

export function Header({
  apiBase,
  onApiBaseChange,
  server,
  dataset,
  rtt,
  servers,
  serverListRef,
  editing,
  onEditingChange,
  busy,
  theme,
  onCycleTheme,
}: HeaderProps) {
  const { info } = server;
  const status = server.loading ? 'loading' : info ? 'ok' : 'error';
  const location = [info?.server.region, info?.server.service, info?.server.revision].filter(Boolean).join(' · ');

  return (
    <header className="topbar">
      <div className="topbar-inner">
        <div className="brand">
          <svg className="brand-mark" viewBox="0 0 32 32" aria-hidden="true">
            <rect x="3" y="7" width="6" height="18" rx="2" />
            <rect x="12" y="11" width="6" height="14" rx="2" />
            <path d="M21 9h7l-3.5-4M28 9l-3.5 4" />
          </svg>
          <div>
            <h1>Rust Arrow Playground</h1>
            <p>JSON vs Arrow IPC vs Parquet, from PostgreSQL to your browser</p>
          </div>
        </div>

        <div className="server-strip">
          <button
            type="button"
            className="chip chip-button"
            aria-expanded={editing}
            onClick={() => onEditingChange(!editing)}
            title="Change the API server or pick servers to compare"
          >
            <span className={`status-dot status-${status}`} aria-hidden="true" />
            <span className="chip-label">Server</span>
            <span className="chip-value">{targetLabel(apiBase)}</span>
          </button>
          {dataset && (
            <span className="chip" title={datasetTitle(dataset)}>
              <span className="chip-label">{dataset.name}</span>
              <span className="chip-value">
                {dataset.row_estimate === null ? '? rows' : `${formatCompact(dataset.row_estimate)} rows`}
                {dataset.bytes !== null && ` · ${formatBytes(dataset.bytes)}`}
              </span>
            </span>
          )}
          {info && (
            <span className="chip">
              <span className="chip-label">Runs on</span>
              <span className="chip-value">{location || 'local'} · v{info.server.version}</span>
            </span>
          )}
          {info && (
            <span className="chip">
              <span className="chip-label">RTT</span>
              <span className="chip-value">{rtt.measuring ? 'measuring…' : rtt.error ? 'failed' : formatMs(rtt.medianMs)}</span>
              <button
                type="button"
                className="icon-button"
                onClick={rtt.measure}
                disabled={rtt.measuring || busy}
                aria-label="Measure round-trip time again"
                title="Measure again"
              >
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4v4.5H15" />
                </svg>
              </button>
              <InfoTip term="rtt" label="RTT" />
            </span>
          )}
          <button
            type="button"
            className="icon-button theme-button"
            onClick={onCycleTheme}
            aria-label={`Theme: ${THEME_LABEL[theme]}. Switch theme`}
            title={`Theme: ${THEME_LABEL[theme]}`}
          >
            <ThemeIcon theme={theme} />
          </button>
        </div>
      </div>

      {(editing || status === 'error') && (
        <ConnectionPanel
          apiBase={apiBase}
          error={server.error}
          loading={server.loading}
          onSave={(base) => {
            onApiBaseChange(base);
            onEditingChange(false);
          }}
          onRetry={server.reload}
          onClose={editing ? () => onEditingChange(false) : undefined}
          servers={servers}
          serverListRef={serverListRef}
          busy={busy}
        />
      )}
    </header>
  );
}
