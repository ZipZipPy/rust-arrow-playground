import type { Ref } from 'react';
import { targetLabel } from '../../lib/api';
import { describeSizeRatio, describeSpeedup, formatBytes, formatInt, formatMs } from '../../lib/format';
import { COMPARE_FORMATS } from '../../lib/plan';
import { serverDetail } from '../../lib/servers';
import { FORMAT_LABELS, USE_SCOPE_LABELS } from '../../lib/settings';
import { type GroupSummary, PHASES, type ServerSummary, summarizeByServer } from '../../lib/stats';
import type { DataFormat, Measurement } from '../../lib/types';
import { Legend } from '../charts/Legend';
import { FORMAT_COLOR, PHASE_META } from '../charts/meta';
import { StackedBars } from '../charts/StackedBars';
import { Term } from '../InfoTip';
import { Card, ChartTitle, FormatTag, TableScroll } from './common';

/** A server as it took part in a Compare servers batch. */
export interface ComparedServer {
  base: string;
  label: string;
  region: string | null;
  /** The RTT measured just before the batch's requests, if any. */
  rttMs: number | null;
}

interface ServersCardProps {
  servers: readonly ComparedServer[];
  batch: readonly Measurement[];
  running: boolean;
  ref?: Ref<HTMLElement>;
}

/** `rows` is shown when the servers returned different numbers of rows. */
function ServerName({ server, rows }: { server: ComparedServer; rows: number | null }) {
  const detail = serverDetail(server.label, server.region, server.base);
  return (
    <span className="server-name">
      <span className="server-name-label">{server.label}</span>
      {detail && <span className="server-name-detail">{detail}</span>}
      {rows !== null && <span className="server-name-detail">{formatInt(rows)} rows</span>}
    </span>
  );
}

function FormatCell({ group, fastest }: { group: GroupSummary | undefined; fastest: boolean }) {
  if (!group) return <td className="num">–</td>;
  return (
    <td className={fastest ? 'is-fastest' : undefined}>
      <div className="server-cell">
        <span className="server-cell-time">
          {formatMs(group.endToEnd.median)}
          {fastest && <span className="badge badge-accent">fastest</span>}
        </span>
        <span className="server-cell-wire">{formatBytes(group.wireBytes)}</span>
        <dl className="server-split">
          <dt>TTFB</dt>
          <dd>{formatMs(group.ttfbMs)}</dd>
          <dt>Download</dt>
          <dd>{formatMs(group.downloadMs)}</dd>
          <dt>Decode</dt>
          <dd>{formatMs(group.decodeMs)}</dd>
        </dl>
      </div>
    </td>
  );
}

/**
 * The same JSON vs Arrow IPC vs Parquet comparison against several servers,
 * for example one in the user's country and others abroad.
 */
