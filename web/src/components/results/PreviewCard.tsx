import { formatInt } from '../../lib/format';
import type { LatestPreview } from '../../lib/runner';
import { Card, TableScroll } from './common';

/** The first rows of the latest decoded response, whichever format it came in. */
export function PreviewCard({ preview }: { preview: LatestPreview }) {
  const { table } = preview;
  return (
    <Card
      title="Data preview"
      subtitle={
        <>
          First {table.rows.length} of {formatInt(preview.rowCount)} rows of <strong>{preview.dataset}</strong>, decoded
          from <strong>{preview.variant}</strong>
        </>
      }
    >
      {table.rows.length === 0 ? (
        <p className="field-hint">The response had no rows. Try a smaller start id.</p>
      ) : (
        <TableScroll label={`First rows of ${preview.dataset}`} className="preview-scroll">
          <table className="data-table preview-table">
            <thead>
              <tr>
                {table.columns.map((c) => (
                  <th key={c} scope="col">
                    {c}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {table.rows.map((row, i) => (
                <tr key={i}>
                  {row.map((cell, j) => (
                    <td key={table.columns[j]}>{cell === null ? <span className="null">null</span> : cell}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </TableScroll>
      )}
    </Card>
  );
}
