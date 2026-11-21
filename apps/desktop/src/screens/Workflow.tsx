import { useEffect, useMemo, useState } from "react";

import {
  BOARD_STATUSES,
  loadWorkflow,
  saveWorkflow,
  type Agent,
  type IssueStatus,
  type WorkflowDoc,
  type WorkflowSection,
} from "../api";
import Icon from "../Icon";

const OWNERS = ["human", "agent"] as const;

/// Rewrite the status sections of a workflow document, keeping whatever preamble
/// the person wrote above the first `## ` heading. The diagram and the document
/// are the same thing, so editing one has to move the other.
function rewrite(source: string, sections: WorkflowSection[]): string {
  const firstHeading = source.search(/^##\s/m);
  const preamble = firstHeading === -1 ? source.trimEnd() : source.slice(0, firstHeading).trimEnd();
  const body = sections
    .map((section) =>
      [
        `## ${section.title}`,
        `Owner: ${section.owner}`,
        `Agent: ${section.agent ?? "any"}`,
        `Next: ${section.next ?? "none"}`,
        section.body.trim(),
      ]
        .filter((line) => line.length > 0)
        .join("\n"),
    )
    .join("\n\n");
  return `${preamble}\n\n${body}\n`;
}

/// The workflow is one markdown document per workspace. This screen shows it two
/// ways: as a flow you can change, and as the text the server actually reads.
export default function WorkflowScreen({
  workspaceId,
  agents,
}: {
  workspaceId: string;
  agents: Agent[];
}) {
  const [doc, setDoc] = useState<WorkflowDoc | null>(null);
  const [source, setSource] = useState("");
  const [sections, setSections] = useState<WorkflowSection[]>([]);
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
        setSections(next.sections);
      })
      .catch((problem) => live && setError(problem.message));
    return () => {
      live = false;
    };
  }, [workspaceId]);

  const dirty = doc !== null && source !== doc.source;

  function change(next: WorkflowSection[]) {
    setSections(next);
    setSource(rewrite(source, next));
  }

  async function save() {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const next = await saveWorkflow(workspaceId, source);
      setDoc(next);
      setSections(next.sections);
      setNote("saved — the board follows this document");
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    } finally {
      setBusy(false);
    }
  }

  const flow = useMemo(
    () => BOARD_STATUSES.map((status) => sections.find((section) => section.status === status)),
    [sections],
  );

  return (
    <section className="screen workflow">
      <header>
        <div>
          <h1>Workflow</h1>
          <p className="dim">
            One document per workspace. Every <code>## status</code> heading is a column and says who
            works it, which agent picks it up, and where it leads.
          </p>
        </div>
        <button className="primary" onClick={save} disabled={busy || !dirty}>
          {busy ? "Saving…" : dirty ? "Save workflow" : "Saved"}
        </button>
      </header>

      {error ? <p className="error">{error}</p> : null}
      {note ? <p className="note">{note}</p> : null}
      {doc && doc.errors.length > 0 ? (
        <ul className="problems">
          {doc.errors.map((problem) => (
            <li key={problem}>{problem}</li>
          ))}
        </ul>
      ) : null}

      <div className="flow">
        {flow.map((section, index) => (
          <div key={section?.status ?? `missing-${index}`} style={{ display: "contents" }}>
            {section ? (
              <article className={`flow-node ${section.owner === "agent" ? "agent" : ""}`}>
                <div className="spread">
                  <span className="name">{section.title}</span>
                  <span className={`chip ${section.owner}`}>{section.owner}</span>
                </div>
                <div className="owner" role="group" aria-label={`who works ${section.title}`}>
                  {OWNERS.map((owner) => (
                    <button
                      key={owner}
                      className={section.owner === owner ? "on" : ""}
                      onClick={() =>
                        change(
                          sections.map((entry) =>
                            entry.status === section.status ? { ...entry, owner } : entry,
                          ),
                        )
                      }
                    >
                      {owner}
                    </button>
                  ))}
                </div>
                {section.owner === "agent" ? (
                  <select
                    value={section.agent ?? ""}
                    onChange={(event) =>
                      change(
                        sections.map((entry) =>
                          entry.status === section.status
                            ? { ...entry, agent: event.target.value || null }
                            : entry,
                        ),
                      )
                    }
                  >
                    <option value="">any agent on the project</option>
                    {agents.map((agent) => (
                      <option key={agent.id} value={agent.name}>
                        {agent.name}
                      </option>
                    ))}
                  </select>
                ) : null}
                <label className="stack">
                  <span className="label">then</span>
                  <select
                    value={section.next ?? ""}
                    onChange={(event) =>
                      change(
                        sections.map((entry) =>
                          entry.status === section.status
                            ? { ...entry, next: (event.target.value || null) as IssueStatus | null }
                            : entry,
                        ),
                      )
                    }
                  >
                    <option value="">stays here</option>
                    {BOARD_STATUSES.filter((status) => status !== section.status).map((status) => (
                      <option key={status} value={status}>
                        {sections.find((entry) => entry.status === status)?.title ?? status}
                      </option>
                    ))}
                  </select>
                </label>
              </article>
            ) : (
              <article className="flow-node" key={`missing-${index}`}>
                <span className="name dim">{index + 1}. missing</span>
                <span className="dim">
                  The document has no section for this column.
                </span>
              </article>
            )}
            {index < flow.length - 1 ? (
              <span className="flow-arrow" aria-hidden="true">
                <Icon name="chevron" size={14} />
              </span>
            ) : null}
          </div>
        ))}
      </div>

      <div className="workflow-grid">
        <label className="workflow-editor">
          <span className="label">WORKFLOWS.md</span>
          <textarea
            spellCheck={false}
            value={source}
            onChange={(event) => setSource(event.target.value)}
            rows={24}
          />
        </label>

        <div className="workflow-preview">
          <div className="label">what the server reads</div>
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
            <p className="dim">
              Nothing parsed yet. Start a section with <code>## To do</code>.
            </p>
          ) : null}
        </div>
      </div>
    </section>
  );
}
