import type { Ref } from 'react';
import { formatBytes, formatInt, formatMs, formatTimes } from '../../lib/format';
import { ratio } from '../../lib/stats';
import type { BenchReport } from '../../lib/types';
import { FORMAT_COLOR } from '../charts/meta';
import { Term } from '../InfoTip';
import { Card, FormatTag, TableScroll } from './common';

interface BenchCardProps {
  report: BenchReport;
  dataset: string;
  ref?: Ref<HTMLElement>;
}

function CellBar({ value, max, color, label }: { value: number; max: number; color: string; label: string }) {
  const width = max > 0 ? Math.max(0.5, (value / max) * 100) : 0;
  return (
    <span className="cell-bar">
      <span className="cell-bar-track">
        <span className="cell-bar-fill" style={{ width: `${width}%`, background: color }} />
      </span>
      <span className="cell-bar-value">{label}</span>
    </span>
  );
}

/** Server-side encoding of every variant from the same rows: no network, no browser. */
export function BenchCard({ report, dataset, ref }: BenchCardProps) {
  const maxMs = Math.max(...report.results.map((r) => r.encode_ms));
  const maxBytes = Math.max(...report.results.map((r) => r.bytes));
  const jsonBytes = report.results.find((r) => r.format === 'json')?.bytes ?? null;
  const fastest = report.results.reduce((a, b) => (b.encode_ms < a.encode_ms ? b : a), report.results[0]!);
  const smallest = report.results.reduce((a, b) => (b.bytes < a.bytes ? b : a), report.results[0]!);

  return (
    <Card
      ref={ref}
      title="Server benchmark"
      subtitle={
        <>
          {formatInt(report.rows)} rows × {report.columns} columns of <strong>{dataset}</strong> fetched once in{' '}
          {formatMs(report.db_ms)}, then encoded every
          way on the server. No network involved. Fastest: <strong>{fastest.variant}</strong>; smallest:{' '}
          <strong>{smallest.variant}</strong>.
        </>
      }
    >
      <TableScroll label="Encode time and size per variant">
        <table className="data-table bench-table">
          <thead>
            <tr>
              <th scope="col">Variant</th>
              <th scope="col">
                <Term term="encode">Encode time</Term>
              </th>
              <th scope="col">
                <Term term="payload">Size</Term>
              </th>
              <th scope="col" className="num">
                <Term term="sizeRatio">vs JSON</Term>
              </th>
            </tr>
          </thead>
          <tbody>
            {report.results.map((r) => (
              <tr key={r.variant}>
                <th scope="row">
                  <FormatTag format={r.format}>{r.variant}</FormatTag>
                </th>
                <td>
                  <CellBar value={r.encode_ms} max={maxMs} color={FORMAT_COLOR[r.format]} label={formatMs(r.encode_ms)} />
                </td>
                <td>
                  <CellBar value={r.bytes} max={maxBytes} color={FORMAT_COLOR[r.format]} label={formatBytes(r.bytes)} />
                </td>
                <td className="num">{formatTimes(ratio(r.bytes, jsonBytes))}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </TableScroll>
    </Card>
  );
}
