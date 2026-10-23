import { GLOSSARY } from '../../lib/glossary';
import { PHASES } from '../../lib/stats';
import { PHASE_META } from '../charts/meta';

/** Background for non-experts: why the formats differ, and what each number means. */
export function HowToRead({ defaultOpen }: { defaultOpen: boolean }) {
  return (
    <details className="card howto" open={defaultOpen}>
      <summary>
        <h2>How to read this</h2>
        <span className="card-subtitle">Why Arrow is usually faster and smaller, in plain words</span>
      </summary>
      <div className="howto-body">
        <div className="howto-grid">
          <div>
            <h3>JSON (Arrow off)</h3>
            <p>
              Every row is a text object that repeats every column name. Numbers and dates are written as text. The
              browser must read every character, turn digits back into numbers and build one JavaScript object per row
              before anything can use the data.
            </p>
          </div>
          <div>
            <h3>Arrow IPC</h3>
            <p>
              Data is sent column by column in the same binary layout it has in memory. The browser wraps those bytes as
              typed arrays (for example a <code>Float64Array</code> of prices) with almost no work, so decoding is close
              to free and scanning a column is fast. Strings and 64-bit numbers become JavaScript values only when read.
            </p>
          </div>
          <div>
            <h3>Parquet</h3>
            <p>
              A columnar file format built for storage: values are encoded and compressed per column, so it is usually
              the smallest on the wire. The price is CPU: the browser decompresses and decodes it (here in WebAssembly)
              before use.
            </p>
          </div>
        </div>
        <p>
          <strong>Decode vs use:</strong> JSON builds every value while it parses; Arrow builds a value only when it is
          read. With <em>Touch data: Every value</em> (the default) every format ends up building every selected value,
          so the end-to-end numbers compare the same work, as for a table view or an export. With <em>One column</em>{' '}
          only one column is read, the column scan (a sum, a chart) where Arrow skips nearly all the work. Compare
          decode + use, not decode alone.
        </p>
        <p>
          <strong>Where it matters:</strong> on a fast local network, the browser’s work and the server’s encoding
          dominate. Over the internet, bytes dominate: HTTP compression shrinks JSON a lot (its cost shows as Compress on
          the server), but Arrow and Parquet still send fewer bytes and the browser still avoids parsing text. Compare
          servers runs the same comparison against a nearby and a distant server side by side to see both effects.
        </p>
        <p>
          <strong>Datasets:</strong> a join view or a live aggregate moves time into PostgreSQL, which is the same for
          every format. The more of the end-to-end time the database takes, the less the format matters; the
          materialized view shows how precomputing the aggregate gives that time back.
        </p>
        <dl className="glossary">
          {PHASES.map((phase) => (
            <div key={phase}>
              <dt>
                <span className="legend-swatch" style={{ background: PHASE_META[phase].color }} aria-hidden="true" />
                {PHASE_META[phase].label}
              </dt>
              <dd>{GLOSSARY[PHASE_META[phase].term]}</dd>
            </div>
          ))}
          <div>
            <dt>Time to first byte</dt>
            <dd>{GLOSSARY.ttfb}</dd>
          </div>
          <div>
            <dt>On the wire</dt>
            <dd>{GLOSSARY.wire}</dd>
          </div>
          <div>
            <dt>Payload</dt>
            <dd>{GLOSSARY.payload}</dd>
          </div>
          <div>
            <dt>Browser work</dt>
            <dd>{GLOSSARY.browser}</dd>
          </div>
          <div>
            <dt>Speed-up</dt>
            <dd>{GLOSSARY.speedup}</dd>
          </div>
        </dl>
      </div>
    </details>
  );
}
