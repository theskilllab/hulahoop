import { useEffect, useRef, useState } from "react";
import {
  clearGithubToken,
  loadGithubAccount,
  loadModels,
  savedRemote,
  setGithubToken,
  signOut,
  setWorkspaceLumi,
  updateWorkspace,
  type GithubAccount,
  type ModelOption,
  type Workspace,
} from "../api";
import { Avatar, fmtWhen } from "../components/TaskBits";
import ApiTokens from "./ApiTokens";
import Members from "./Members";
import { applyTheme, resolvedTheme, savedPalette, THEME_GROUPS, THEMES } from "../theme";

type Tab = "personal" | "workspace" | "members";
const TABS: Tab[] = ["personal", "workspace", "members"];

function Themes() {
  const [palette, setPalette] = useState(savedPalette());
  const painted = resolvedTheme(palette);

  return (
    <div className="settings-block">
      <span className="field-label">Theme</span>
      <div className="run-opts" role="radiogroup" aria-label="Theme">
        {THEME_GROUPS.map((group) => (
          <div className="th-group" key={group}>
            <div className="th-gh">{group}</div>
            <div className="th-grid">
              {THEMES.filter((theme) => theme.group === group).map((theme) => {
                const on = palette === theme.id;
                const swatch = theme.kind === "auto" && painted === "dark" && theme.darkSwatch
                  ? theme.darkSwatch : theme.swatch;
                return (
                  <label className={on ? "run-opt th-card on" : "run-opt th-card"} key={theme.id}>
                    <input
                      checked={on}
                      name="theme"
                      onChange={() => { setPalette(theme.id); applyTheme(theme.id); }}
                      type="radio"
                      value={theme.id}
                    />
                    <span className="stack">
                      <span className="row">
                        <strong className="fill">{theme.name}</strong>
                        {theme.kind !== "auto" && (
                          <span className="chip">{theme.kind === "light" ? "Light" : "Dark"}</span>
                        )}
                        <span aria-hidden="true" className="th-sw">
                          {swatch.map((color) => <i key={color} style={{ background: color }} />)}
                        </span>
                      </span>
                      <span className="dim small">{theme.blurb}</span>
                    </span>
                  </label>
                );
              })}
            </div>
          </div>
        ))}
      </div>
      <p className="dim small">Saved on this Mac. Match system follows your appearance setting.</p>
    </div>
  );
}

