import { useEffect, useState } from 'react';
import { copyText } from '../../lib/clipboard';
import { toCsv, toMarkdown } from '../../lib/export';
import { formatBytes, formatInt, formatMs } from '../../lib/format';
import type { Measurement } from '../../lib/types';
import { Term } from '../InfoTip';
import { Card, FormatTag, TableScroll } from './common';

const VISIBLE_ROWS = 100;

interface HistoryCardProps {
  history: readonly Measurement[];
  onClear: () => void;
}

const KIND_BADGE: Partial<Record<Measurement['kind'], string>> = { compare: 'cmp', servers: 'srv' };

const timeOfDay = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });

export function HistoryCard({ history, onClear }: HistoryCardProps) {
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(null), 2500);
    return () => clearTimeout(timer);
  }, [notice]);

  // Exports read oldest first, like a log.
  const copy = async (kind: 'CSV' | 'Markdown') => {
    const rows = [...history].reverse();
    const ok = await copyText(kind === 'CSV' ? toCsv(rows) : toMarkdown(rows));
    setNotice(ok ? `Copied ${rows.length} rows as ${kind}` : 'Copy failed: clipboard not available');
  };

  const shown = history.slice(0, VISIBLE_ROWS);
  return (
    <Card
      title="History"
      subtitle={
        history.length === 0
          ? 'Every measured request lands here, kept in this browser.'
          : `${formatInt(history.length)} requests, newest first${history.length > VISIBLE_ROWS ? ` (showing ${VISIBLE_ROWS})` : ''}`
      }
      actions={
        <>
          <span className="copy-notice" role="status" aria-live="polite">
            {notice}
          </span>
          <button type="button" className="button button-small" onClick={() => copy('CSV')} disabled={history.length === 0}>
            Copy CSV
          </button>
          <button type="button" className="button button-small" onClick={() => copy('Markdown')} disabled={history.length === 0}>
            Copy Markdown
          </button>
          <button type="button" className="button button-small button-danger" onClick={onClear} disabled={history.length === 0}>
            Clear
          </button>
        </>
      }
    >
      {history.length > 0 && (
        <TableScroll label="Measured requests" className="history-scroll">
          <table className="data-table history-table">
            <thead>
              <tr>
                <th scope="col">Time</th>
                <th scope="col">Dataset</th>
                <th scope="col">Variant</th>
                <th scope="col" className="num">Rows</th>
                <th scope="col" className="num"><Term term="endToEnd">End to end</Term></th>
                <th scope="col" className="num"><Term term="ttfb">TTFB</Term></th>
                <th scope="col" className="num"><Term term="download">Download</Term></th>
                <th scope="col" className="num"><Term term="decode">Decode</Term></th>
                <th scope="col" className="num"><Term term="wire">Wire</Term></th>
                <th scope="col" className="num"><Term term="payload">Payload</Term></th>
                <th scope="col">HTTP</th>
                <th scope="col">Target</th>
              </tr>
            </thead>
            <tbody>
              {shown.map((m) => (
                <tr key={m.id}>
                  <td className="nowrap">
                    {timeOfDay(m.at)}
                    {KIND_BADGE[m.kind] && <span className="badge">{KIND_BADGE[m.kind]}</span>}
                  </td>
                  <td className="nowrap">{m.dataset}</td>
                  <td className="nowrap">
                    <FormatTag format={m.format}>{m.variant}</FormatTag>
                  </td>
                  <td className="num">{formatInt(m.rowCount)}</td>
                  <td className="num strong">{formatMs(m.endToEndMs)}</td>
                  <td className="num">{formatMs(m.ttfbMs)}</td>
                  <td className="num">{formatMs(m.downloadMs)}</td>
                  <td className="num">{formatMs(m.decodeMs)}</td>
                  <td className="num">{formatBytes(m.wireBytes)}</td>
                  <td className="num">{formatBytes(m.payloadBytes)}</td>
                  <td>{m.contentEncoding ?? 'none'}</td>
                  <td className="nowrap">
                    {m.target}
                    {m.region && <span className="muted"> · {m.region}</span>}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </TableScroll>
      )}
    </Card>
  );
}
