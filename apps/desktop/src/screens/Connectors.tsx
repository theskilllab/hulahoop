import { useEffect, useRef, useState, type FormEvent } from "react";
import {
  cancelSignIn,
  gql,
  loadConnections,
  loadSignIn,
  setConnectorPlan,
  startSignIn,
  submitSignInCode,
  type ConnectorRow,
  type SignIn,
} from "../api";
import { useToast } from "../components/Toast";
import Icon from "../Icon";
import PageHeader from "../PageHeader";
import { PI_LOGIN, PI_PROVIDERS } from "../piProviders";

/** Which harness each entry powers, for the summary at the top. */
const HARNESSES: { adapter: string; name: string }[] = [
  { adapter: "pi", name: "Pi" },
  { adapter: "claude_code", name: "Claude Code" },
];

const GROUPS = ["Source control", "Agent access"] as const;

/** Entries that sign in on the provider's own page, and the name the toast uses. */
const SIGN_IN: Record<string, string> = { "claude-sub": "Claude", "codex-sub": "Codex" };

const POLL_MS = 1500;

/** How long a Copy button says "Copied" before it reads "Copy" again. */
const COPIED_MS = 2000;

/** "A", "A and B", "A, B and C". */
const listed = (names: string[]) =>
  names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;

/** The Pi providers' model names that `rows` sign in, in entry order, each once. */
const piModels = (rows: ConnectorRow[]) => [
  ...new Set(rows.flatMap((row) => (PI_PROVIDERS[row.id] ? [PI_PROVIDERS[row.id].models] : []))),
];

const message = (problem: unknown) => String(problem instanceof Error ? problem.message : problem);

/**
 * What the loop signs in with. One column, never wider than 760px: the rows are
 * a list of decisions, and a wide table would make them look like data.
 */
