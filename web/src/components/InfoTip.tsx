import { GLOSSARY, type GlossaryKey } from '../lib/glossary';
import { useAnchoredTooltip } from './Tooltip';

/** A small "?" button that explains a metric in plain language. */
export function InfoTip({ term, label }: { term: GlossaryKey; label: string }) {
  const tip = useAnchoredTooltip();
  return (
    <button type="button" className="infotip" aria-label={`About ${label}: ${GLOSSARY[term]}`} {...tip(GLOSSARY[term])}>
      ?
    </button>
  );
}

/** A label followed by its explanation button. */
export function Term({ term, children }: { term: GlossaryKey; children: string }) {
  return (
    <span className="term">
      {children}
      <InfoTip term={term} label={children} />
    </span>
  );
}
