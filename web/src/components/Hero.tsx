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
  onRun: () => void;
  onCompare: () => void;
  onBench: () => void;
  onCancel: () => void;
}

const FLAVORS = [
  { value: 'arrow', label: 'Arrow IPC stream', title: 'Record batches the browser maps straight into typed arrays' },
  { value: 'parquet', label: 'Parquet file', title: 'Compressed columnar file, decoded with parquet-wasm' },
] as const satisfies readonly { value: ArrowFlavor; label: string; title: string }[];

const ACTIVITY_LABEL = { run: 'Running', compare: 'Comparing', bench: 'Server benchmark' } as const;

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
          Next run sends <strong>{variant}</strong> · {formatInt(settings.rows)} rows × {props.columnCount} columns · HTTP
          compression {settings.httpCompression ? 'on' : 'off'}
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
          <button type="button" className="button" onClick={props.onBench} disabled={!ready || busy}>
            Server benchmark
          </button>
        </div>
        {blockedReason && !busy && <p className="hero-blocked">{blockedReason}</p>}
        {busy && (
          <div className="progress" role="status" aria-live="polite">
            <div className="progress-text">
              <span>
                {ACTIVITY_LABEL[activity.kind]}
                {progress && (
                  <>
                    {' · '}
                    {progress.current.warmup ? 'warm-up' : `request ${progress.done + 1} of ${progress.total}`} ·{' '}
                    {variantLabel(progress.current.request)}
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
