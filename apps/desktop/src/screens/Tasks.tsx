import { useCallback, useEffect, useMemo, useState, type MouseEvent } from "react";
import {
  moveIssue,
  updateIssue,
  type Agent,
  type IssueStatus,
  type IssueSummary,
  type Milestone,
  type Project,
  type Run,
} from "../api";
import ContextMenu, { type MenuItem } from "../components/ContextMenu";
import Seg from "../components/Seg";
import SidePanel from "../components/SidePanel";
import TaskBoard, { FALLBACK_COLUMNS, isDoneStatus, type Column } from "../components/TaskBoard";
import TaskList from "../components/TaskList";
import Transcript from "../components/Transcript";
import { useToast } from "../components/Toast";
import Icon from "../Icon";
import PageHeader from "../PageHeader";
import IssueDetail, { useIssueDetail, type DraftUpdate, type TaskDraft } from "./IssueDetail";

type Layout = "board" | "list";
const LAYOUT_KEY = "hulahoop.tasks.layout";

/** The header filter is "all", "none" (tasks without a project) or a project id. */
function matchesProjectFilter(projectId: string | null, filter: string): boolean {
  if (filter === "all") return true;
  return filter === "none" ? projectId === null : projectId === filter;
}

type Props = {
  workspaceId: string;
  issues: IssueSummary[];
  projects: Project[];
  agents: Agent[];
  milestones: Milestone[];
  columns?: Column[];
  projectFilter: string;
  onProjectFilter: (id: string) => void;
  /** Whether done tasks are loaded (All) or left on the server (Active). */
  showDone: boolean;
  onShowDone: (show: boolean) => void;
  /** Done tasks in the current project filter, loaded or not. */
  doneCount: number;
  /** The task open in the side panel, if any. */
  openId: string | null;
  onOpen: (id: string | null) => void;
  onNew: () => void;
  onChanged: () => void | Promise<void>;
  /** The open task's key and title once its detail has loaded, for Lumi's context. */
  onDetail?: (detail: { id: string; key: string; title: string } | null) => void;
};

