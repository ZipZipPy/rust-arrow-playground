import type { CSSProperties } from 'react';
import { formatMs } from '../../lib/format';
import { PHASES, type PhaseTimes } from '../../lib/stats';
import { useTooltip } from '../Tooltip';
import { Legend } from './Legend';
import { PHASE_META } from './meta';

export interface StackedRow {
  key: string;
  label: string;
  /** Small swatch identifying the row's format, if any. */
  color?: string;
  phases: PhaseTimes;
  total: number;
}

function PhaseTooltip({ row, focus }: { row: StackedRow; focus?: keyof PhaseTimes }) {
  return (
    <div className="tooltip-body">
      <div className="tooltip-title">
        {row.label} · <strong>{formatMs(row.total)}</strong>
      </div>
      <table>
        <tbody>
          {PHASES.map((phase) => (
            <tr key={phase} className={phase === focus ? 'is-focus' : undefined}>
              <td>
                <span className="tooltip-key" style={{ background: PHASE_META[phase].color }} />
                {PHASE_META[phase].label}
              </td>
              <td className="num">
                <strong>{formatMs(row.phases[phase])}</strong>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/**
 * Horizontal stacked bars of request phases on one shared time scale, so rows
 * can be compared by length. Values for every segment are in the tooltip and
 * in the table each caller renders next to the chart.
 */
export function StackedBars({ rows, caption }: { rows: readonly StackedRow[]; caption: string }) {
  const tip = useTooltip();
  const max = Math.max(...rows.map((r) => r.total), 1e-9);
  return (
    <figure className="chart">
      <figcaption className="sr-only">{caption}</figcaption>
      <div className="bar-rows bar-rows-compact">
        {rows.map((row) => {
          const { onFocus, onBlur } = tip(<PhaseTooltip row={row} />);
          return (
            <div key={row.key} className="bar-row" tabIndex={0} onFocus={onFocus} onBlur={onBlur}>
              <span className="bar-label" title={row.label}>
                {row.color && <span className="format-dot" style={{ background: row.color }} aria-hidden="true" />}
                <span className="bar-label-text">{row.label}</span>
              </span>
              <span className="bar-track">
                <span className="stack" style={{ '--share': row.total / max } as CSSProperties}>
                  {PHASES.map((phase) => {
                    const value = row.phases[phase];
                    if (!(value > 0)) return null;
                    const { onPointerMove, onPointerLeave } = tip(<PhaseTooltip row={row} focus={phase} />);
                    return (
                      <span
                        key={phase}
                        className="stack-segment"
                        style={{ flexGrow: value, background: PHASE_META[phase].color }}
                        onPointerMove={onPointerMove}
                        onPointerLeave={onPointerLeave}
                      />
                    );
                  })}
                </span>
                <span className="bar-value">{formatMs(row.total)}</span>
              </span>
            </div>
          );
        })}
      </div>
      <Legend items={PHASES.map((p) => PHASE_META[p])} />
    </figure>
  );
}