/** Where the person's own GitHub token goes: reviews and merges are theirs, not the App's. */
export function GithubAccountBlock() {
  // undefined while loading; null when there is no token.
  const [account, setAccount] = useState<GithubAccount | null | undefined>(undefined);
  const [replacing, setReplacing] = useState(false);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Could not ask the server: the account stays unknown, which is not the same as no token.
  const [loadError, setLoadError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [confirming, setConfirming] = useState(false);

  useEffect(() => {
    let live = true;
    setLoadError(null);
    loadGithubAccount()
      .then((found) => { if (live) setAccount(found); })
      .catch((cause) => {
        if (!live) return;
        setLoadError(String(cause instanceof Error ? cause.message : cause));
      });
    return () => { live = false; };
  }, [attempt]);

  async function save() {
    if (!token.trim()) return;
    setBusy(true);
    setError(null);
    try {
      setAccount(await setGithubToken(token.trim()));
      setToken("");
      setReplacing(false);
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    } finally {
      setBusy(false);
    }
  }

  async function remove() {
    setBusy(true);
    setError(null);
    try {
      await clearGithubToken();
      setAccount(null);
      setReplacing(false);
      setConfirming(false);
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    } finally {
      setBusy(false);
    }
  }

  const form = account === null || replacing;

  return (
    <div className="settings-block gh-account">
      <span className="field-label">GitHub account</span>
      {account === undefined ? (
        loadError ? (
          <div className="row">
            <p className="error fill" role="alert">Could not check your GitHub account: {loadError}</p>
            <button type="button" className="secondary" onClick={() => setAttempt((count) => count + 1)}>Retry</button>
          </div>
        ) : (
          <p className="dim small">Checking…</p>
        )
      ) : account && !replacing ? (
        <>
          <div className="row gh-who">
            {account.avatarUrl ? <img className="gh-avatar" src={account.avatarUrl} alt="" /> : <Avatar name={account.login} />}
            <span className="stack fill">
              <strong>{account.name ?? account.login}</strong>
              <span className="dim small">
                @{account.login} · <span className="mono">{account.tokenHint}</span> · checked {fmtWhen(account.validatedAt)}
              </span>
            </span>
          </div>
          {account.problems.length > 0 ? (
            <ul className="gh-problems" aria-label="What the token cannot do">
              {account.problems.map((problem) => <li key={problem}>{problem}</li>)}
            </ul>
          ) : null}
          {confirming ? (
            <div className="row" role="group" aria-label="Remove the token">
              <span className="small fill">Remove the token? Reviews and merges stop going to GitHub as you.</span>
              <button type="button" className="danger" disabled={busy} onClick={() => void remove()}>{busy ? "Removing…" : "Remove token"}</button>
              <button type="button" className="ghost" disabled={busy} onClick={() => setConfirming(false)}>Keep</button>
            </div>
          ) : (
            <div className="row">
              <button type="button" className="secondary" disabled={busy} onClick={() => { setReplacing(true); setError(null); }}>Replace</button>
              <button type="button" className="danger" disabled={busy} onClick={() => setConfirming(true)}>Remove</button>
            </div>
          )}
        </>
      ) : null}
      {form ? (
        <form className="row gh-token" onSubmit={(event) => { event.preventDefault(); void save(); }}>
          <label className="field fill">
            <span className="field-label">Personal access token</span>
            <input
              type="password"
              autoComplete="off"
              spellCheck={false}
              placeholder="github_pat_…"
              value={token}
              onChange={(event) => setToken(event.target.value)}
            />
          </label>
          <button type="submit" className="secondary" disabled={busy || !token.trim()}>{busy ? "Checking…" : "Save"}</button>
          {replacing ? <button type="button" className="ghost" disabled={busy} onClick={() => { setReplacing(false); setToken(""); setError(null); }}>Cancel</button> : null}
        </form>
      ) : null}
      {error ? <p className="error" role="alert">{error}</p> : null}
      <p className="dim small">
        {account && !replacing
          ? "Reviews, line comments and merges go to GitHub as you."
          : "Used to review, comment on and merge pull requests as you. Without it, line comments stay in Hulahoop."}{" "}
        A fine-grained token needs Pull requests (read and write), Contents (read and write, to merge), and Checks and Commit statuses (read) on your repositories; a classic token needs <span className="mono">repo</span>. It is stored sealed on the server.
      </p>
    </div>
  );
}

/** Only a connection made by signing in has a session to end; the local one has none. */
function SignOutBlock() {
  if (!savedRemote()) return null;
  return (
    <div className="settings-block">
      <span className="field-label">Session</span>
      <div className="row">
        <button type="button" className="secondary" onClick={() => void signOut().then(() => window.location.reload())}>
          Sign out
        </button>
      </div>
    </div>
  );
}

export default function Settings({ workspace, initialTab, onChanged, onClose }: {
  workspace: Workspace;
  initialTab: Tab;
  onChanged: () => Promise<void>;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const firstTab = useRef<HTMLButtonElement>(null);
  const workspaceTab = useRef<HTMLButtonElement>(null);
  const membersTab = useRef<HTMLButtonElement>(null);
  const [tab, setTab] = useState<Tab>(initialTab);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [wsName, setWsName] = useState(workspace.name);
  const [wsPrefix, setWsPrefix] = useState(workspace.defaultPrefix);
  const [lumiHarness, setLumiHarness] = useState(workspace.lumiHarness ?? "pi");
  const [lumiModel, setLumiModel] = useState(workspace.lumiModel ?? "");
  const [lumiModels, setLumiModels] = useState<ModelOption[]>([]);

  useEffect(() => {
    dialog.current?.showModal();
    tabButton(initialTab).current?.focus();
    return () => { dialog.current?.close(); };
  }, []);

  useEffect(() => {
    let live = true;
    loadModels(lumiHarness)
      .then((models) => {
        if (!live) return;
        setLumiModels(models);
        // A model saved for another harness is foreign to this catalogue; the
        // first entry is a real choice, the stale id is a guaranteed failure.
        setLumiModel((current) =>
          models.length && !models.some((model) => model.id === current) ? models[0].id : current,
        );
      })
      .catch(() => { if (live) setLumiModels([]); });
    return () => { live = false; };
  }, [lumiHarness]);

  function tabButton(name: Tab) {
    return name === "members" ? membersTab : name === "workspace" ? workspaceTab : firstTab;
  }

  function switchTab(next: Tab) {
    setTab(next);
    tabButton(next).current?.focus();
  }

  async function save() {
    setBusy(true);
    setError(null);
    try {
      if (wsName.trim() !== workspace.name || wsPrefix.trim() !== workspace.defaultPrefix) {
        await updateWorkspace(workspace.id, wsName.trim(), wsPrefix.trim());
      }
      if (lumiHarness !== (workspace.lumiHarness ?? "pi") || lumiModel.trim() !== (workspace.lumiModel ?? "")) {
        await setWorkspaceLumi(workspace.id, lumiHarness, lumiModel.trim());
      }
      await onChanged();
      onClose();
    } catch (cause) {
      await onChanged().catch(() => undefined);
      setError(`Some settings may have saved. ${String(cause instanceof Error ? cause.message : cause)}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <dialog
      aria-labelledby="settings-title"
      className="settings-dialog"
      ref={dialog}
      onCancel={(event) => { event.preventDefault(); if (!busy) onClose(); }}
      onClick={(event) => { if (!busy && event.target === event.currentTarget) onClose(); }}
    >
      <div className="settings-head">
        <h2 id="settings-title" className="fill">Settings</h2>
        <div className="seg settings-tabs" role="tablist" aria-label="Settings section"
          onKeyDown={(event) => {
            if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
              event.preventDefault();
              const at = TABS.indexOf(tab);
              const next = event.key === "Home" ? 0 : event.key === "End" ? TABS.length - 1
                : (at + (event.key === "ArrowRight" ? 1 : TABS.length - 1)) % TABS.length;
              switchTab(TABS[next]);
            }
          }}>
          <button ref={firstTab} id="settings-personal-tab" role="tab" aria-controls="settings-panel"
            aria-selected={tab === "personal"} tabIndex={tab === "personal" ? 0 : -1}
            onClick={() => switchTab("personal")}>Personal</button>
          <button ref={workspaceTab} id="settings-workspace-tab" role="tab" aria-controls="settings-panel"
            aria-selected={tab === "workspace"} tabIndex={tab === "workspace" ? 0 : -1}
            onClick={() => switchTab("workspace")}>Workspace</button>
          <button ref={membersTab} id="settings-members-tab" role="tab" aria-controls="settings-panel"
            aria-selected={tab === "members"} tabIndex={tab === "members" ? 0 : -1}
            onClick={() => switchTab("members")}>Members</button>
        </div>
      </div>
      <div id="settings-panel" className="settings-content" role="tabpanel" aria-labelledby={`settings-${tab}-tab`}>
        {tab === "personal" ? (
          <>
            <GithubAccountBlock />
            <ApiTokens />
            <SignOutBlock />
            <Themes />
          </>
        ) : tab === "members" ? (
          <Members workspaceId={workspace.id} />
        ) : (
          <>
            <div className="settings-block">
              <strong>Workspace</strong>
              <div className="row">
                <label className="field fill"><span className="field-label">Name</span>
                  <input type="text" value={wsName} onChange={(event) => setWsName(event.target.value)} /></label>
                <label className="field"><span className="field-label">Issue prefix</span>
                  <input type="text" className="narrow-input" maxLength={5} value={wsPrefix}
                    onChange={(event) => setWsPrefix(event.target.value.toUpperCase())} /></label>
              </div>
              <p className="dim small">Tasks without a project use {workspace.defaultPrefix}-1, {workspace.defaultPrefix}-2…</p>
            </div>
            <div className="settings-block">
              <strong>Lumi</strong>
              <p className="dim small">Choose the harness and model for Ask Lumi. Changes to tasks still require your approval.</p>
              <div className="grid-2">
                <label><span>Harness</span><select value={lumiHarness} onChange={(event) => {
                  const next = event.target.value;
                  setLumiHarness(next);
                  setLumiModel(next === (workspace.lumiHarness ?? "pi") ? (workspace.lumiModel ?? "") : "");
                }}>
                  <option value="pi">Pi</option><option value="claude_code">Claude Code</option>
                  <option value="fake">Fake (development)</option>
                </select></label>
                <label><span>Model</span>{lumiModels.length ? (
                  <select value={lumiModel} onChange={(event) => setLumiModel(event.target.value)}>
                    {!lumiModels.some((model) => model.id === lumiModel) && (
                      <option value={lumiModel}>{lumiModel || "Choose a model"}</option>
                    )}
                    {lumiModels.map((model) => <option key={model.id} value={model.id}>{model.label}</option>)}
                  </select>
                ) : <input type="text" value={lumiModel} placeholder="model id" onChange={(event) => setLumiModel(event.target.value)} />}</label>
              </div>
            </div>
          </>
        )}
        {error && <p className="error" role="alert">{error}</p>}
      </div>
      <div className="settings-actions">
        {tab === "workspace" && <button onClick={onClose} disabled={busy}>Cancel</button>}
        {tab !== "workspace" ? <button className="primary" onClick={onClose}>Done</button>
          : <button className="primary" disabled={busy || !wsName.trim() || ((lumiHarness !== (workspace.lumiHarness ?? "pi") || lumiModel !== (workspace.lumiModel ?? "")) && !lumiModel.trim())}
            onClick={() => void save()}>Save</button>}
      </div>
    </dialog>
  );
}