export function ServersCard({ servers, batch, running, ref }: ServersCardProps) {
  const byTarget = new Map(summarizeByServer(batch).map((s) => [s.target, s]));
  const rows = servers.flatMap((server) => {
    const summary = byTarget.get(targetLabel(server.base));
    return summary ? [{ server, summary }] : [];
  });
  const groups = rows.flatMap(({ summary }) => summary.formats);
  const rounds = Math.max(...groups.map((g) => g.count));
  const scale = Math.max(...groups.map((g) => g.endToEnd.median), 1e-9);
  const first = batch[0]!;
  const rowCounts = rows.length > 0 ? rows.map(({ summary }) => summary.rowCount) : [first.rowCount];
  const fewest = Math.min(...rowCounts);
  const most = Math.max(...rowCounts);
  const sameRows = fewest === most;
  const variant = (format: DataFormat) => batch.find((m) => m.format === format)?.variant ?? FORMAT_LABELS[format];
  const missed = rows.flatMap(({ server, summary }) =>
    summary.cacheMisses > 0
      ? [`${server.label} (${summary.cacheMisses} of ${summary.formats.reduce((n, g) => n + g.count, 0)})`]
      : [],
  );
  const usesFallback = batch.some((m) => m.timingSource === 'fetch');
  const group = (summary: ServerSummary, format: DataFormat) => summary.formats.find((g) => g.format === format);

  return (
    <Card
      ref={ref}
      title="Compare servers"
      busy={running}
      subtitle={
        <>
          <strong>{first.dataset}</strong> ·{' '}
          {sameRows ? formatInt(most) : `${formatInt(fewest)} to ${formatInt(most)}`} rows × {first.columnCount} columns ·{' '}
          {servers.length} servers · {rounds === 1 ? '1 round' : `median of ${rounds} rounds`}, interleaved · HTTP
          compression {first.httpCompression ? 'on' : 'off'} · touch {USE_SCOPE_LABELS[first.useScope].toLowerCase()}
          {running && ' · running…'}
        </>
      }
    >
      {!sameRows && (
        <p className="callout">
          The servers returned different numbers of rows (under each name in the table), so their times do not compare
          directly.
        </p>
      )}
      {missed.length === 0 ? (
        <p className="callout">
          Every server read {sameRows ? 'the same rows' : 'its rows'} from its row cache, loaded before each round, so
          the database is left out and these times are the network, encoding and decoding, wherever the database is.
        </p>
      ) : (
        <p className="callout">
          Not every request was answered from the row cache, so some times include the database, and a server far from
          it looks slower for that alone. Requests that missed: {missed.join(', ')}. A server with several instances
          keeps one cache per instance; run again to compare without the database.
        </p>
      )}

      <div className="panels">
        {rows.map(({ server, summary }) => (
          <div key={server.base} className="panel">
            <ChartTitle label={server.label}>
              <span className="chart-title-note">RTT {formatMs(server.rttMs)}</span>
            </ChartTitle>
            <StackedBars
              caption={`Median time per phase for each format on ${server.label}`}
              max={scale}
              legend={false}
              rows={summary.formats.map((g) => ({
                key: g.key,
                label: g.variant,
                color: FORMAT_COLOR[g.format],
                phases: g.phases,
                total: g.endToEnd.median,
              }))}
            />
          </div>
        ))}
      </div>
      <Legend items={PHASES.map((p) => PHASE_META[p])} />

      <TableScroll label="Comparison per server">
        <table className="data-table servers-table">
          <thead>
            <tr>
              <th scope="col">Server</th>
              <th scope="col" className="num">
                <Term term="rtt">RTT</Term>
              </th>
              {COMPARE_FORMATS.map((format) => (
                <th key={format} scope="col">
                  <FormatTag format={format}>{variant(format)}</FormatTag>
                </th>
              ))}
              <th scope="col" className="num">
                <Term term="speedup">Arrow vs JSON</Term>
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map(({ server, summary }) => (
              <tr key={server.base}>
                <th scope="row">
                  <ServerName server={server} rows={sameRows ? null : summary.rowCount} />
                </th>
                <td className="num">{formatMs(server.rttMs)}</td>
                {COMPARE_FORMATS.map((format) => (
                  <FormatCell key={format} group={group(summary, format)} fastest={summary.fastest === format} />
                ))}
                <td className="num">
                  <div className="server-cell">
                    <strong>{describeSpeedup(summary.arrowSpeedup)}</strong>
                    <span className="server-cell-wire">{describeSizeRatio(summary.arrowSize)}</span>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </TableScroll>

      <p className="footnote">
        Each format shows its median <Term term="endToEnd">end to end</Term> time, the bytes{' '}
        <Term term="wire">on the wire</Term>, and the medians of <Term term="ttfb">time to first byte</Term>,{' '}
        <Term term="download">download</Term> and <Term term="decode">decode</Term>. RTT was measured just before the
        requests, one server at a time.
      </p>
      {usesFallback && (
        <p className="footnote">
          <Term term="timingSource">Some timings were measured around fetch()</Term>
        </p>
      )}
    </Card>
  );
}
