import { pickUseColumn } from '../../lib/decode';
import { formatCompact, formatInt } from '../../lib/format';
import {
  BATCH_SIZES,
  MAX_ITERATIONS,
  ROW_PRESETS,
  selectedColumns,
  type Settings,
  USE_SCOPE_LABELS,
} from '../../lib/settings';
import {
  type ApiInfo,
  type DatasetInfo,
  IPC_COMPRESSIONS,
  type IpcCompression,
  PARQUET_COMPRESSIONS,
  type ParquetCompression,
  USE_SCOPES,
  type UseScope,
} from '../../lib/types';
import { Checkbox, NumberField, Section, Segmented, Select, Switch } from '../controls';
import { ColumnPicker } from './ColumnPicker';
import { DatasetPicker } from './DatasetPicker';

interface SettingsPanelProps {
  settings: Settings;
  info: ApiInfo | null;
  /** The selected dataset, once the server has answered. */
  dataset: DatasetInfo | null;
  onChange: (patch: Partial<Settings>) => void;
  onDatasetChange: (name: string) => void;
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
const useScopeOptions = USE_SCOPES.map((scope) => ({ value: scope, label: USE_SCOPE_LABELS[scope] }));

/** Offers only what the server advertises, in our canonical order. */
function offered<T extends string>(all: readonly T[], advertised: readonly string[] | undefined): T[] {
  return advertised ? all.filter((c) => advertised.includes(c)) : [...all];
}

function InactiveNote({ active }: { active: boolean }) {
  return active ? null : <span className="section-note">used by Compare</span>;
}

function describeUseScope(scope: UseScope, column: string | null): string {
  if (scope === 'all') {
    return 'After decoding, read every value of every selected column once, as the JavaScript value an app works with: strings decoded, 64-bit integers and timestamps turned into numbers. JSON built these values while parsing; Arrow and Parquet build them now. The same work in every format, like a table view or an export.';
  }
  const which = column ? `, “${column}”` : '';
  return `Read one column only${which} (an amount when one is selected): the column scan Arrow makes cheap, where no rows or strings are ever built. JSON already built every value while parsing, so this shows Arrow’s best case.`;
}

export function SettingsPanel({ settings: s, info, dataset, onChange, onDatasetChange, disabled }: SettingsPanelProps) {
  const maxRows = info?.max_rows ?? 1_000_000;
  const rowPresets = ROW_PRESETS.filter((n) => n <= maxRows).map((n) => ({ value: n, label: formatCompact(n) }));
  const arrowActive = s.useArrow && s.arrowFlavor === 'arrow';
  const parquetActive = s.useArrow && s.arrowFlavor === 'parquet';
  const columns = dataset ? selectedColumns(s, dataset.columns) : [];
  const smallDataset = dataset?.row_estimate != null && dataset.row_estimate < s.rows;

  return (
    <div className="settings" aria-label="Settings">
      <Section title="Dataset">
        {info && info.datasets.length > 0 ? (
          <DatasetPicker
            datasets={info.datasets}
            value={s.dataset}
            onChange={onDatasetChange}
            columns={columns}
            seed={info.seed}
            disabled={disabled}
          />
        ) : (
          <p className="field-hint">Datasets appear once the server is reachable.</p>
        )}
      </Section>

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
          hint={
            <>
              Server limit: {formatInt(maxRows)} rows
              {smallDataset && ` · ${dataset.name} has only ≈${formatCompact(dataset.row_estimate)}, so you get fewer`}
            </>
          }
        />
      </Section>

      <Section title="Columns">
        {dataset ? (
          <ColumnPicker
            catalog={dataset.columns}
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
          label={`Return rows with ${dataset?.key_column ?? 'id'} greater than`}
          value={s.offset}
          min={0}
          max={dataset?.max_key ?? Number.MAX_SAFE_INTEGER}
          onChange={(offset) => onChange({ offset })}
          disabled={disabled || s.randomizeOffset}
          hint={dataset?.max_key != null ? `Largest ${dataset.key_column}: ${formatInt(dataset.max_key)}` : undefined}
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
          description="zstd / br / gzip picked from what the browser accepts, timed on the server as Compress. Shrinks JSON a lot, but the browser still parses text. Skipped for bodies that are already compressed."
        />
        <Switch
          checked={s.cache}
          onChange={(cache) => onChange({ cache })}
          disabled={disabled}
          label="Server row cache"
          description="Reuse rows already fetched for the same dataset, rows, start id and columns, to take PostgreSQL out of the picture. Compare loads each round’s rows first, so no format pays the query alone. Compare servers always uses it."
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

      <Section title="Touch data">
        <Segmented
          label="What the browser reads after decoding"
          value={s.useScope}
          options={useScopeOptions}
          onChange={(useScope) => onChange({ useScope })}
          disabled={disabled}
        />
        <p className="field-hint">{describeUseScope(s.useScope, pickUseColumn(columns))}</p>
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
