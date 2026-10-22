import type { Ref } from 'react';
import {
  describeSizeRatio,
  describeSpeedup,
  formatBytes,
  formatInt,
  formatMs,
  formatTimes,
} from '../../lib/format';
import type { GlossaryKey } from '../../lib/glossary';
import { USE_SCOPE_LABELS } from '../../lib/settings';
import { type GroupSummary, ratio, speedup, summarizeByFormat } from '../../lib/stats';
import type { Measurement } from '../../lib/types';
import { type BarDatum, BarRows } from '../charts/BarRows';
import { FORMAT_COLOR } from '../charts/meta';
import { StackedBars } from '../charts/StackedBars';
import { Term } from '../InfoTip';
import { Card, ChartTitle, FormatTag, TableScroll } from './common';

interface CompareCardProps {
  batch: readonly Measurement[];
  running: boolean;
  ref?: Ref<HTMLElement>;
}

interface Panel {
  key: string;
  label: string;
  term: GlossaryKey;
  value: (g: GroupSummary) => number | null;
  format: (v: number | null) => string;
  /** How the value compares with JSON's. */
  versus: (value: number | null, json: number | null) => string | undefined;
}

const timeVersus = (value: number | null, json: number | null) =>
  value === null || json === null ? undefined : describeSpeedup(speedup(json, value));
const sizeVersus = (value: number | null, json: number | null) => {
  const r = ratio(value, json);
  return r === null ? undefined : describeSizeRatio(r);
};

const PANELS: Panel[] = [
  { key: 'e2e', label: 'End to end (median)', term: 'endToEnd', value: (g) => g.endToEnd.median, format: formatMs, versus: timeVersus },
  { key: 'wire', label: 'Bytes on the wire', term: 'wire', value: (g) => g.wireBytes, format: formatBytes, versus: sizeVersus },
  { key: 'browser', label: 'Browser work (decode + use)', term: 'browser', value: (g) => g.browserMs, format: formatMs, versus: timeVersus },
  { key: 'payload', label: 'Payload to decode', term: 'payload', value: (g) => g.payloadBytes, format: formatBytes, versus: sizeVersus },
];

const FORMAT_ORDER = { json: 0, arrow: 1, parquet: 2 } as const;
const order = (g: GroupSummary) => FORMAT_ORDER[g.format];

function Headline({ group, json }: { group: GroupSummary; json: GroupSummary | undefined }) {
  const faster = json ? speedup(json.endToEnd.median, group.endToEnd.median) : null;
  const smaller = json ? ratio(group.wireBytes, json.wireBytes) : null;
  const browser = json ? speedup(json.browserMs, group.browserMs) : null;
  const isBaseline = group.format === 'json';
  return (
    <div className={`headline headline-${group.format}`}>
      <div className="headline-name">
        <FormatTag format={group.format}>{group.variant}</FormatTag>
      </div>
      {isBaseline ? (
        <>
          <div className="headline-figure">{formatMs(group.endToEnd.median)}</div>
          <div className="headline-detail">baseline · {formatBytes(group.wireBytes)} on the wire</div>
        </>
      ) : (
        <>
          <div className="headline-figure">
            {faster === null ? '–' : faster >= 1 ? formatTimes(faster) : formatTimes(1 / faster)}
            <span className="headline-unit">{faster === null || faster >= 1 ? ' faster' : ' slower'}</span>
          </div>
          <div className="headline-detail">
            {formatMs(group.endToEnd.median)} · {describeSizeRatio(smaller)} · browser work {describeSpeedup(browser)}
          </div>
        </>
      )}
    </div>
  );
}

export function CompareCard({ batch, running, ref }: CompareCardProps) {
  const groups = summarizeByFormat(batch).sort((a, b) => order(a) - order(b));
  const json = groups.find((g) => g.format === 'json');
  const first = batch[0]!;
  const rounds = Math.max(...groups.map((g) => g.count));

  return (
    <Card
      ref={ref}
      title="JSON vs Arrow IPC vs Parquet"
      busy={running}
      subtitle={
        <>
          <strong>{first.dataset}</strong> · {formatInt(first.rowCount)} rows × {first.columnCount} columns ·{' '}
          {rounds === 1 ? '1 round' : `median of ${rounds} rounds`}, interleaved · HTTP compression{' '}
          {first.httpCompression ? 'on' : 'off'}
          {first.rowCache && ' · row cache on'} · touch {USE_SCOPE_LABELS[first.useScope].toLowerCase()}
          {running && ' · running…'}
        </>
      }
    >
      <div className="headlines">
        {groups.map((g) => (
          <Headline key={g.key} group={g} json={json} />
        ))}
      </div>

      <div className="panels">
        {PANELS.map((panel) => {
          const jsonValue = json ? panel.value(json) : null;
          const rows: BarDatum[] = groups.map((g) => {
            const value = panel.value(g);
            return {
              key: g.key,
              label: g.variant,
              value,
              display: panel.format(value),
              color: FORMAT_COLOR[g.format],
              note: g.format === 'json' ? undefined : panel.versus(value, jsonValue),
            };
          });
          return (
            <div key={panel.key} className="panel">
              <ChartTitle label={panel.label} term={panel.term} />
              <BarRows caption={panel.label} rows={rows} />
            </div>
          );
        })}
      </div>

      <div className="chart-block">
        <ChartTitle label="Where the time goes (medians)" />
        <StackedBars
          caption="Median time per phase for each format"
          rows={groups.map((g) => ({
            key: g.key,
            label: g.variant,
            color: FORMAT_COLOR[g.format],
            phases: g.phases,
            total: g.endToEnd.median,
          }))}
        />
      </div>

      <TableScroll label="Comparison per format">
        <table className="data-table">
          <thead>
            <tr>
              <th scope="col">Format</th>
              <th scope="col" className="num">n</th>
              <th scope="col" className="num"><Term term="endToEnd">Median</Term></th>
              <th scope="col" className="num">Min</th>
              <th scope="col" className="num">Max</th>
              <th scope="col" className="num"><Term term="speedup">Speed-up</Term></th>
              <th scope="col" className="num"><Term term="wire">Wire</Term></th>
              <th scope="col" className="num"><Term term="sizeRatio">Size vs JSON</Term></th>
              <th scope="col" className="num"><Term term="decode">Decode</Term></th>
              <th scope="col" className="num"><Term term="use">Use</Term></th>
              <th scope="col" className="num"><Term term="payload">Payload</Term></th>
            </tr>
          </thead>
          <tbody>
            {groups.map((g) => (
              <tr key={g.key}>
                <th scope="row">
                  <FormatTag format={g.format}>{g.variant}</FormatTag>
                </th>
                <td className="num">{g.count}</td>
                <td className="num">{formatMs(g.endToEnd.median)}</td>
                <td className="num">{formatMs(g.endToEnd.min)}</td>
                <td className="num">{formatMs(g.endToEnd.max)}</td>
                <td className="num">{json ? formatTimes(speedup(json.endToEnd.median, g.endToEnd.median)) : '–'}</td>
                <td className="num">{formatBytes(g.wireBytes)}</td>
                <td className="num">{json ? formatTimes(ratio(g.wireBytes, json.wireBytes)) : '–'}</td>
                <td className="num">{formatMs(g.decodeMs)}</td>
                <td className="num">{formatMs(g.useMs)}</td>
                <td className="num">{formatBytes(g.payloadBytes)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </TableScroll>
    </Card>
  );
}
