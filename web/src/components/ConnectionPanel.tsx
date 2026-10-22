import { type FormEvent, useId, useState } from 'react';
import { normalizeBaseUrl } from '../lib/api';

interface ConnectionPanelProps {
  apiBase: string;
  error: string | null;
  loading: boolean;
  onSave: (base: string) => void;
  onRetry: () => void;
  onClose?: () => void;
}

/** Edits the API base URL, so a local UI can measure a remote (e.g. Cloud Run) server. */
export function ConnectionPanel({ apiBase, error, loading, onSave, onRetry, onClose }: ConnectionPanelProps) {
  const inputId = useId();
  const [draft, setDraft] = useState(apiBase);
  const [invalid, setInvalid] = useState<string | null>(null);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    try {
      onSave(normalizeBaseUrl(draft));
      setInvalid(null);
    } catch (err) {
      setInvalid(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="connection">
      <form className="connection-inner" onSubmit={submit}>
        {error && !loading && (
          <p className="connection-error" role="alert">
            Could not load <code>/api/info</code>: {error}
          </p>
        )}
        <label htmlFor={inputId}>API base URL</label>
        <div className="connection-row">
          <input
            id={inputId}
            type="url"
            inputMode="url"
            placeholder="Same origin as this page (leave empty)"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            aria-invalid={invalid !== null}
          />
          <button type="submit" className="button button-primary">
            Connect
          </button>
          {apiBase !== '' && (
            <button type="button" className="button" onClick={() => onSave('')}>
              Use same origin
            </button>
          )}
          {error && (
            <button type="button" className="button" onClick={onRetry} disabled={loading}>
              Retry
            </button>
          )}
          {onClose && (
            <button type="button" className="button button-ghost" onClick={onClose}>
              Close
            </button>
          )}
        </div>
        {invalid && <p className="field-error">{invalid}</p>}
        <p className="field-hint">
          For example <code>https://playground-xxxx.a.run.app</code>. Saved in this browser only. The server allows any
          origin and sends <code>Timing-Allow-Origin</code>, so remote measurements stay precise.
        </p>
      </form>
    </div>
  );
}
