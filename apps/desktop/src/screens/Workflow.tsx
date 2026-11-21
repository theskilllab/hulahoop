import { useEffect, useState } from "react";

import { loadWorkflow, saveWorkflow, type WorkflowDoc } from "../api";

/// The workspace's workflow is a markdown document, so this screen is an editor
/// with the parse result beside it: what the server understood is never a guess.
export default function WorkflowScreen({ workspaceId }: { workspaceId: string }) {
  const [doc, setDoc] = useState<WorkflowDoc | null>(null);
  const [source, setSource] = useState("");
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let live = true;
    loadWorkflow(workspaceId)
      .then((next) => {
        if (!live) return;
        setDoc(next);
        setSource(next.source);
      })
      .catch((problem) => live && setError(problem.message));
    return () => {
      live = false;
    };
  }, [workspaceId]);

  async function save() {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const next = await saveWorkflow(workspaceId, source);
      setDoc(next);
      setNote("saved — the board follows this document");
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    } finally {
      setBusy(false);
    }
  }

  const dirty = doc !== null && source !== doc.source;

  return (
    <section className="screen workflow">
      <header>
        <div>
          <h1>Workflow</h1>
          <p className="dim">
            One document per workspace. Every <code>## status</code> heading is a column and says who
            works it, which agent picks it up and where it leads.
          </p>
        </div>
        <button className="primary" onClick={save} disabled={busy || !dirty}>
          {busy ? "Saving…" : dirty ? "Save workflow" : "Saved"}
        </button>
      </header>

      {error ? <p className="error">{error}</p> : null}
      {note ? <p className="note">{note}</p> : null}

      <div className="workflow-grid">
        <label className="workflow-editor">
          <span className="label">WORKFLOWS.md</span>
          <textarea
            spellCheck={false}
            value={source}
            onChange={(event) => setSource(event.target.value)}
            rows={28}
          />
        </label>

        <div className="workflow-preview">
          <div className="label">what the server reads</div>
          {doc && doc.errors.length > 0 ? (
            <ul className="problems">
              {doc.errors.map((problem) => (
                <li key={problem}>{problem}</li>
              ))}
            </ul>
          ) : null}
          <table>
            <thead>
              <tr>
                <th>column</th>
                <th>owner</th>
                <th>agent</th>
                <th>next</th>
              </tr>
            </thead>
            <tbody>
              {(doc?.sections ?? []).map((section) => (
                <tr key={section.status}>
                  <td>{section.title}</td>
                  <td>
                    <span className={`chip ${section.owner}`}>{section.owner}</span>
                  </td>
                  <td className="mono">{section.agent ?? "—"}</td>
                  <td className="mono">{section.next ?? "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {doc && doc.sections.length === 0 ? (
            <p className="dim">Nothing parsed yet. Start a section with <code>## To do</code>.</p>
          ) : null}
        </div>
      </div>
    </section>
  );
}
