import { useEffect, useState } from "react";

import { gql, loadConnections, type Agent, type ConnectorRow, type IssueSummary, type RecentRun, type Skill } from "../api";
import { plural } from "../components/TaskBits";
import { useToast } from "../components/Toast";
import Icon from "../Icon";
import PageHeader from "../PageHeader";
import AgentDialog from "./AgentDialog";
import { DiffTab, LogTab, TranscriptTab } from "./AgentTabs";
import { harnessOf } from "./agentHarness";

interface Preflight {
  agentId: string;
  repoId: string;
  ok: boolean;
  detail: string;
  checkedAt: string;
}

export type AgentState = "running" | "blocked" | "idle";

const startedAt = (run: RecentRun) => run.startedAt ?? "";

/** The run an agent is on: its running one, else the newest it started. */
export function latestRunFor(agentId: string, runs: RecentRun[]): RecentRun | null {
  const mine = runs.filter((run) => run.agentId === agentId).sort((a, b) => startedAt(a).localeCompare(startedAt(b)));
  return mine.filter((run) => run.status === "running").at(-1) ?? mine.at(-1) ?? null;
}

/**
 * Running when a run is live. Blocked when the agent's latest run is on a task that
 * is blocked on a question and nobody else has taken a turn on it since. Idle otherwise.
 */
export function agentState(agentId: string, runs: RecentRun[], issues: IssueSummary[]): AgentState {
  if (runs.some((run) => run.agentId === agentId && run.status === "running")) return "running";
  const latest = latestRunFor(agentId, runs);
  const issue = latest && issues.find((entry) => entry.id === latest.issueId);
  if (!latest || issue?.status !== "blocked" || !issue.blockedBy) return "idle";
  const taken = runs.some((run) => run.issueId === latest.issueId && startedAt(run) > startedAt(latest));
  return taken ? "idle" : "blocked";
}

function StateChip({ state }: { state: AgentState }) {
  if (state === "running")
    return (
      <span className="chip ok">
        <span className="pulse" aria-hidden="true" />
        Running
      </span>
    );
  if (state === "blocked")
    return (
      <span className="chip warn">
        <Icon name="alert" size={12} />
        Blocked
      </span>
    );
  return <span className="chip">Idle</span>;
}

/**
 * Agents are the workers: a name, a harness, a model, the instructions it works by and
 * the skills it carries. The workflow document picks among them by name, so this screen
 * is where a team is assembled — a roster on the left, one agent on the right.
 */