export function Connectors({ onOpenSettings }: { onOpenSettings?: () => void } = {}) {
  const [rows, setRows] = useState<ConnectorRow[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<ConnectorRow | null>(null);
  const [signingIn, setSigningIn] = useState<ConnectorRow | null>(null);
  const [toast, showToast] = useToast();

  const load = () =>
    loadConnections()
      .then((connections) => {
        setRows(connections);
        setError(null);
      })
      .catch((problem) => setError(message(problem)));

  useEffect(() => {
    void load();
  }, []);

  /**
   * What the summary chip says after the harness name, or null when nothing
   * powers it. Pi with only some providers' entries (and no Pi login) is ready
   * for those providers' models only.
   */
  const readiness = (adapter: string): string | null => {
    const serving = rows.filter((row) => row.harnesses.includes(adapter));
    const on = serving.filter((row) => row.connected);
    if (!on.length) return null;
    if (adapter !== "pi" || on.some((row) => row.id === PI_LOGIN)) return "ready";
    const have = piModels(on);
    return have.length === piModels(serving).length ? "ready" : `ready for ${listed(have)} models`;
  };

  // The select moves at once; a refusal puts back what the server holds.
  const choosePlan = (row: ConnectorRow, plan: string) => {
    setRows((list) => list.map((entry) => (entry.id === row.id ? { ...entry, plan: plan || null } : entry)));
    setConnectorPlan(row.id, plan || null)
      .then((saved) => {
        setRows((list) => list.map((entry) => (entry.id === saved.id ? saved : entry)));
        setError(null);
      })
      .catch((problem) => {
        setError(message(problem));
        void loadConnections().then(setRows).catch(() => undefined);
      });
  };

  const connected = rows.filter((row) => row.connected).length;

  return (
    <section className="screen fill">
      <PageHeader
        title="Connectors"
        subtitle={`${connected} of ${rows.length} connected`}
      />

      {error ? <p className="error">{error}</p> : null}

      <div className="cn-wrap">
        <div className="cn-summary">
          <span className="field-label">Agent harnesses</span>
          <div className="cn-sum">
            {HARNESSES.map((harness) => {
              const state = readiness(harness.adapter);
              return state ? (
                <span key={harness.adapter} className="chip ok">
                  <Icon name="check" size={12} /> {harness.name} {state}
                </span>
              ) : (
                <span key={harness.adapter} className="chip warn">
                  {harness.name} needs a connector
                </span>
              );
            })}
          </div>
          <p className="dim small">
            A harness becomes available when at least one connector that powers it is
            connected.
          </p>
        </div>

        {GROUPS.map((group) => (
          <section key={group} className="cn-sec">
            <span className="field-label">{group}</span>
            <div className="cn-list">
              {rows
                .filter((row) => row.group === group)
                .map((row) => (
                  <div key={row.id} className={row.connected ? "cn-row on" : "cn-row"}>
                    <span className="cn-ic">
                      <Icon name={row.icon} size={16} />
                    </span>
                    <div className="cn-main">
                      <strong>{row.name}</strong>
                      <span className="dim small">
                        {row.connected
                          ? `${row.label ?? "stored"} · ${row.kind.replaceAll("_", " ")}`
                          : row.desc}
                      </span>
                      <small>{row.enables}</small>
                      {row.connected && row.plans?.length ? (
                        <label className="cn-plan">
                          <span className="dim small">Plan</span>
                          <select
                            className="input small"
                            value={row.plan ?? ""}
                            onChange={(event) => choosePlan(row, event.target.value)}
                          >
                            <option value="">Choose your plan</option>
                            {row.plans.map((plan) => (
                              <option key={plan.id} value={plan.id}>
                                {plan.name}
                              </option>
                            ))}
                          </select>
                        </label>
                      ) : null}
                    </div>
                    <span className={row.connected ? "chip ok" : "chip"}>
                      {row.connected ? (
                        <>
                          <Icon name="check" size={12} /> Connected
                        </>
                      ) : (
                        "Not connected"
                      )}
                    </span>
                    <div className="cn-actions">
                      {row.connected ? (
                        <button
                          type="button"
                          className="btn"
                          onClick={() => {
                            void gql(
                              `mutation ($id: String!) { disconnectConnection(id: $id) { id connected } }`,
                              { id: row.id },
                            )
                              .then(load)
                              .catch((problem) => setError(message(problem)));
                          }}
                        >
                          Disconnect
                        </button>
                      ) : SIGN_IN[row.id] ? (
                        <>
                          {row.id === "claude-sub" ? (
                            <button type="button" className="btn" onClick={() => setOpen(row)}>
                              Use a setup token
                            </button>
                          ) : null}
                          <button
                            type="button"
                            className="btn primary"
                            aria-label={`Sign in to ${SIGN_IN[row.id]}`}
                            onClick={() => setSigningIn(row)}
                          >
                            Sign in
                          </button>
                        </>
                      ) : (
                        <button type="button" className="btn primary" onClick={() => setOpen(row)}>
                          Connect
                        </button>
                      )}
                    </div>
                  </div>
                ))}
            </div>
          </section>
        ))}
      </div>

      {open ? (
        <ConnectDialog
          row={open}
          onClose={() => setOpen(null)}
          onDone={() => {
            setOpen(null);
            void load();
          }}
          onError={setError}
          onOpenSettings={
            onOpenSettings
              ? () => {
                  setOpen(null);
                  onOpenSettings();
                }
              : undefined
          }
        />
      ) : null}

      {signingIn ? (
        <SignInDialog
          row={signingIn}
          onClose={() => setSigningIn(null)}
          onDone={() => {
            showToast(`Signed in to ${SIGN_IN[signingIn.id] ?? signingIn.name}`);
            setSigningIn(null);
            void load();
          }}
        />
      ) : null}
      {toast}
    </section>
  );
}

/** The form each entry needs, which is the only thing that differs between them. */
function ConnectDialog({
  row,
  onClose,
  onDone,
  onError,
  onOpenSettings,
}: {
  row: ConnectorRow;
  onClose: () => void;
  onDone: () => void;
  onError: (message: string | null) => void;
  /** Where a person adds their own GitHub token. */
  onOpenSettings?: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [appId, setAppId] = useState("");
  const [pem, setPem] = useState("");
  const [key, setKey] = useState("");
  const [token, setToken] = useState("");
  const [contents, setContents] = useState("");

  const run = (document: string, variables: Record<string, unknown>) => {
    setBusy(true);
    onError(null);
    void gql(document, variables)
      .then(onDone)
      .catch((problem) => onError(String(problem instanceof Error ? problem.message : problem)))
      .finally(() => setBusy(false));
  };

  const body = () => {
    if (row.id === "github") {
      return (
        <>
          <p className="dim small">
            The workspace connects through a GitHub App, which pushes branches and opens pull requests.
            To review and merge as yourself, add your own token in{" "}
            {onOpenSettings ? (
              <button type="button" className="link" onClick={onOpenSettings}>
                Settings → Personal
              </button>
            ) : (
              "Settings → Personal"
            )}
            .
          </p>
          <label className="field">
            <span className="field-label">App ID</span>
            <input
              type="text"
              className="input mono"
              value={appId}
              placeholder="1042517"
              onChange={(event) => setAppId(event.target.value)}
            />
          </label>
          <label className="field">
            <span className="field-label">Private key</span>
            <textarea
              className="textarea mono"
              rows={5}
              value={pem}
              placeholder={"-----BEGIN RSA PRIVATE KEY-----\n…\n-----END RSA PRIVATE KEY-----"}
              onChange={(event) => setPem(event.target.value)}
            />
            <span className="dim small">
              Paste the .pem from the app's settings. It is sealed before it is stored.
            </span>
          </label>
          <button
            className="primary"
            disabled={busy || !appId.trim() || !pem.trim()}
            onClick={() =>
              run(`mutation ($appId: String!, $key: String!) { createGithubApp(appId: $appId, privateKey: $key) { id connected } }`, {
                appId: appId.trim(),
                key: pem,
              })
            }
          >
            Connect
          </button>
        </>
      );
    }

    if (row.id === "claude-sub") {
      return (
        <>
          <p className="dim small">
            On the host, run <code>claude setup-token</code> and follow the browser login.
            It prints one token; paste it here. The token lasts about a year and replaces
            the session file the CLI would otherwise rotate on you.
          </p>
          <label className="field">
            <span className="field-label">Token</span>
            <input
              className="input mono"
              type="password"
              value={token}
              placeholder="sk-ant-oat01-…"
              onChange={(event) => setToken(event.target.value)}
            />
          </label>
          <details className="cn-advanced">
            <summary className="dim small">Or paste a credentials file</summary>
            <label className="field">
              <span className="field-label">~/.claude/.credentials.json</span>
              <textarea
                className="textarea mono"
                rows={4}
                value={contents}
                onChange={(event) => setContents(event.target.value)}
              />
              <span className="dim small">
                Copied into each run's scratch home. It expires within the hour and its
                refresh rotates, so the token above is the better answer.
              </span>
            </label>
            <button
              className="btn"
              disabled={busy || !contents.trim()}
              onClick={() =>
                run(`mutation ($id: String!, $contents: String!) { setAgentSubscription(id: $id, contents: $contents) { id connected } }`, {
                  id: row.id,
                  contents,
                })
              }
            >
              Save the file
            </button>
          </details>
          <button
            className="primary"
            disabled={busy || !token.trim()}
            onClick={() =>
              run(`mutation ($id: String!, $token: String!) { setAgentToken(id: $id, token: $token) { id connected } }`, {
                id: row.id,
                token: token.trim(),
              })
            }
          >
            Save the token
          </button>
        </>
      );
    }

    if (row.id === "claude-api" || row.id === "opencode-go") {
      const opencode = row.id === "opencode-go";
      return (
        <>
          <label className="field">
            <span className="field-label">API key</span>
            <input
              className="input mono"
              type="password"
              value={key}
              placeholder={opencode ? "Your OpenCode Go key" : "sk-ant-api03-…"}
              onChange={(event) => setKey(event.target.value)}
            />
            <span className="dim small">
              {opencode
                ? "Stored sealed on the host and handed to Pi for OpenCode Go models. Usage counts against your OpenCode Go plan."
                : "Stored sealed on the host and passed to Claude Code as ANTHROPIC_API_KEY, and to Pi for Claude models. Usage is billed to your Anthropic account."}
            </span>
          </label>
          <button
            className="primary"
            disabled={busy || !key.trim()}
            onClick={() =>
              run(`mutation ($id: String!, $key: String!) { setAgentKey(id: $id, key: $key) { id connected } }`, {
                id: row.id,
                key: key.trim(),
              })
            }
          >
            Connect
          </button>
        </>
      );
    }

    return (
      <>
        <p className="dim small">
          Sign in on the host with <code>pi</code> and paste the file it writes, or run
          <code> pi /login</code> for the provider you use. The login stays on this machine.
        </p>
        <label className="field">
          <span className="field-label">~/.pi/agent/auth.json</span>
          <textarea
            className="textarea mono"
            rows={4}
            value={contents}
            onChange={(event) => setContents(event.target.value)}
          />
        </label>
        <button
          className="primary"
          disabled={busy || !contents.trim()}
          onClick={() =>
            run(`mutation ($id: String!, $contents: String!) { setAgentSubscription(id: $id, contents: $contents) { id connected } }`, {
              id: row.id,
              contents,
            })
          }
        >
          Save
        </button>
      </>
    );
  };

  return (
    <div className="dialog" role="dialog" aria-modal="true" aria-label={`Connect ${row.name}`}>
      <div className="dialog-body">
        <h2>Connect {row.name}</h2>
        <p className="dim small">{row.desc}</p>
        {body()}
        <div className="dialog-foot">
          <button className="btn" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}

const LIVE = new Set(["starting", "waiting"]);

/**
 * Sign-in on the provider's own page. The server runs the harness's login and
 * reports what it printed: a page to open, sometimes a device code to type there,
 * sometimes a code to paste back here. This polls until the login ends.
 */
export function SignInDialog({
  row,
  onClose,
  onDone,
  pollMs = POLL_MS,
}: {
  row: Pick<ConnectorRow, "id" | "name">;
  onClose: () => void;
  onDone: () => void;
  pollMs?: number;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [signIn, setSignIn] = useState<SignIn | null>(null);
  // A request that failed, as opposed to a login that failed.
  const [problem, setProblem] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [code, setCode] = useState("");
  const [sending, setSending] = useState(false);
  const [copied, setCopied] = useState<"link" | "code" | null>(null);
  const copiedTimer = useRef<number | undefined>(undefined);
  const latest = useRef<SignIn | null>(null);
  const settled = useRef(false);
  const label = SIGN_IN[row.id] ?? row.name;

  latest.current = signIn;
  const live = signIn !== null && LIVE.has(signIn.state);

  useEffect(() => {
    dialog.current?.showModal();
    return () => {
      window.clearTimeout(copiedTimer.current);
      dialog.current?.close();
      // Leaving the screen mid-login stops the login instead of letting it wait out its timeout.
      const open = latest.current;
      if (!settled.current && open && LIVE.has(open.state)) void cancelSignIn(open.id).catch(() => undefined);
    };
  }, []);

  useEffect(() => {
    let alive = true;
    setSignIn(null);
    setProblem(null);
    setCode("");
    startSignIn(row.id)
      .then((started) => {
        if (alive) setSignIn(started);
        else void cancelSignIn(started.id).catch(() => undefined);
      })
      .catch((failure) => alive && setProblem(message(failure)));
    return () => {
      alive = false;
    };
  }, [row.id, attempt]);

  useEffect(() => {
    if (!signIn || !LIVE.has(signIn.state)) return;
    let alive = true;
    const timer = window.setTimeout(() => {
      loadSignIn(signIn.id)
        .then((next) => {
          if (!alive) return;
          setProblem(null);
          setSignIn(next ?? { ...signIn, state: "failed", error: "The sign-in expired. Try again." });
        })
        .catch((failure) => {
          if (!alive) return;
          // Keep asking: a missed poll is not a failed login.
          setProblem(message(failure));
          setSignIn({ ...signIn });
        });
    }, pollMs);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [signIn, pollMs]);

  useEffect(() => {
    if (signIn?.state !== "done") return;
    settled.current = true;
    onDone();
  }, [signIn?.state]);

  const cancel = () => {
    const open = latest.current;
    if (open && LIVE.has(open.state)) {
      settled.current = true;
      void cancelSignIn(open.id).catch(() => undefined);
    }
    onClose();
  };

  const retry = () => {
    const open = latest.current;
    if (open && LIVE.has(open.state)) void cancelSignIn(open.id).catch(() => undefined);
    setAttempt((count) => count + 1);
  };

  const copy = (text: string, which: "link" | "code") => {
    void navigator.clipboard?.writeText(text).then(
      () => {
        setCopied(which);
        window.clearTimeout(copiedTimer.current);
        copiedTimer.current = window.setTimeout(() => setCopied(null), COPIED_MS);
      },
      () => setProblem("Could not copy. Select the text and copy it yourself."),
    );
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!signIn || !code.trim() || sending) return;
    setSending(true);
    setProblem(null);
    submitSignInCode(signIn.id, code.trim())
      .then(setSignIn)
      .catch((failure) => setProblem(message(failure)))
      .finally(() => setSending(false));
  };

  const failed = signIn?.state === "failed" || signIn?.state === "cancelled" || (!signIn && problem !== null);

  return (
    <dialog
      aria-labelledby="signin-title"
      className="settings-dialog signin-dialog"
      ref={dialog}
      onCancel={(event) => {
        event.preventDefault();
        cancel();
      }}
    >
      <div className="settings-form">
        <div className="settings-head">
          <h2 id="signin-title">Sign in to {label}</h2>
        </div>
        <div className="settings-content">
          <span className="sr-only" aria-live="polite">
            {copied === "link" ? "Link copied" : copied === "code" ? "Code copied" : ""}
          </span>
          {!signIn && !problem ? (
            <p className="dim small" role="status">
              <span className="pulse" aria-hidden="true" /> Starting the {label} login…
            </p>
          ) : null}

          {signIn && live && !signIn.url && !signIn.userCode ? (
            <p className="dim small" role="status">
              <span className="pulse" aria-hidden="true" /> Waiting for the sign-in page…
            </p>
          ) : null}

          {signIn?.url && live ? (
            <div className="signin-step">
              <span className="field-label">Open the sign-in page</span>
              <div className="signin-line">
                <a className="signin-url mono" href={signIn.url} target="_blank" rel="noreferrer">
                  {signIn.url}
                </a>
                <button type="button" className="btn" onClick={() => copy(signIn.url as string, "link")}>
                  {copied === "link" ? "Copied" : "Copy link"}
                </button>
              </div>
            </div>
          ) : null}

          {signIn?.userCode && live ? (
            <div className="signin-step">
              <span className="field-label" id="signin-code-label">
                Enter this code on that page
              </span>
              <div className="signin-line">
                <output className="signin-code mono" aria-labelledby="signin-code-label">
                  {signIn.userCode}
                </output>
                <button type="button" className="btn" onClick={() => copy(signIn.userCode as string, "code")}>
                  {copied === "code" ? "Copied" : "Copy code"}
                </button>
              </div>
            </div>
          ) : null}

          {signIn?.needsCode && live ? (
            <form className="signin-step" onSubmit={submit}>
              <label className="field">
                <span className="field-label">Paste the code</span>
                <input
                  type="text"
                  className="input mono"
                  value={code}
                  placeholder="code#state"
                  autoComplete="off"
                  spellCheck={false}
                  onChange={(event) => setCode(event.target.value)}
                />
              </label>
              <span className="dim small">
                After you sign in, {label} shows a code. Paste it here to finish.
              </span>
              <div className="row">
                <button type="submit" className="primary" disabled={sending || !code.trim()}>
                  Submit
                </button>
              </div>
            </form>
          ) : null}

          {signIn && live && (signIn.url || signIn.userCode) && !signIn.needsCode ? (
            <p className="dim small" role="status">
              <span className="pulse" aria-hidden="true" /> Waiting for you to finish in the browser…
            </p>
          ) : null}

          {signIn?.state === "failed" ? (
            <p className="error" role="alert">
              {signIn.error ?? "The sign-in failed."}
            </p>
          ) : null}
          {signIn?.state === "cancelled" ? <p className="dim small">The sign-in was cancelled.</p> : null}
          {problem ? (
            <p className="error" role="alert">
              {problem}
            </p>
          ) : null}
        </div>
        <div className="settings-actions">
          <span />
          <div className="row">
            <button type="button" onClick={cancel}>
              {failed ? "Close" : "Cancel"}
            </button>
            {failed ? (
              <button type="button" className="primary" onClick={retry}>
                Retry
              </button>
            ) : null}
          </div>
        </div>
      </div>
    </dialog>
  );
}
