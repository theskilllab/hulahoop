import { useState } from "react";
import { probe, saveRemote, useLocalServer } from "../api";

/**
 * Where the work runs. Hulahoop's server owns the repository, the database, and
 * the runner, so the desktop only needs an address and a token: point it at a
 * machine with nix, direnv, jj, and the agent CLIs and the loop runs there.
 */
export default function Connect({ onConnected, onCancel }: { onConnected: () => void; onCancel?: () => void }) {
  const [url, setUrl] = useState("http://");
  const [token, setToken] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function connectTo(url: string, token: string) {
    setBusy(true);
    setError(null);
    try {
      await probe(url, token);
      saveRemote(url, token);
      onConnected();
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="connect">
      <h1>connect to a server</h1>
      <p className="hint">
        The server runs the agents, so this machine needs the app and nothing else. On the host:
      </p>
      <pre>
        hulahoop-server --listen 0.0.0.0 --port 9000 --print-connection{"\n"}
        hulahoop-runner --server http://host:9000 --token &lt;token&gt;
      </pre>

      <form
        onSubmit={(event) => {
          event.preventDefault();
          void connectTo(url, token);
        }}
      >
        <label>
          <span>server</span>
          <input value={url} onChange={(event) => setUrl(event.target.value)} spellCheck={false} />
        </label>
        <label>
          <span>token</span>
          <input
            value={token}
            onChange={(event) => setToken(event.target.value)}
            spellCheck={false}
            placeholder="from --print-connection"
          />
        </label>
        {error && <p className="error">{error}</p>}
        <div className="actions">
          <button type="submit" className="primary" disabled={busy || !token.trim()}>
            {busy ? "connecting…" : "connect"}
          </button>
          <button
            type="button"
            onClick={() => {
              useLocalServer();
              onConnected();
            }}
          >
            use this machine
          </button>
          {onCancel && (
            <button type="button" onClick={onCancel}>
              cancel
            </button>
          )}
        </div>
      </form>
      <p className="hint dim">
        The token travels as a bearer header over plain HTTP, so keep the port on a private network: a
        Tailscale address, or an SSH tunnel like <code>ssh -L 5123:localhost:5123 host</code>.
      </p>
    </main>
  );
}
