import type { GlossaryKey } from '../../lib/glossary';
import type { Phase } from '../../lib/stats';
import type { DataFormat } from '../../lib/types';

export interface SeriesMeta {
  label: string;
  color: string;
  term: GlossaryKey;
}

/** Stacking order and colors of the request phases (see --phase-* in tokens.css). */
export const PHASE_META: Record<Phase, SeriesMeta> = {
  db: { label: 'DB', color: 'var(--phase-db)', term: 'db' },
  encode: { label: 'Encode', color: 'var(--phase-encode)', term: 'encode' },
  network: { label: 'Network wait', color: 'var(--phase-network)', term: 'network' },
  download: { label: 'Download', color: 'var(--phase-download)', term: 'download' },
  decode: { label: 'Decode', color: 'var(--phase-decode)', term: 'decode' },
  use: { label: 'Use', color: 'var(--phase-use)', term: 'use' },
  other: { label: 'Other', color: 'var(--phase-other)', term: 'other' },
};

export const FORMAT_COLOR: Record<DataFormat, string> = {
  json: 'var(--format-json)',
  arrow: 'var(--format-arrow)',
  parquet: 'var(--format-parquet)',
};
