import { Term } from '../InfoTip';
import type { SeriesMeta } from './meta';

export function Legend({ items }: { items: readonly SeriesMeta[] }) {
  return (
    <ul className="legend">
      {items.map((item) => (
        <li key={item.label}>
          <span className="legend-swatch" style={{ background: item.color }} aria-hidden="true" />
          <Term term={item.term}>{item.label}</Term>
        </li>
      ))}
    </ul>
  );
}
