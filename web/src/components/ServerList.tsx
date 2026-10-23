import { type FormEvent, type Ref, useId, useState } from 'react';
import type { ServersState } from '../hooks/useServers';
import { errorMessage } from '../lib/errors';
import { formatMs } from '../lib/format';
import { type SavedServer, SERVERS_PARAM, serverBase, serverDetail, serverLabel } from '../lib/servers';
import { InfoTip } from './InfoTip';

interface ServerRowProps {
  server: SavedServer;
  servers: ServersState;
  /** A run is in progress: the list is read-only. */
  busy: boolean;
}

interface ServerListProps extends Omit<ServerRowProps, 'server'> {
  /** The list's heading, which takes the focus when the hero's "Choose servers" opens the list. */
  headingRef?: Ref<HTMLHeadingElement>;
}

function ServerRow({ server, servers, busy }: ServerRowProps) {
  const status = servers.status(server.base);
  const region = status.info?.server.region ?? null;
  const label = serverLabel(server, region);
  const detail = serverDetail(label, region, server.base);
  const state = status.loading ? 'loading' : status.info ? 'ok' : 'error';
  const { rtt } = status;
  const rttText = rtt.measuring ? 'measuring…' : rtt.error ? 'failed' : formatMs(rtt.medianMs);

  return (
    <li className="server-row">
      <label className="server-pick">
        <input
          type="checkbox"
          checked={server.selected}
          disabled={busy}
          onChange={(e) => servers.select(server.base, e.target.checked)}
        />
        <span className={`status-dot status-${state}`} aria-hidden="true" />
        <span className="server-name">
          <span className="server-name-label">{label}</span>
          {detail && <span className="server-name-detail">{detail}</span>}
        </span>
      </label>
      <span className="server-rtt" title={rtt.error ?? undefined}>
        <span className="chip-label">RTT</span> {server.selected ? rttText : '–'}
      </span>
      <span className="server-actions">
        <button
          type="button"
          className="icon-button"
          onClick={() => servers.refresh(server.base)}
          disabled={busy}
          aria-label={`Check ${label} again`}
          title="Check again"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4v4.5H15" />
          </svg>
        </button>
        {server.base !== '' && (
          <button
            type="button"
            className="button button-small button-ghost"
            onClick={() => servers.remove(server.base)}
            disabled={busy}
            aria-label={`Remove ${label}`}
          >
            Remove
          </button>
        )}
      </span>
      {status.error && !status.loading && <p className="server-error">Not reachable: {status.error}</p>}
    </li>
  );
}

/** The servers Compare servers runs against: add, remove and tick them. */
export function ServerList({ servers, busy, headingRef }: ServerListProps) {
  const urlId = useId();
  const labelId = useId();
  const [draft, setDraft] = useState('');
  const [labelDraft, setLabelDraft] = useState('');
  const [invalid, setInvalid] = useState<string | null>(null);
  const ticked = servers.list.filter((s) => s.selected).length;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    try {
      if (draft.trim() === '') throw new Error('Enter a full URL such as https://my-service.run.app');
      servers.add(serverBase(draft, location.origin), labelDraft.trim() || undefined);
      setDraft('');
      setLabelDraft('');
      setInvalid(null);
    } catch (err) {
      setInvalid(errorMessage(err));
    }
  };

  return (
    <section className="server-list" aria-labelledby={`${urlId}-title`}>
      <h3 id={`${urlId}-title`} className="server-list-title" ref={headingRef} tabIndex={-1}>
        Servers to compare
        <span className="server-list-count">{ticked} ticked</span>
        <InfoTip term="rtt" label="RTT" />
      </h3>
      {servers.skipped.length > 0 && (
        <p className="field-error">
          Skipped from the link: {servers.skipped.map((entry) => `“${entry}”`).join(', ')}. Each server needs a full URL
          such as https://my-service.run.app.
        </p>
      )}
      <ul className="server-rows">
        {servers.list.map((server) => (
          <ServerRow key={server.base} server={server} servers={servers} busy={busy} />
        ))}
      </ul>
      <form className="server-add" onSubmit={submit}>
        <label htmlFor={urlId} className="sr-only">
          Server URL
        </label>
        <input
          id={urlId}
          type="url"
          inputMode="url"
          placeholder="Another server, e.g. https://playground-xxxx.a.run.app"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          aria-invalid={invalid !== null}
          disabled={busy}
        />
        <label htmlFor={labelId} className="sr-only">
          Label (optional)
        </label>
        <input
          id={labelId}
          type="text"
          className="server-add-label"
          placeholder="Label (optional)"
          value={labelDraft}
          onChange={(e) => setLabelDraft(e.target.value)}
          disabled={busy}
        />
        <button type="submit" className="button" disabled={busy}>
          Add server
        </button>
      </form>
      {invalid && <p className="field-error">{invalid}</p>}
      <p className="field-hint">
        Tick two or more, then press <strong>Compare servers</strong>: the same requests go to each, interleaved. A link
        to this page with <code>?{SERVERS_PARAM}=URL,URL</code> adds servers too and ticks only those. Ticked servers get
        their RTT measured.
      </p>
    </section>
  );
}
