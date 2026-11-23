import { useEffect, useRef, useState } from "react";
import {
  loadModels,
  setWorkspaceLumi,
  updateWorkspace,
  type ModelOption,
  type Workspace,
} from "../api";
import { applyTheme, resolvedTheme, savedPalette, THEME_GROUPS, THEMES } from "../theme";

type Tab = "personal" | "workspace";

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

export default function Settings({ workspace, initialTab, onChanged, onClose }: {
  workspace: Workspace;
  initialTab: Tab;
  onChanged: () => Promise<void>;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const firstTab = useRef<HTMLButtonElement>(null);
  const workspaceTab = useRef<HTMLButtonElement>(null);
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
    (initialTab === "workspace" ? workspaceTab : firstTab).current?.focus();
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

  function switchTab(next: Tab) {
    setTab(next);
    (next === "workspace" ? workspaceTab : firstTab).current?.focus();
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
              switchTab(event.key === "Home" ? "personal" : event.key === "End" ? "workspace"
                : tab === "personal" ? "workspace" : "personal");
            }
          }}>
          <button ref={firstTab} id="settings-personal-tab" role="tab" aria-controls="settings-panel"
            aria-selected={tab === "personal"} tabIndex={tab === "personal" ? 0 : -1}
            onClick={() => switchTab("personal")}>Personal</button>
          <button ref={workspaceTab} id="settings-workspace-tab" role="tab" aria-controls="settings-panel"
            aria-selected={tab === "workspace"} tabIndex={tab === "workspace" ? 0 : -1}
            onClick={() => switchTab("workspace")}>Workspace</button>
        </div>
      </div>
      <div id="settings-panel" className="settings-content" role="tabpanel" aria-labelledby={tab === "personal" ? "settings-personal-tab" : "settings-workspace-tab"}>
        {tab === "personal" ? <Themes /> : (
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
        {tab === "personal" ? <button className="primary" onClick={onClose}>Done</button>
          : <button className="primary" disabled={busy || !wsName.trim() || ((lumiHarness !== (workspace.lumiHarness ?? "pi") || lumiModel !== (workspace.lumiModel ?? "")) && !lumiModel.trim())}
            onClick={() => void save()}>Save</button>}
      </div>
    </dialog>
  );
}
