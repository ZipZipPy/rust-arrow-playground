import type { CSSProperties } from 'react';
import { useTooltip } from '../Tooltip';

export interface BarDatum {
  key: string;
  label: string;
  value: number | null;
  /** Formatted value printed at the bar's end. */
  display: string;
  color: string;
  /** Secondary text after the value, e.g. "4.1× smaller". */
  note?: string;
}

/** Labeled horizontal bars sharing one scale that starts at zero. */
export function BarRows({ rows, caption, max }: { rows: readonly BarDatum[]; caption: string; max?: number }) {
  const tip = useTooltip();
  const scale = max ?? Math.max(...rows.map((r) => r.value ?? 0), 1e-9);
  return (
    <figure className="chart">
      <figcaption className="sr-only">{caption}</figcaption>
      <div className="bar-rows">
        {rows.map((row) => {
          const share = row.value === null ? 0 : Math.max(0, Math.min(1, row.value / scale));
          const content = (
            <div className="tooltip-body">
              <div className="tooltip-title">{row.label}</div>
              <strong>{row.display}</strong>
              {row.note && <span className="tooltip-note"> · {row.note}</span>}
            </div>
          );
          return (
            <div key={row.key} className="bar-row" tabIndex={0} {...tip(content)}>
              <span className="bar-label" title={row.label}>
                <span className="bar-label-text">{row.label}</span>
                {row.note && <span className="bar-label-note">{row.note}</span>}
              </span>
              <span className="bar-track">
                {share > 0 && <span className="bar" style={{ '--share': share, background: row.color } as CSSProperties} />}
                <span className="bar-value">
                  {row.display}
                  {row.note && <span className="bar-note">{row.note}</span>}
                </span>
              </span>
            </div>
          );
        })}
      </div>
    </figure>
  );
}
