import type { Activity } from '../hooks/useRunner';
import { formatInt } from '../lib/format';
import { activeFormat, type ArrowFlavor, buildRequest, type Settings, variantLabel } from '../lib/settings';
import { Segmented, Switch } from './controls';

interface HeroProps {
  settings: Settings;
  onChange: (patch: Partial<Settings>) => void;
  columnCount: number;
  ready: boolean;
  blockedReason: string | null;
  activity: Activity | null;
  /**
   * Compare servers: how many ticked servers answer, why it cannot start yet
   * (null when it can), and a note on servers it leaves out. It does not
   * depend on `ready`, which is about the API server.
   */
  servers: { count: number; blockedReason: string | null; note: string | null };
  onRun: () => void;
  onCompare: () => void;
  onCompareServers: () => void;
  /** Opens the server list. */
  onChooseServers: () => void;
  onBench: () => void;
  onCancel: () => void;
}

const FLAVORS = [
  { value: 'arrow', label: 'Arrow IPC stream', title: 'Record batches the browser maps straight into typed arrays' },
  { value: 'parquet', label: 'Parquet file', title: 'Compressed columnar file, decoded with parquet-wasm' },
] as const satisfies readonly { value: ArrowFlavor; label: string; title: string }[];

const ACTIVITY_LABEL = {
  run: 'Running',
  compare: 'Comparing',
  servers: 'Comparing servers',
  bench: 'Server benchmark',
} as const;
const ROLE_LABEL = { warmup: 'warm-up', prime: 'loading the row cache' } as const;

function describeFormat(s: Settings): string {
  if (!s.useArrow) return 'Rows as JSON objects. The browser has to parse every character and build one object per row.';
  if (s.arrowFlavor === 'arrow') {
    return 'Columns as Arrow record batches. The bytes already have the in-memory layout, so the browser wraps them as typed arrays.';
  }
  return 'Columns in a compressed Parquet file. Smallest on the wire; the browser decodes it with WebAssembly.';
}

export function Hero(props: HeroProps) {
  const { settings, onChange, activity, ready, blockedReason } = props;
  const format = activeFormat(settings);
  const variant = variantLabel(buildRequest(settings, format, 0, null));
  const busy = activity !== null;
  const progress = activity?.progress;
  const percent = progress ? Math.round((progress.done / progress.total) * 100) : 0;
  const serversHint = [
    props.servers.blockedReason === blockedReason ? null : props.servers.blockedReason,
    props.servers.note,
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <section className={`hero ${settings.useArrow ? 'hero-arrow' : 'hero-json'}`} aria-label="Format and actions">
      <div className="hero-switch">
        <Switch
          size="large"
          checked={settings.useArrow}
          onChange={(useArrow) => onChange({ useArrow })}
          disabled={busy}
          label={
            <>
              Use Arrow <span className="switch-state">{settings.useArrow ? 'On' : 'Off · JSON'}</span>
            </>
          }
          description={describeFormat(settings)}
        />
        <Segmented
          label="Arrow transport"
          value={settings.arrowFlavor}
          options={FLAVORS}
          onChange={(arrowFlavor) => onChange({ arrowFlavor, useArrow: true })}
          disabled={busy}
        />
        <p className="hero-summary">
          Next run sends <strong>{variant}</strong> · {formatInt(settings.rows)} rows × {props.columnCount} columns
          {settings.dataset && (
            <>
              {' '}
              of <strong>{settings.dataset}</strong>
            </>
          )}{' '}
          · HTTP compression {settings.httpCompression ? 'on' : 'off'}
        </p>
      </div>

      <div className="hero-actions">
        <button
          type="button"
          className="button button-primary button-large"
          onClick={props.onRun}
          disabled={!ready || busy}
        >
          Run {variant}
        </button>
        <div className="hero-secondary">
          <button type="button" className="button" onClick={props.onCompare} disabled={!ready || busy}>
            Compare JSON · Arrow · Parquet
          </button>
          <button
            type="button"
            className="button"
            onClick={props.onCompareServers}
            disabled={busy || props.servers.blockedReason !== null}
          >
            {props.servers.count >= 2 ? `Compare ${props.servers.count} servers` : 'Compare servers'}
          </button>
          <button type="button" className="button" onClick={props.onBench} disabled={!ready || busy}>
            Server benchmark
          </button>
        </div>
        {blockedReason && !busy && <p className="hero-blocked">{blockedReason}</p>}
        {serversHint && !busy && (
          <p className="hero-blocked">
            {serversHint}{' '}
            <button type="button" className="link-button" onClick={props.onChooseServers}>
              Choose servers
            </button>
          </p>
        )}
        {busy && (
          <div className="progress" role="status" aria-live="polite">
            <div className="progress-text">
              <span>
                {ACTIVITY_LABEL[activity.kind]}
                {progress && (
                  <>
                    {' · '}
                    {progress.current.role === 'measure'
                      ? `request ${progress.done + 1} of ${progress.total}`
                      : ROLE_LABEL[progress.current.role]}{' '}
                    ·{' '}
                    {variantLabel(progress.current.request)}
                    {progress.current.server && ` · ${progress.current.server.label}`}
                  </>
                )}
                …
              </span>
              <button type="button" className="button button-ghost button-small" onClick={props.onCancel}>
                Stop
              </button>
            </div>
            <div className="progress-track">
              <div className="progress-bar" style={{ width: `${progress ? percent : 100}%` }} data-indeterminate={!progress || undefined} />
            </div>
          </div>
        )}
      </div>
    </section>
  );
}
