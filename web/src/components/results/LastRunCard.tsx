import type { Ref } from 'react';
import {
  describeSizeRatio,
  describeSpeedup,
  formatBytes,
  formatInt,
  formatMegabytesPerSecond,
  formatMs,
  formatPercent,
  formatRowsPerSecond,
} from '../../lib/format';
import { counterpartSummary, type GroupSummary, PHASES, ratio, speedup, summarizeGroup } from '../../lib/stats';
import type { Measurement } from '../../lib/types';
import { BarRows } from '../charts/BarRows';
import { FORMAT_COLOR, PHASE_META } from '../charts/meta';
import { StackedBars } from '../charts/StackedBars';
import { Term } from '../InfoTip';
import { Card, ChartTitle, FormatTag, Stat } from './common';

interface LastRunCardProps {
  batch: readonly Measurement[];
  history: readonly Measurement[];
  running: boolean;
  ref?: Ref<HTMLElement>;
}

function Counterpart({ current, other }: { current: GroupSummary; other: GroupSummary | null }) {
  const isJson = current.format === 'json';
  if (!other) {
    return (
      <p className="callout">
        {isJson
          ? 'Turn “Use Arrow” on and run again with the same rows and columns to see how much Arrow saves.'
          : 'Turn “Use Arrow” off and run again with the same rows and columns to see what JSON costs.'}
      </p>
    );
  }
  // Always phrase it as "Arrow-based vs JSON".
  const [arrowSide, jsonSide] = isJson ? [other, current] : [current, other];
  const faster = speedup(jsonSide.endToEnd.median, arrowSide.endToEnd.median);
  const smaller = ratio(arrowSide.wireBytes, jsonSide.wireBytes);
  const decode = speedup(jsonSide.decodeMs, arrowSide.decodeMs);
  return (
    <div className="callout callout-compare">
      <span>
        <FormatTag format={arrowSide.format}>{arrowSide.variant}</FormatTag> vs <FormatTag format="json">JSON</FormatTag>
        <span className="muted"> ({isJson ? 'your latest Arrow run' : 'your latest JSON run'}, same rows and columns)</span>
      </span>
      <span className="callout-figures">
        <strong>{describeSpeedup(faster)}</strong> end to end · <strong>{describeSizeRatio(smaller)}</strong> on the wire ·
        decode <strong>{describeSpeedup(decode)}</strong>
      </span>
    </div>
  );
}

export function LastRunCard({ batch, history, running, ref }: LastRunCardProps) {
  const summary = summarizeGroup('last', batch);
  const first = batch[0]!;
  const counterpart = counterpartSummary(history, first);
  const e2e = summary.endToEnd;
  const usesFallback = batch.some((m) => m.timingSource === 'fetch');
  const encoding = first.contentEncoding ?? 'none';

  return (
    <Card
      ref={ref}
      title="Last run"
      busy={running}
      subtitle={
        <>
          <FormatTag format={summary.format}>{summary.variant}</FormatTag> · {formatInt(summary.rowCount)} rows ×{' '}
          {first.columnCount} columns · {summary.count === 1 ? '1 request' : `median of ${summary.count} requests`} · HTTP{' '}
          {encoding}
          {first.cache && first.cache !== 'off' ? ` · cache ${first.cache}` : ''}
        </>
      }
    >
      <div className="stats">
        <Stat
          hero
          label="End to end"
          term="endToEnd"
          value={formatMs(e2e.median)}
          detail={summary.count > 1 ? `min ${formatMs(e2e.min)} · max ${formatMs(e2e.max)}` : undefined}
        />
        <Stat label="Rows per second" term="rowsPerSecond" value={formatRowsPerSecond(summary.rowCount, e2e.median)} />
        <Stat label="Throughput" term="megabytesPerSecond" value={formatMegabytesPerSecond(summary.payloadBytes, e2e.median)} />
        <Stat
          label="On the wire"
          term="wire"
          value={formatBytes(summary.wireBytes)}
          detail={`${formatBytes(summary.payloadBytes)} payload`}
        />
      </div>

      <Counterpart current={summary} other={counterpart} />

      <div className="chart-block">
        <ChartTitle label="Where the time goes" />
        <StackedBars
          caption="Median time per phase of the last run"
          rows={[{ key: 'last', label: summary.variant, color: FORMAT_COLOR[summary.format], phases: summary.phases, total: e2e.median }]}
        />
        <table className="data-table phase-table">
          <thead>
            <tr>
              <th scope="col">Phase</th>
              <th scope="col" className="num">Median</th>
              <th scope="col" className="num">Share</th>
            </tr>
          </thead>
          <tbody>
            {PHASES.map((phase) => (
              <tr key={phase}>
                <th scope="row">
                  <span className="legend-swatch" style={{ background: PHASE_META[phase].color }} aria-hidden="true" />
                  <Term term={PHASE_META[phase].term}>{PHASE_META[phase].label}</Term>
                </th>
                <td className="num">{formatMs(summary.phases[phase])}</td>
                <td className="num">{formatPercent(summary.phases[phase] / e2e.median)}</td>
              </tr>
            ))}
            <tr>
              <th scope="row">
                <Term term="ttfb">Time to first byte</Term>
              </th>
              <td className="num">{formatMs(summary.ttfbMs)}</td>
              <td />
            </tr>
          </tbody>
        </table>
      </div>

      <div className="chart-block">
        <ChartTitle label="Size" />
        <BarRows
          caption="Bytes on the wire and payload bytes"
          rows={[
            {
              key: 'wire',
              label: 'On the wire',
              value: summary.wireBytes,
              display: formatBytes(summary.wireBytes),
              color: FORMAT_COLOR[summary.format],
              note: encoding === 'none' ? 'no HTTP compression' : `HTTP ${encoding}`,
            },
            {
              key: 'payload',
              label: 'Payload',
              value: summary.payloadBytes,
              display: formatBytes(summary.payloadBytes),
              color: FORMAT_COLOR[summary.format],
            },
          ]}
        />
      </div>

      {usesFallback && (
        <p className="footnote">
          <Term term="timingSource">Some timings were measured around fetch()</Term>
        </p>
      )}
    </Card>
  );
}
