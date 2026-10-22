import type { ReactNode, Ref } from 'react';
import type { GlossaryKey } from '../../lib/glossary';
import type { DataFormat } from '../../lib/types';
import { Term } from '../InfoTip';
import { FORMAT_COLOR } from '../charts/meta';

interface CardProps {
  title: string;
  subtitle?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  busy?: boolean;
  className?: string;
  ref?: Ref<HTMLElement>;
}

export function Card({ title, subtitle, actions, children, busy, className, ref }: CardProps) {
  return (
    <section ref={ref} className={`card ${className ?? ''}`} aria-busy={busy || undefined} aria-label={title}>
      <header className="card-header">
        <div>
          <h2>{title}</h2>
          {subtitle && <p className="card-subtitle">{subtitle}</p>}
        </div>
        {actions && <div className="card-actions">{actions}</div>}
      </header>
      {children}
    </section>
  );
}

interface StatProps {
  label: string;
  term: GlossaryKey;
  value: ReactNode;
  detail?: ReactNode;
  tone?: 'good' | 'bad';
  hero?: boolean;
}

export function Stat({ label, term, value, detail, tone, hero }: StatProps) {
  return (
    <div className={`stat${hero ? ' stat-hero' : ''}`}>
      <div className="stat-label">
        <Term term={term}>{label}</Term>
      </div>
      <div className={`stat-value${tone ? ` tone-${tone}` : ''}`}>{value}</div>
      {detail && <div className="stat-detail">{detail}</div>}
    </div>
  );
}

export function FormatTag({ format, children }: { format: DataFormat; children: ReactNode }) {
  return (
    <span className="format-tag">
      <span className="format-dot" style={{ background: FORMAT_COLOR[format] }} aria-hidden="true" />
      {children}
    </span>
  );
}

export function ChartTitle({ children, term, label }: { children?: ReactNode; term?: GlossaryKey; label: string }) {
  return (
    <h3 className="chart-title">
      {term ? <Term term={term}>{label}</Term> : label}
      {children}
    </h3>
  );
}

/**
 * A horizontally scrollable table box. It is focusable and labelled so a
 * keyboard can scroll it in every browser (Safari does not focus scrollers
 * on its own).
 */
export function TableScroll({ label, className, children }: { label: string; className?: string; children: ReactNode }) {
  return (
    <div className={`table-scroll ${className ?? ''}`} role="region" aria-label={label} tabIndex={0}>
      {children}
    </div>
  );
}
