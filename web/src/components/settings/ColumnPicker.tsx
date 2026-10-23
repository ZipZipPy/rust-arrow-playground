import { columnGroups, columnsForGroup } from '../../lib/settings';
import type { ColumnInfo } from '../../lib/types';

interface ColumnPickerProps {
  catalog: readonly ColumnInfo[];
  /** `null` = all columns. */
  value: string[] | null;
  onChange: (columns: string[] | null) => void;
  disabled?: boolean;
}

/** "All", then one preset per column group the dataset has. */
function presets(catalog: readonly ColumnInfo[]): { key: string; label: string; group: string | null }[] {
  return [
    { key: 'all', label: 'All', group: null },
    ...columnGroups(catalog).map((group) => ({ key: group, label: group[0]!.toUpperCase() + group.slice(1), group })),
  ];
}

function sameSelection(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((name) => b.includes(name));
}

/** Short type label for the badge, e.g. `Timestamp(Microsecond, "UTC")` → `timestamp`. */
function typeBadge(column: ColumnInfo): string {
  return column.arrow_type.replace(/\(.*$/, '').toLowerCase();
}

export function ColumnPicker({ catalog, value, onChange, disabled }: ColumnPickerProps) {
  const selected = value ?? catalog.map((c) => c.name);

  const toggle = (name: string, on: boolean) => {
    const next = catalog.map((c) => c.name).filter((n) => (n === name ? on : selected.includes(n)));
    onChange(next.length === catalog.length ? null : next);
  };

  return (
    <div className="columns">
      <div className="segmented segmented-wrap" role="group" aria-label="Column presets">
        {presets(catalog).map((preset) => {
          const names = columnsForGroup(catalog, preset.group) ?? catalog.map((c) => c.name);
          return (
            <button
              key={preset.key}
              type="button"
              aria-pressed={sameSelection(names, selected)}
              disabled={disabled || names.length === 0}
              onClick={() => onChange(preset.group === null ? null : names)}
            >
              {preset.label}
            </button>
          );
        })}
      </div>
      <p className="field-hint">
        {selected.length} of {catalog.length} columns
        {selected.length === 0 && <strong className="field-error"> · pick at least one</strong>}
      </p>
      <ul className="column-list">
        {catalog.map((column) => (
          <li key={column.name}>
            <label className="checkbox column-item" title={`${column.pg_type} → ${column.arrow_type}`}>
              <input
                type="checkbox"
                checked={selected.includes(column.name)}
                disabled={disabled}
                onChange={(e) => toggle(column.name, e.target.checked)}
              />
              <span className="column-name">{column.name}</span>
              <span className="badge badge-type">{typeBadge(column)}</span>
              {column.dictionary && (
                <span className="badge badge-accent" title="Low-cardinality text: dictionary-encoded when the Arrow dictionary option is on">
                  dict
                </span>
              )}
            </label>
          </li>
        ))}
      </ul>
    </div>
  );
}
