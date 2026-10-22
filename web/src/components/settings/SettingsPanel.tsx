import { formatCompact, formatInt } from '../../lib/format';
import {
  BATCH_SIZES,
  MAX_ITERATIONS,
  ROW_PRESETS,
  type Settings,
} from '../../lib/settings';
import { type ApiInfo, IPC_COMPRESSIONS, type IpcCompression, PARQUET_COMPRESSIONS, type ParquetCompression } from '../../lib/types';
import { Checkbox, NumberField, Section, Segmented, Select, Switch } from '../controls';
import { ColumnPicker } from './ColumnPicker';

interface SettingsPanelProps {
  settings: Settings;
  info: ApiInfo | null;
  onChange: (patch: Partial<Settings>) => void;
  disabled: boolean;
}

const IPC_LABEL: Record<IpcCompression, string> = { none: 'None', lz4: 'LZ4 frame', zstd: 'Zstandard' };
const PARQUET_LABEL: Record<ParquetCompression, string> = {
  none: 'None',
  snappy: 'Snappy',
  gzip: 'Gzip',
  zstd: 'Zstandard',
  lz4: 'LZ4 (raw)',
  brotli: 'Brotli',
};

const batchOptions = BATCH_SIZES.map((n) => ({ value: n, label: `${formatInt(n)} rows` }));

/** Offers only what the server advertises, in our canonical order. */
function offered<T extends string>(all: readonly T[], advertised: readonly string[] | undefined): T[] {
  return advertised ? all.filter((c) => advertised.includes(c)) : [...all];
}

function InactiveNote({ active }: { active: boolean }) {
  return active ? null : <span className="section-note">used by Compare</span>;
}

export function SettingsPanel({ settings: s, info, onChange, disabled }: SettingsPanelProps) {
  const maxRows = info?.max_rows ?? 1_000_000;
  const rowPresets = ROW_PRESETS.filter((n) => n <= maxRows).map((n) => ({ value: n, label: formatCompact(n) }));
  const arrowActive = s.useArrow && s.arrowFlavor === 'arrow';
  const parquetActive = s.useArrow && s.arrowFlavor === 'parquet';

  return (
    <div className="settings" aria-label="Settings">
      <Section title="Rows">
        <Segmented
          label="Row count presets"
          value={s.rows}
          options={rowPresets}
          onChange={(rows) => onChange({ rows })}
          disabled={disabled}
          wrap
        />
        <NumberField
          label="Exact number of rows"
          value={s.rows}
          min={1}
          max={maxRows}
          onChange={(rows) => onChange({ rows })}
          disabled={disabled}
          hint={`Server limit: ${formatInt(maxRows)} rows`}
        />
      </Section>

      <Section title="Columns">
        {info ? (
          <ColumnPicker
            catalog={info.columns}
            value={s.columns}
            onChange={(columns) => onChange({ columns })}
            disabled={disabled}
          />
        ) : (
          <p className="field-hint">Columns appear once the server is reachable.</p>
        )}
      </Section>

      <Section title="Start id">
        <NumberField
          label="Return rows with id greater than"
          value={s.offset}
          min={0}
          max={info?.max_id ?? Number.MAX_SAFE_INTEGER}
          onChange={(offset) => onChange({ offset })}
          disabled={disabled || s.randomizeOffset}
        />
        <Checkbox checked={s.randomizeOffset} onChange={(randomizeOffset) => onChange({ randomizeOffset })} disabled={disabled}>
          Random start id for every round (avoids measuring only warm database pages)
        </Checkbox>
      </Section>

      <Section title="Transport">
        <Switch
          checked={s.httpCompression}
          onChange={(httpCompression) => onChange({ httpCompression })}
          disabled={disabled}
          label="HTTP compression"
          description="gzip / br / zstd picked from what the browser accepts. Shrinks JSON a lot, but the browser still parses text."
        />
        <Switch
          checked={s.cache}
          onChange={(cache) => onChange({ cache })}
          disabled={disabled}
          label="Server row cache"
          description="Reuse rows already fetched for the same rows, start id and columns, to take PostgreSQL out of the picture."
        />
      </Section>

      <Section title="Arrow IPC" aside={<InactiveNote active={arrowActive} />}>
        <Switch
          checked={s.dictionary}
          onChange={(dictionary) => onChange({ dictionary })}
          disabled={disabled}
          label="Dictionary encoding"
          description="Send repeated strings (country, city, category…) once, then small integer codes."
        />
        <div className="field-pair">
          <Select
            label="IPC body compression"
            value={s.ipcCompression}
            options={offered(IPC_COMPRESSIONS, info?.ipc_compression).map((c) => ({ value: c, label: IPC_LABEL[c] }))}
            onChange={(ipcCompression) => onChange({ ipcCompression })}
            disabled={disabled}
          />
          <Select
            label="Record batch size"
            value={s.arrowBatchSize}
            options={batchOptions}
            onChange={(arrowBatchSize) => onChange({ arrowBatchSize })}
            disabled={disabled}
          />
        </div>
      </Section>

      <Section title="Parquet" aside={<InactiveNote active={parquetActive} />}>
        <div className="field-pair">
          <Select
            label="Compression codec"
            value={s.parquetCompression}
            options={offered(PARQUET_COMPRESSIONS, info?.parquet_compression).map((c) => ({ value: c, label: PARQUET_LABEL[c] }))}
            onChange={(parquetCompression) => onChange({ parquetCompression })}
            disabled={disabled}
          />
          <Select
            label="Row-group size"
            value={s.rowGroupSize}
            options={batchOptions}
            onChange={(rowGroupSize) => onChange({ rowGroupSize })}
            disabled={disabled}
          />
        </div>
      </Section>

      <Section title="Repeat">
        <NumberField
          label={`Iterations per format (1–${MAX_ITERATIONS})`}
          value={s.iterations}
          min={1}
          max={MAX_ITERATIONS}
          onChange={(iterations) => onChange({ iterations })}
          disabled={disabled}
          hint="Results show the median, which ignores one-off hiccups."
        />
        <Checkbox checked={s.warmup} onChange={(warmup) => onChange({ warmup })} disabled={disabled}>
          Warm-up request first (not recorded): opens the connection and warms caches
        </Checkbox>
      </Section>
    </div>
  );
}
