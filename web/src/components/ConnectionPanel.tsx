import { type FormEvent, type Ref, useId, useState } from 'react';
import type { ServersState } from '../hooks/useServers';
import { normalizeBaseUrl } from '../lib/api';
import { ServerList } from './ServerList';

interface ConnectionPanelProps {
  apiBase: string;
  error: string | null;
  loading: boolean;
  onSave: (base: string) => void;
  onRetry: () => void;
  onClose?: () => void;
  servers: ServersState;
  /** The heading of the server list, for moving the focus to it. */
  serverListRef?: Ref<HTMLHeadingElement>;
  busy: boolean;
}

/**
 * Edits the API base URL, so a local UI can measure a remote (e.g. Cloud Run)
 * server, and the list of servers that Compare servers runs against.
 */
export function ConnectionPanel(props: ConnectionPanelProps) {
  const { apiBase, error, loading, onSave, onRetry, onClose, servers, serverListRef, busy } = props;
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
      <div className="connection-inner">
        <form className="connection-form" onSubmit={submit}>
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
        <ServerList servers={servers} busy={busy} headingRef={serverListRef} />
      </div>
    </div>
  );
}