/** Tasks as a board or a list, with the open task in a panel from the right. */
export default function Tasks({
  workspaceId,
  issues,
  projects,
  agents,
  milestones,
  columns: declared,
  projectFilter,
  onProjectFilter,
  showDone,
  onShowDone,
  doneCount,
  openId,
  onOpen,
  onNew,
  onChanged,
  onDetail,
}: Props) {
  const [layout, setLayout] = useState<Layout>(() => (localStorage.getItem(LAYOUT_KEY) === "list" ? "list" : "board"));
  const [menu, setMenu] = useState<{ x: number; y: number; issue: IssueSummary } | null>(null);
  const [transcript, setTranscript] = useState<Run | null>(null);
  const [toast, showToast] = useToast();
  const detail = useIssueDetail(openId);
  // Unsent comments and description edits, per task. The panel remounts for
  // each task, so drafts kept inside it were gone after a click on another card.
  const [drafts, setDrafts] = useState<Record<string, TaskDraft>>({});
  const updateDraft = useCallback((id: string, key: keyof TaskDraft, update: (current: string | undefined) => string | undefined) => {
    setDrafts((all) => {
      const current = all[id]?.[key];
      const next = update(current);
      if (next === current) return all;
      const entry: TaskDraft = { ...all[id], [key]: next };
      if (next === undefined || (key === "comment" && next === "")) delete entry[key];
      const copy = { ...all };
      if (Object.keys(entry).length) copy[id] = entry;
      else delete copy[id];
      return copy;
    });
  }, []);

  useEffect(() => localStorage.setItem(LAYOUT_KEY, layout), [layout]);
  useEffect(() => setTranscript(null), [openId]);

  const columns = declared?.length ? declared : FALLBACK_COLUMNS;
  const title = (status: string) => columns.find((column) => column.status === status)?.title ?? status.replace(/_/g, " ");
  const shown = useMemo(
    () => issues.filter((issue) => matchesProjectFilter(issue.projectId, projectFilter)),
    [issues, projectFilter],
  );
  const hiddenDone = showDone ? 0 : doneCount;
  const openCount = shown.filter((issue) => !isDoneStatus(issue.status)).length;

  async function refreshAll() {
    await Promise.all([onChanged(), detail.reload()]);
  }

  async function move(issue: IssueSummary, status: IssueStatus) {
    try {
      await moveIssue(issue.id, status);
      showToast(`${issue.key} → ${title(status)}`);
      await refreshAll();
    } catch (cause) {
      showToast(cause instanceof Error ? cause.message : String(cause), true);
    }
  }

  async function assign(issue: IssueSummary, mine: boolean) {
    try {
      await updateIssue(issue.id, mine ? { assigneeType: "human", assigneeId: "me" } : { assigneeType: null, assigneeId: null });
      showToast(mine ? `${issue.key} assigned to you` : `${issue.key} unassigned`);
      await refreshAll();
    } catch (cause) {
      showToast(cause instanceof Error ? cause.message : String(cause), true);
    }
  }

  function openMenu(issue: IssueSummary, event: MouseEvent<HTMLElement>) {
    event.preventDefault();
    setMenu({ x: event.clientX, y: event.clientY, issue });
  }

  function menuItems(issue: IssueSummary): MenuItem[] {
    const moves = (issue.moves ?? []).filter((status) => status !== issue.status);
    return [
      { label: "Open", hint: "↵", onSelect: () => onOpen(issue.id) },
      {
        label: "Copy ID",
        onSelect: () => {
          void navigator.clipboard?.writeText(issue.key).then(
            () => showToast(`Copied ${issue.key}`),
            () => showToast("Could not copy", true),
          );
        },
      },
      ...(moves.length ? (["separator"] as MenuItem[]) : []),
      ...moves.map((status) => ({ label: `Move to ${title(status)}`, onSelect: () => void move(issue, status) })),
      "separator",
      issue.assignee?.type === "human"
        ? { label: "Unassign", onSelect: () => void assign(issue, false) }
        : { label: "Assign to me", onSelect: () => void assign(issue, true) },
    ];
  }

  const opener = () => (openId ? document.querySelector<HTMLElement>(`[data-issue-id="${openId}"]`) : null);
  const issue = detail.issue && detail.issue.id === openId ? detail.issue : null;
  const summary = shown.find((entry) => entry.id === openId) ?? issues.find((entry) => entry.id === openId) ?? null;
  const agentName = (id: string) => agents.find((agent) => agent.id === id)?.name ?? "Agent";
  const onDraft: DraftUpdate = (key, update) => {
    if (issue) updateDraft(issue.id, key, update);
  };

  useEffect(() => {
    onDetail?.(issue ? { id: issue.id, key: issue.key, title: issue.title } : null);
  }, [onDetail, issue?.id, issue?.key, issue?.title]);

  return (
    <section className="screen fill tasks-screen">
      <PageHeader
        title="Tasks"
        subtitle={`${openCount} open`}
        actions={
          <>
            <select className="select" aria-label="Project" value={projectFilter} onChange={(event) => onProjectFilter(event.target.value)}>
              <option value="all">All projects</option>
              <option value="none">No project</option>
              {projects.map((entry) => (
                <option key={entry.id} value={entry.id}>
                  {entry.prefix ? `${entry.prefix} · ${entry.name}` : entry.name}
                </option>
              ))}
            </select>
            <Seg<Layout>
              label="Layout"
              value={layout}
              onChange={setLayout}
              options={[
                { value: "board", label: "Board" },
                { value: "list", label: "List" },
              ]}
            />
            <Seg<"active" | "all">
              label="Which tasks"
              value={showDone ? "all" : "active"}
              onChange={(value) => onShowDone(value === "all")}
              options={[
                { value: "active", label: "Active", title: "Hide done tasks" },
                {
                  value: "all",
                  label: (
                    <>
                      All{!showDone && doneCount ? <span className="dim"> +{doneCount}</span> : null}
                    </>
                  ),
                  title: doneCount ? `Also load ${doneCount} done tasks` : "Show done tasks too",
                },
              ]}
            />
            <button type="button" className="primary" onClick={onNew} title="New task (⌘N)">
              <Icon name="plus" size={14} /> New task <kbd>⌘N</kbd>
            </button>
          </>
        }
      />

      <div className="tasks-work">
        {layout === "board" ? (
          <TaskBoard
            issues={shown}
            columns={columns}
            selectedId={openId}
            hiddenDone={hiddenDone}
            onOpen={(id) => onOpen(id)}
            onMenu={openMenu}
            onMove={(entry, status) => void move(entry, status)}
            onShowDone={() => onShowDone(true)}
          />
        ) : (
          <TaskList issues={shown} columns={columns} selectedId={openId} onOpen={(id) => onOpen(id)} onMenu={openMenu} />
        )}

        {openId ? (
          <SidePanel
            key={openId}
            label={summary ? `Task ${summary.key}` : "Task"}
            title={
              transcript ? (
                <button type="button" className="linkbtn tx-back" autoFocus onClick={() => setTranscript(null)}>
                  ← Back to {issue?.key ?? summary?.key ?? "Task"}
                </button>
              ) : (
                <span className="mono">{issue?.key ?? summary?.key ?? "Task"}</span>
              )
            }
            onClose={() => onOpen(null)}
            onEscape={() => {
              if (!transcript) return false;
              setTranscript(null);
              return true;
            }}
            returnFocus={opener}
          >
            {issue ? (
              <>
                {transcript ? (
                  <Transcript
                    runId={transcript.id}
                    runStatus={issue.runs.find((run) => run.id === transcript.id)?.status ?? transcript.status}
                    agentName={agentName(transcript.agentId)}
                    issueKey={issue.key}
                    credentialSource={
                      issue.runs.find((run) => run.id === transcript.id)?.credentialSource ?? transcript.credentialSource ?? null
                    }
                  />
                ) : null}
                {/* Stays mounted under the transcript, so drafts survive a look at a run. */}
                <div hidden={Boolean(transcript)}>
                  <IssueDetail
                    issue={issue}
                    draft={drafts[issue.id] ?? {}}
                    onDraft={onDraft}
                    workspaceId={workspaceId}
                    agents={agents}
                    projects={projects}
                    milestones={milestones}
                    columns={columns}
                    onChanged={refreshAll}
                    onOpenTranscript={setTranscript}
                    onSaved={(message) => showToast(message)}
                    onError={(message) => showToast(message, true)}
                  />
                </div>
              </>
            ) : detail.error ? (
              <p className="error">{detail.error}</p>
            ) : (
              <p className="dim small side-panel-loading">Loading task…</p>
            )}
          </SidePanel>
        ) : null}
      </div>

      {menu ? (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          label={`Actions for ${menu.issue.key}`}
          items={menuItems(menu.issue)}
          onClose={() => setMenu(null)}
          returnFocus={() => document.querySelector<HTMLElement>(`[data-issue-id="${menu.issue.id}"]`)}
        />
      ) : null}
      {toast}
    </section>
  );
}
