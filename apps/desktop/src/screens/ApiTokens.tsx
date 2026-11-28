import { useEffect, useState } from "react";
import { createApiToken, loadApiTokens, revokeApiToken, type ApiToken } from "../api";
import { fmtWhen } from "../components/TaskBits";

/** Personal API tokens: each one calls the API as its owner until it is revoked. */
export default function ApiTokens() {
  const [tokens, setTokens] = useState<ApiToken[]>([]);
  const [name, setName] = useState("");
  const [created, setCreated] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = () => loadApiTokens().then(setTokens);
  const fail = (cause: unknown) => setError(String(cause instanceof Error ? cause.message : cause));

  useEffect(() => {
    reload().catch(fail);
  }, []);

  async function create() {
    setError(null);
    try {
      setCreated(await createApiToken(name.trim()));
      setName("");
      await reload();
    } catch (cause) {
      fail(cause);
    }
  }

  async function revoke(id: string) {
    setError(null);
    try {
      await revokeApiToken(id);
      await reload();
    } catch (cause) {
      fail(cause);
    }
  }

  return (
    <div className="settings-block">
      <span className="field-label">API tokens</span>
      <ul className="stack" aria-label="API tokens">
        {tokens.map((token) => (
          <li className="row" key={token.id}>
            <span className="stack fill">
              <strong>{token.name}</strong>
              <span className="dim small">created {fmtWhen(token.createdAt)}</span>
            </span>
            <button type="button" className="danger" aria-label={`Revoke ${token.name}`} onClick={() => void revoke(token.id)}>
              Revoke
            </button>
          </li>
        ))}
      </ul>
      <form className="row" onSubmit={(event) => { event.preventDefault(); void create(); }}>
        <label className="field fill">
          <span className="field-label">Token name</span>
          <input value={name} onChange={(event) => setName(event.target.value)} placeholder="my laptop" />
        </label>
        <button type="submit" className="secondary" disabled={!name.trim()}>Create token</button>
      </form>
      {created && (
        <p className="small">
          Copy it now, it is shown once: <code className="mono">{created}</code>
        </p>
      )}
      {error && <p className="error" role="alert">{error}</p>}
      <p className="dim small">
        Send it as <span className="mono">Authorization: Bearer …</span> to <span className="mono">/graphql</span>. It acts as you.
      </p>
    </div>
  );
}
