import { useMemo, useRef } from "react";

import type { RecentRun } from "../api";
import DiffView from "../components/DiffView";
import { parseGitDiff, type DiffFile } from "../components/diff";
import { fmtTokens, fmtWhen } from "../components/TaskBits";
import Transcript from "../components/Transcript";
import { useRunDiff } from "../ws";

const Empty = ({ title, children }: { title: string; children: string }) => (
  <div className="empty">
    <strong>{title}</strong>
    <span className="dim small">{children}</span>
  </div>
);

/** The agent's diff: a file tree on the left (click to scroll), the diff on the right. */
export function DiffTab({ run, blocked, agentName }: { run: RecentRun | null; blocked: boolean; agentName: string }) {
  const { patch, live } = useRunDiff(run?.id ?? null);
  const files = useMemo(() => parseGitDiff(patch), [patch]);
  const view = useRef<HTMLDivElement>(null);

  if (!files.length) {
    return blocked ? (
      <Empty title="Waiting for an answer">{`${agentName} is waiting on your answer before writing code.`}</Empty>
    ) : (
      <Empty title="No changes yet">When an agent runs, its diff streams in here line by line.</Empty>
    );
  }

  const scrollTo = (path: string) => {
    const section = [...(view.current?.querySelectorAll<HTMLElement>(".diff-file") ?? [])].find(
      (node) => node.getAttribute("aria-label") === path,
    );
    section?.scrollIntoView?.({ block: "start" });
  };

  return (
    <div className="diffwrap">
      <ul className="ftree" aria-label="Changed files">
        {files.map((file, index) => (
          <li key={file.path}>
            <button type="button" onClick={() => scrollTo(file.path)} title={file.path}>
              <span className="mono truncate fill">{file.path}</span>
              <span className="mono small">
                <span className="diff-plus">+{file.additions}</span> <span className="diff-minus">−{file.deletions}</span>
              </span>
              <span className="tiny dim">{fileState(files, index, live)}</span>
            </button>
          </li>
        ))}
      </ul>
      <div ref={view} className="diffwrap-view">
        <DiffView files={files} />
      </div>
    </div>
  );
}

/** While the run works, the last file in the patch is the one still being written. */
const fileState = (files: DiffFile[], index: number, live: boolean) =>
  live && index === files.length - 1 ? "writing" : "done";

export function TranscriptTab({ run, issueKey, agentName }: { run: RecentRun | null; issueKey: string; agentName: string }) {
  if (!run) {
    return <Empty title="No active session">{`${agentName} is not working on anything right now. Start a run from a task to follow its transcript here.`}</Empty>;
  }
  return <Transcript runId={run.id} runStatus={run.status} agentName={agentName} issueKey={issueKey} />;
}

/** Every run the agent started, newest first; a row opens that run's transcript. */
export function LogTab({
  runs,
  keyOf,
  onOpen,
}: {
  runs: RecentRun[];
  keyOf: (run: RecentRun) => string;
  onOpen: (run: RecentRun) => void;
}) {
  if (!runs.length) return <Empty title="No runs yet">Every run this agent starts is listed here with its outcome.</Empty>;
  const tokens = (run: RecentRun) => run.tokensIn + run.tokensOut;
  return (
    <div className="log agent-log">
      {runs.map((run) => (
        <button type="button" key={run.id} className="log-row" onClick={() => onOpen(run)}>
          <span className="mono dim">{fmtWhen(run.startedAt)}</span>
          <span>
            {[keyOf(run), run.status.replace(/_/g, " "), tokens(run) ? `${fmtTokens(tokens(run))} tokens` : null]
              .filter(Boolean)
              .join(" · ")}
            {run.status === "failed" && run.error ? <span className="bad small log-error">{run.error}</span> : null}
          </span>
        </button>
      ))}
    </div>
  );
}
