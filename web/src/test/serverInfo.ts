// `/api/info` payloads in the shape of docs/DESIGN.md, for tests.
import type { ApiInfo, ColumnInfo, DatasetInfo } from '../lib/types';

export const column = (name: string, group: string, arrow_type = 'Int64', dictionary = false): ColumnInfo => ({
  name,
  group,
  arrow_type,
  pg_type: 'x',
  nullable: false,
  dictionary,
});

export function makeDataset(overrides: Partial<DatasetInfo> = {}): DatasetInfo {
  return {
    name: 'orders',
    label: 'Orders (table)',
    kind: 'table',
    relation: 'orders',
    key_column: 'id',
    description: 'One wide table.',
    sql: 'SELECT * FROM orders',
    row_estimate: 1_000_000,
    max_key: 1_000_000,
    bytes: 250_000_000,
    columns: [column('id', 'numeric'), column('total', 'numeric', 'Float64'), column('country', 'text', 'Utf8', true), column('created_at', 'time', 'Timestamp(Microsecond, "UTC")')],
    ...overrides,
  };
}

export const ORDER_LINES = makeDataset({
  name: 'order_lines',
  label: 'Order lines (7-table join view)',
  kind: 'view',
  relation: 'order_line_details',
  key_column: 'line_id',
  row_estimate: 2_500_000,
  max_key: 10_000_005,
  bytes: 512_000_000,
  columns: [
    column('line_id', 'numeric'),
    column('created_at', 'time', 'Timestamp(Microsecond, "UTC")'),
    column('customer_segment', 'text', 'Utf8', true),
    column('line_total', 'numeric', 'Float64'),
  ],
});

export const DAILY_SALES = makeDataset({
  name: 'daily_sales',
  label: 'Daily sales (materialized view)',
  kind: 'materialized_view',
  relation: 'daily_sales',
  row_estimate: 200_000,
  max_key: 200_000,
  bytes: 30_000_000,
  columns: [column('id', 'numeric'), column('day', 'time', 'Date32'), column('region', 'text', 'Utf8', true)],
});

export function makeInfo(overrides: Partial<ApiInfo> = {}): ApiInfo {
  return {
    default_dataset: 'orders',
    datasets: [makeDataset(), ORDER_LINES, DAILY_SALES],
    seed: { state: 'complete', schema_version: 2, orders: 1_000_000, order_items: 2_500_000, finished_at: '2026-10-22T20:00:00Z' },
    max_rows: 1_000_000,
    formats: ['json', 'arrow', 'parquet'],
    ipc_compression: ['none', 'lz4', 'zstd'],
    parquet_compression: ['none', 'snappy', 'gzip', 'zstd', 'lz4', 'brotli'],
    http_compression: ['zstd', 'br', 'gzip'],
    server: { version: '0.2.0', region: null, service: null, revision: null },
    ...overrides,
  };
}
