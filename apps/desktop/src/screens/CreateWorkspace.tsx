import { useEffect, useRef, useState } from "react";
import { createWorkspace } from "../api";

/** A small sheet from the workspace menu: a name and an issue prefix, nothing else. */
export default function CreateWorkspace({ onCreated, onClose }: {
  onCreated: (id: string) => Promise<void>;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const nameInput = useRef<HTMLInputElement>(null);
  const [name, setName] = useState("");
  const [prefix, setPrefix] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    dialog.current?.showModal();
    nameInput.current?.focus();
    return () => { dialog.current?.close(); };
  }, []);

  const valid = name.trim().length >= 2 && (prefix.trim() === "" || /^[A-Z0-9]{2,5}$/.test(prefix.trim()));

  async function create() {
    if (!valid || busy) return;
    setBusy(true);
    setError(null);
    try {
      const created = await createWorkspace(name.trim(), prefix.trim() || undefined);
      await onCreated(created.id);
      onClose();
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
      setBusy(false);
    }
  }

  return (
    <dialog
      aria-labelledby="create-workspace-title"
      className="settings-dialog create-workspace-dialog"
      ref={dialog}
      onCancel={(event) => { event.preventDefault(); if (!busy) onClose(); }}
      onClick={(event) => { if (!busy && event.target === event.currentTarget) onClose(); }}
    >
      <form
        className="settings-form"
        onSubmit={(event) => { event.preventDefault(); void create(); }}
      >
        <div className="settings-head">
          <h2 id="create-workspace-title">New workspace</h2>
        </div>
        <div className="settings-content">
          <div className="row">
            <label className="field fill"><span className="field-label">Name</span>
              <input ref={nameInput} type="text" value={name} placeholder="Acme"
                onChange={(event) => setName(event.target.value)} /></label>
            <label className="field"><span className="field-label">Issue prefix</span>
              <input type="text" className="narrow-input" maxLength={5} value={prefix} placeholder="ACM"
                onChange={(event) => setPrefix(event.target.value.toUpperCase())} /></label>
          </div>
          <p className="dim small">The prefix names tasks without a project, such as {prefix.trim() || "ACM"}-1. Leave it empty to derive one from the name.</p>
          {error && <p className="error" role="alert">{error}</p>}
        </div>
        <div className="settings-actions">
          <button type="button" onClick={onClose} disabled={busy}>Cancel</button>
          <button type="submit" className="primary" disabled={!valid || busy}>Create</button>
        </div>
      </form>
    </dialog>
  );
}
