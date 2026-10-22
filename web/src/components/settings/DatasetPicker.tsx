import { useId } from 'react';
import { formatCompact } from '../../lib/format';
import { KIND_LABELS } from '../../lib/settings';
import type { ColumnInfo, DatasetInfo, DatasetKind, SeedStatus } from '../../lib/types';

interface DatasetPickerProps {
  datasets: readonly DatasetInfo[];
  value: string;
  onChange: (name: string) => void;
  /** The selected columns, for the per-request SQL. */
  columns: readonly ColumnInfo[];
  seed: SeedStatus | null;
  disabled?: boolean;
}

/** What each dataset shows about Arrow vs JSON, in a sentence. */
const EXPECTATIONS: Record<string, string> = {
  orders:
    'One wide table read by an index range scan. The database part is small, so the difference between the formats shows clearly.',
  order_lines:
    'Seven tables are joined for every request, so the database takes a bigger share. Many repeated names (country, brand, category) make dictionary-encoded Arrow and Parquet much smaller.',
  daily_sales:
    'A precomputed aggregate read like a table: short rows of counts and sums with a few repeated names. The database is quick and payloads are small, so fewer rows are needed to see a difference.',
  daily_sales_live:
    'The same aggregate, computed from every order line on every request, even for 1,000 rows. The database dominates, so the format barely changes the end-to-end time.',
};

const KIND_EXPECTATIONS: Record<DatasetKind, string> = {
  table: 'Rows read straight from a table.',
  view: 'The view’s query runs for every request, so database time counts more.',
  materialized_view: 'A precomputed result stored like a table, so reads are cheap.',
};

function rowsLabel(d: DatasetInfo): string {
  return d.row_estimate === null ? 'size unknown' : `≈${formatCompact(d.row_estimate)} rows`;
}

/** The statement the server runs for each request (see "SQL per request" in docs/DESIGN.md). */
function requestSql(d: DatasetInfo, columns: readonly ColumnInfo[]): string {
  const names = columns.map((c) => c.name);
  const list = names.length <= 6 ? names.join(', ') : `${names.slice(0, 4).join(', ')}, … (${names.length} columns)`;
  return `SELECT ${list}\nFROM ${d.relation}\nWHERE ${d.key_column} > $1\nORDER BY ${d.key_column}\nLIMIT $2`;
}

function SeedNotice({ seed }: { seed: SeedStatus | null }) {
  if (seed?.state === 'complete') return null;
  return (
    <p className="notice" role="status">
      {seed === null ? (
        <>
          The database has not been seeded yet, so every dataset is empty. Run <code>playground seed</code> (docker
          compose does it for you).
        </>
      ) : (
        <>
          <code>playground seed</code> is still loading the data: row counts and timings will change until it
          finishes.
        </>
      )}
    </p>
  );
}

/** Picks the dataset every request reads, and explains what it costs the database. */
export function DatasetPicker({ datasets, value, onChange, columns, seed, disabled }: DatasetPickerProps) {
  const name = useId();
  const selected = datasets.find((d) => d.name === value) ?? null;

  return (
    <div className="datasets">
      <SeedNotice seed={seed} />
      <div className="dataset-list" role="radiogroup" aria-label="Dataset">
        {datasets.map((d) => (
          <label key={d.name} className="dataset-option" data-selected={d.name === value || undefined}>
            <input
              type="radio"
              name={name}
              value={d.name}
              checked={d.name === value}
              disabled={disabled}
              onChange={() => onChange(d.name)}
            />
            <span className="dataset-option-body">
              <span className="dataset-option-label">{d.label}</span>
              <span className="dataset-option-meta">
                <span className="badge">{KIND_LABELS[d.kind] ?? d.kind}</span>
                <span>{rowsLabel(d)}</span>
              </span>
            </span>
          </label>
        ))}
      </div>
      {selected && (
        <div className="dataset-details">
          <p>{selected.description}</p>
          <p className="dataset-expect">
            <strong>What to expect:</strong> {EXPECTATIONS[selected.name] ?? KIND_EXPECTATIONS[selected.kind]}
          </p>
          <details className="dataset-sql">
            <summary>Show SQL</summary>
            <h4>
              {KIND_LABELS[selected.kind] ?? selected.kind} <code>{selected.relation}</code>
            </h4>
            <pre tabIndex={0} aria-label={`Definition of ${selected.relation}`}>
              <code>{selected.sql}</code>
            </pre>
            <h4>Each request runs</h4>
            <pre tabIndex={0} aria-label="Query run for each request">
              <code>{requestSql(selected, columns)}</code>
            </pre>
          </details>
        </div>
      )}
    </div>
  );
}
