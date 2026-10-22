import { USE_SCOPE_LABELS } from './settings';
import type { Measurement } from './types';

interface ExportColumn {
  header: string;
  value: (m: Measurement) => string | number | null;
}

const round3 = (v: number | null) => (v === null ? null : Math.round(v * 1000) / 1000);

const COLUMNS: ExportColumn[] = [
  { header: 'time', value: (m) => m.at },
  { header: 'target', value: (m) => m.target },
  { header: 'dataset', value: (m) => m.dataset },
  { header: 'kind', value: (m) => m.kind },
  { header: 'variant', value: (m) => m.variant },
  { header: 'rows', value: (m) => m.rowCount },
  { header: 'columns', value: (m) => m.columnCount },
  { header: 'offset', value: (m) => m.offset },
  { header: 'http_encoding', value: (m) => m.contentEncoding ?? 'none' },
  { header: 'cache', value: (m) => m.cache },
  { header: 'end_to_end_ms', value: (m) => round3(m.endToEndMs) },
  { header: 'db_ms', value: (m) => round3(m.dbMs) },
  { header: 'encode_ms', value: (m) => round3(m.encodeMs) },
  { header: 'compress_ms', value: (m) => round3(m.compressMs) },
  { header: 'server_total_ms', value: (m) => round3(m.serverTotalMs) },
  { header: 'ttfb_ms', value: (m) => round3(m.ttfbMs) },
  { header: 'network_wait_ms', value: (m) => round3(m.networkWaitMs) },
  { header: 'download_ms', value: (m) => round3(m.downloadMs) },
  { header: 'decode_ms', value: (m) => round3(m.decodeMs) },
  { header: 'use_ms', value: (m) => round3(m.useMs) },
  { header: 'wire_bytes', value: (m) => m.wireBytes },
  { header: 'payload_bytes', value: (m) => m.payloadBytes },
  { header: 'touch_data', value: (m) => USE_SCOPE_LABELS[m.useScope] },
  { header: 'timing_source', value: (m) => m.timingSource },
];

function csvCell(value: string | number | null): string {
  if (value === null) return '';
  const text = String(value);
  return /[",\n\r]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
}

export function toCsv(items: readonly Measurement[]): string {
  const lines = [COLUMNS.map((c) => c.header).join(',')];
  for (const m of items) lines.push(COLUMNS.map((c) => csvCell(c.value(m))).join(','));
  return `${lines.join('\n')}\n`;
}

function markdownCell(value: string | number | null): string {
  if (value === null) return '';
  return String(value).replace(/\|/g, '\\|').replace(/\r?\n/g, ' ');
}

export function toMarkdown(items: readonly Measurement[]): string {
  const header = `| ${COLUMNS.map((c) => c.header).join(' | ')} |`;
  const divider = `| ${COLUMNS.map(() => '---').join(' | ')} |`;
  const rows = items.map((m) => `| ${COLUMNS.map((c) => markdownCell(c.value(m))).join(' | ')} |`);
  return `${[header, divider, ...rows].join('\n')}\n`;
}