export default function Agents({
  workspaceId,
  agents,
  skills,
  issues,
  runs,
  repoId,
  projectId,
  onChanged,
  onOpenSkills,
  onOpenIssue,
  onOpenConnectors,
}: {
  workspaceId: string;
  agents: Agent[];
  skills: Skill[];
  issues: IssueSummary[];
  /** Recent runs across the workspace; each records its agent and task. */
  runs: RecentRun[];
  repoId: string | null;
  projectId: string | null;
  onChanged: () => Promise<void>;
  onOpenSkills: () => void;
  onOpenIssue: (issueId: string) => void;
  onOpenConnectors: () => void;
}) {
  const [selected, setSelected] = useState<string | null>(agents[0]?.id ?? null);
  const [connections, setConnections] = useState<ConnectorRow[]>([]);
  const [preflight, setPreflight] = useState<Preflight[]>([]);
  const [editing, setEditing] = useState<null | "new" | string>(null);
  const [tab, setTab] = useState<"diff" | "tx" | "log">("diff");
  // A run picked on the Activity log; its transcript shows instead of the current run's.
  const [pickedRun, setPickedRun] = useState<RecentRun | null>(null);
  const [confirmStop, setConfirmStop] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [toast, showToast] = useToast();

  async function loadPreflight() {
    if (!projectId) return;
    try {
      const data = await gql<{ preflight: Preflight[] }>(
        `query ($p: UUID!) { preflight(projectId: $p) { agentId repoId ok detail checkedAt } }`,
        { p: projectId },
      );
      setPreflight(data.preflight);
    } catch {
      /* a workspace without a repository has nothing to probe yet */
    }
  }

  useEffect(() => {
    void loadConnections()
      .then(setConnections)
      .catch(() => setConnections([]));
  }, [workspaceId]);

  useEffect(() => {
    void loadPreflight();
  }, [projectId, agents.length]);

  useEffect(() => {
    if (!selected && agents.length) setSelected(agents[0].id);
    if (selected && !agents.some((agent) => agent.id === selected)) setSelected(agents[0]?.id ?? null);
  }, [agents, selected]);

  function select(id: string) {
    setSelected(id);
    setPickedRun(null);
    setConfirmStop(false);
  }

  const agent = agents.find((entry) => entry.id === selected) ?? null;
  // A harness is usable when any entry that can sign it in is connected.
  const connected = (harness: string) =>
    harness === "" || connections.some((entry) => entry.connected && entry.harnesses.includes(harness));

  const issueOf = (run: RecentRun | null) => (run ? issues.find((entry) => entry.id === run.issueId) : undefined);
  const keyOf = (run: RecentRun) => issueOf(run)?.key ?? "Task";

  const run = agent ? latestRunFor(agent.id, runs) : null;
  const issue = issueOf(run);
  const state = agent ? agentState(agent.id, runs, issues) : "idle";
  const link = agent ? harnessOf(agent.adapter) : undefined;
  const lost = !connected(link?.harness ?? "");
  const check = agent ? preflight.find((entry) => entry.agentId === agent.id) : undefined;
  const transcriptRun = pickedRun ?? run;
  const running = runs.filter((entry) => entry.status === "running");
  const runningCount = agents.filter((entry) => running.some((live) => live.agentId === entry.id)).length;

  async function stop(runId: string) {
    setConfirmStop(false);
    try {
      await gql(`mutation ($runId: UUID!) { cancelRun(runId: $runId) { id status } }`, { runId });
      await onChanged();
    } catch (cause) {
      setFailure(cause instanceof Error ? cause.message : String(cause));
    }
  }

  return (
    <section className="screen agents-screen">
      <PageHeader
        title="Agents"
        subtitle={`${runningCount} running · Local runner`}
        actions={
          <button className="primary" onClick={() => setEditing("new")}>
            <Icon name="plus" size={14} /> New agent
          </button>
        }
      />

      {failure ? <p className="error">{failure}</p> : null}

      <div className="split">
        <div className="roster">
          {agents.map((entry) => {
            const entryLink = harnessOf(entry.adapter);
            const entryIssue = issueOf(latestRunFor(entry.id, runs));
            const entryLost = !connected(entryLink?.harness ?? "");
            const harnessName = entryLink?.name ?? entry.adapter;
            return (
              <button key={entry.id} className="acard" aria-selected={entry.id === selected} onClick={() => select(entry.id)}>
                <span className="row spread">
                  <span className="avatar">
                    <Icon name="bot" size={14} />
                  </span>
                  <strong className="fill truncate">{entry.name}</strong>
                  <StateChip state={agentState(entry.id, runs, issues)} />
                </span>
                <span className="truncate small dim">
                  {entryIssue ? `${entryIssue.key} · ${entryIssue.title}` : "No task assigned"}
                </span>
                <span className={entryLost ? "truncate tiny warn-text" : "truncate tiny dim"}>
                  {entryLost
                    ? `${harnessName} · not connected`
                    : `${harnessName} · ${entry.model || "default model"} · ${plural(entry.skills.length, "skill")}`}
                </span>
              </button>
            );
          })}
          {agents.length === 0 ? (
            <div className="empty">
              <strong>No agents yet</strong>
              <span className="dim small">Create one, then give it a harness that has a connection.</span>
            </div>
          ) : null}
        </div>

        {agent ? (
          <div className="agent-pane">
            <div className="ahead">
              <div className="row spread">
                <span className="avatar">
                  <Icon name="bot" size={16} />
                </span>
                <h2 className="fill truncate">{agent.name}</h2>
                <StateChip state={state} />
              </div>

              <div className="pillrow">
                <span className="chip">{link?.name ?? agent.adapter}</span>
                <span className="chip">{agent.model || "default model"}</span>
                {lost ? <span className="chip warn">Not connected</span> : null}
                <button className="linkbtn" onClick={() => setEditing(agent.id)}>
                  AGENTS.md
                </button>
                {agent.skills.slice(0, 4).map((skill) => (
                  <span className="chip" key={skill.id} title="Skill">
                    {skill.name}
                  </span>
                ))}
                {agent.skills.length > 4 ? <span className="chip">+{agent.skills.length - 4}</span> : null}
              </div>

              <div className="pillrow">
                {run && issue ? (
                  <>
                    <button className="chip info mono" onClick={() => onOpenIssue(issue.id)}>
                      {issue.key}
                    </button>
                    <span className="truncate agent-task">{issue.title}</span>
                    {run.headBookmark ? (
                      <span className="chip mono">
                        <Icon name="branch" size={12} /> {run.headBookmark}
                      </span>
                    ) : null}
                  </>
                ) : (
                  <span className="dim small">No task assigned. Planning a task in To do hands it to an agent.</span>
                )}
              </div>

              {state === "blocked" && issue ? (
                <div className="callout warn" role="status">
                  <span className="fill">{issue.blockedBy}</span>
                  <button className="primary" onClick={() => onOpenIssue(issue.id)}>
                    Answer on {issue.key}
                  </button>
                </div>
              ) : null}

              <div className="row">
                {state === "running" && run ? (
                  confirmStop ? (
                    <>
                      <span className="small">Stop this run?</span>
                      <button className="danger" onClick={() => void stop(run.id)}>
                        Stop run
                      </button>
                      <button className="ghost" onClick={() => setConfirmStop(false)}>
                        Keep running
                      </button>
                    </>
                  ) : (
                    <button className="danger" onClick={() => setConfirmStop(true)}>
                      Stop
                    </button>
                  )
                ) : null}
                {lost ? (
                  <button className="primary" onClick={onOpenConnectors}>
                    Connect {link?.name ?? agent.adapter}
                  </button>
                ) : null}
                <button onClick={() => setEditing(agent.id)}>
                  <Icon name="gear" size={14} /> Edit agent
                </button>
                {repoId && projectId ? (
                  <button
                    onClick={async () => {
                      await gql(
                        `mutation ($agentId: UUID!, $repoId: UUID!) { runPreflight(agentId: $agentId, repoId: $repoId) }`,
                        { agentId: agent.id, repoId },
                      );
                      setTimeout(() => void loadPreflight(), 6000);
                    }}
                  >
                    <Icon name="refresh" size={14} /> Check harness
                  </button>
                ) : null}
                {check ? (
                  <span className={check.ok ? "chip ok" : "chip bad"} title={check.detail}>
                    {check.ok ? "Harness answers" : "Check failed"}
                  </span>
                ) : null}
              </div>
            </div>

            <div className="tabs" role="tablist">
              <button role="tab" aria-selected={tab === "diff"} onClick={() => setTab("diff")}>
                Live diff
              </button>
              <button role="tab" aria-selected={tab === "tx"} onClick={() => setTab("tx")}>
                Transcript
              </button>
              <button role="tab" aria-selected={tab === "log"} onClick={() => setTab("log")}>
                Activity log
              </button>
            </div>

            <div className="tab-body">
              {tab === "diff" ? <DiffTab run={run} blocked={state === "blocked"} agentName={agent.name} /> : null}
              {tab === "tx" ? (
                <TranscriptTab run={transcriptRun} issueKey={transcriptRun ? keyOf(transcriptRun) : ""} agentName={agent.name} />
              ) : null}
              {tab === "log" ? (
                <LogTab
                  runs={runs.filter((entry) => entry.agentId === agent.id).sort((a, b) => startedAt(b).localeCompare(startedAt(a)))}
                  keyOf={keyOf}
                  onOpen={(picked) => {
                    setPickedRun(picked);
                    setTab("tx");
                  }}
                />
              ) : null}
            </div>
          </div>
        ) : (
          <div className="empty">
            <strong>No agent selected</strong>
            <span className="dim small">Pick one on the left, or create one.</span>
          </div>
        )}
      </div>

      {editing ? (
        <AgentDialog
          workspaceId={workspaceId}
          agent={editing === "new" ? null : (agents.find((entry) => entry.id === editing) ?? null)}
          takenNames={agents.filter((entry) => entry.id !== editing).map((entry) => entry.name)}
          skills={skills}
          connected={connected}
          onOpenSkills={() => {
            setEditing(null);
            onOpenSkills();
          }}
          onOpenConnectors={() => {
            setEditing(null);
            onOpenConnectors();
          }}
          onClose={() => setEditing(null)}
          onSaved={async (id, message) => {
            await onChanged();
            if (id) select(id);
            setEditing(null);
            showToast(message);
          }}
        />
      ) : null}
      {toast}
    </section>
  );
}
