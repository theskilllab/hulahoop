import { useEffect, useState } from "react";
import { gql, loadConnections, type ConnectorRow } from "../api";
import Icon from "../Icon";
import PageHeader from "../PageHeader";

/** Which harness each provider powers, for the summary at the top. */
const HARNESSES: { adapter: string; name: string; connectors: string[] }[] = [
  { adapter: "pi", name: "Pi", connectors: ["pi"] },
  { adapter: "claude_code", name: "Claude Code", connectors: ["claude-sub", "claude-api"] },
];

const GROUPS = ["Source control", "Agent access"] as const;

/**
 * What the loop signs in with. One column, never wider than 760px: the rows are
 * a list of decisions, and a wide table would make them look like data.
 */
export function Connectors({ onOpenSettings }: { onOpenSettings?: () => void } = {}) {
  const [rows, setRows] = useState<ConnectorRow[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<ConnectorRow | null>(null);

  const load = () =>
    loadConnections()
      .then((connections) => {
        setRows(connections);
        setError(null);
      })
      .catch((problem) =>
        setError(String(problem instanceof Error ? problem.message : problem)),
      );

  useEffect(() => {
    void load();
  }, []);

  const ready = (adapter: string) => {
    const wanted = HARNESSES.find((harness) => harness.adapter === adapter);
    if (!wanted) return false;
    return rows.some((row) => wanted.connectors.includes(row.id) && row.connected);
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
            {HARNESSES.map((harness) =>
              ready(harness.adapter) ? (
                <span key={harness.adapter} className="chip ok">
                  <Icon name="check" size={12} /> {harness.name} ready
                </span>
              ) : (
                <span key={harness.adapter} className="chip warn">
                  {harness.name} needs a connector
                </span>
              ),
            )}
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
                    {row.connected ? (
                      <button
                        className="btn"
                        onClick={() => {
                          void gql(
                            `mutation ($id: String!) { disconnectConnection(id: $id) { id connected } }`,
                            { id: row.id },
                          )
                            .then(load)
                            .catch((problem) =>
                              setError(
                                String(problem instanceof Error ? problem.message : problem),
                              ),
                            );
                        }}
                      >
                        Disconnect
                      </button>
                    ) : (
                      <button className="btn primary" onClick={() => setOpen(row)}>
                        Connect
                      </button>
                    )}
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

    if (row.id === "claude-api") {
      return (
        <>
          <label className="field">
            <span className="field-label">API key</span>
            <input
              className="input mono"
              type="password"
              value={key}
              placeholder="sk-ant-api03-…"
              onChange={(event) => setKey(event.target.value)}
            />
            <span className="dim small">
              Stored sealed on the host and passed to Claude Code as ANTHROPIC_API_KEY.
              Usage is billed to your Anthropic account.
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
