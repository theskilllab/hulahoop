import { useState } from "react";
import { probe, saveRemote, signIn, signUp, useLocalServer } from "../api";

type Mode = "signin" | "signup" | "token";

/**
 * Where the work runs. Hulahoop's server owns the repository, the database, and
 * the runner, so the desktop only needs an address and a token: point it at a
 * machine with nix, direnv, jj, and the agent CLIs and the loop runs there.
 */
export default function Connect({
  onConnected,
  onCancel,
  failure,
}: {
  onConnected: () => void;
  onCancel?: () => void;
  /** Why the last attempt did not get as far as the board. */
  failure?: string | null;
}) {
  const [url, setUrl] = useState("http://");
  const [token, setToken] = useState("");
  const [mode, setMode] = useState<Mode>("signin");
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [invite, setInvite] = useState("");
  const [error, setError] = useState<string | null>(failure ?? null);
  const [busy, setBusy] = useState(false);

  async function submit() {
    setBusy(true);
    setError(null);
    try {
      if (mode === "signin") {
        await signIn(url, email, password);
      } else if (mode === "signup") {
        await signUp(url, { name, email, password, invite: invite.trim() });
      } else {
        await probe(url, token);
        saveRemote(url, token);
      }
      onConnected();
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    } finally {
      setBusy(false);
    }
  }

  const ready =
    mode === "token"
      ? token.trim() !== ""
      : email.trim() !== "" && password !== "" && (mode === "signin" || name.trim() !== "");

  return (
    <main className="connect">
      <h1>connect to a server</h1>
      <p className="hint">
        The server runs the agents, so this machine needs the app and nothing else. Sign in with
        your account, or use the server token from <code>connection.json</code> on the host.
      </p>

      <div className="seg" role="tablist" aria-label="How to connect">
        {(["signin", "signup", "token"] as const).map((option) => (
          <button
            key={option}
            type="button"
            role="tab"
            aria-selected={mode === option}
            onClick={() => { setMode(option); setError(null); }}
          >
            {option === "signin" ? "sign in" : option === "signup" ? "sign up" : "server token"}
          </button>
        ))}
      </div>

      <form
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <label>
          <span>server</span>
          <input value={url} onChange={(event) => setUrl(event.target.value)} spellCheck={false} />
        </label>
        {mode === "token" ? (
          <label>
            <span>token</span>
            <input
              value={token}
              onChange={(event) => setToken(event.target.value)}
              spellCheck={false}
              placeholder="the token in connection.json"
            />
          </label>
        ) : (
          <>
            {mode === "signup" && (
              <label>
                <span>name</span>
                <input value={name} onChange={(event) => setName(event.target.value)} autoComplete="name" />
              </label>
            )}
            <label>
              <span>email</span>
              <input type="email" value={email} onChange={(event) => setEmail(event.target.value)} autoComplete="email" />
            </label>
            <label>
              <span>password</span>
              <input
                type="password"
                value={password}
                onChange={(event) => setPassword(event.target.value)}
                autoComplete={mode === "signin" ? "current-password" : "new-password"}
              />
            </label>
            {mode === "signup" && (
              <label>
                <span>invite code</span>
                <input
                  value={invite}
                  onChange={(event) => setInvite(event.target.value)}
                  spellCheck={false}
                  placeholder="only when registration is not open"
                />
              </label>
            )}
          </>
        )}
        {error && <p className="error">{error}</p>}
        <div className="actions">
          <button type="submit" className="primary" disabled={busy || !ready}>
            {busy ? "connecting…" : mode === "signup" ? "create account" : mode === "signin" ? "sign in" : "connect"}
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
        Your password and the token travel over plain HTTP unless the server sits behind HTTPS, so keep the port on a private network: a
        Tailscale address, or an SSH tunnel like <code>ssh -L 9000:localhost:9000 host</code>.
      </p>
    </main>
  );
}
