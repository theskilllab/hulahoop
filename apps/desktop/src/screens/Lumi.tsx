import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  applyLumiProposal,
  askLumi,
  dismissLumiProposal,
  loadLumiMessages,
  type LumiMessage,
  type LumiProposal,
  type Workspace,
} from "../api";
import Icon from "../Icon";

/**
 * Lumi, the workspace's assistant: a pill in the corner, a panel over the page.
 *
 * The conversation lives on the server, so the panel only reads and asks. A
 * question lands at once; the answer is a turn of the workspace's own harness,
 * so the panel polls until the reply row arrives. Lumi never changes anything
 * by itself: a reply may carry a proposal, and the ticked items run only when
 * the person presses Apply.
 */
export default function Lumi({
  workspace,
  context,
  open,
  onToggle,
  onApplied,
  onOpenSettings,
}: {
  workspace: Workspace;
  /** What the person is looking at: "Task HUL-3", "the board", a screen name. */
  context: string;
  open: boolean;
  onToggle: (open: boolean) => void;
  /** An applied proposal changed tasks, so the board behind the panel reloads. */
  onApplied: () => void;
  onOpenSettings: () => void;
}) {
  const [messages, setMessages] = useState<LumiMessage[]>([]);
  const [wide, setWide] = useState(false);
  const [text, setText] = useState("");
  const [failure, setFailure] = useState<string | null>(null);
  /** Unticked boxes per proposal message; everything starts ticked. */
  const [unticked, setUnticked] = useState<Record<string, number[]>>({});
  const [applying, setApplying] = useState<string | null>(null);
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const configured = Boolean(workspace.lumiHarness && workspace.lumiModel);

  const refresh = useCallback(async () => {
    try {
      setMessages(await loadLumiMessages(workspace.id));
    } catch {
      // The next poll tries again; a blink of staleness beats an error storm.
    }
  }, [workspace.id]);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), 2500);
    return () => clearInterval(timer);
  }, [refresh]);

  /** The server stores the question at once; a trailing user row means Lumi owes an answer. */
  const thinking = messages.length > 0 && messages[messages.length - 1].role === "user";

  const pending = useMemo(
    () => messages.filter((m) => m.proposal && m.proposalState === "open").length,
    [messages],
  );

  // New words go where the eye already is: the bottom.
  useEffect(() => {
    const body = bodyRef.current;
    if (body) body.scrollTop = body.scrollHeight;
  }, [messages.length, thinking, open]);

  useEffect(() => {
    if (open) setTimeout(() => inputRef.current?.focus(), 0);
  }, [open]);

  async function send(question: string) {
    const trimmed = question.trim();
    if (!trimmed || thinking) return;
    setText("");
    setFailure(null);
    try {
      await askLumi(workspace.id, trimmed, context);
      await refresh();
    } catch (cause) {
      setFailure(String(cause instanceof Error ? cause.message : cause));
    }
  }

  async function apply(message: LumiMessage, proposal: LumiProposal) {
    const skipped = unticked[message.id] ?? [];
    const items = proposal.items.map((_, index) => index).filter((i) => !skipped.includes(i));
    if (!items.length) return;
    setApplying(message.id);
    setFailure(null);
    try {
      await applyLumiProposal(message.id, items);
      await refresh();
      onApplied();
    } catch (cause) {
      setFailure(String(cause instanceof Error ? cause.message : cause));
    } finally {
      setApplying(null);
    }
  }

  async function dismiss(message: LumiMessage) {
    setFailure(null);
    try {
      await dismissLumiProposal(message.id);
      await refresh();
    } catch (cause) {
      setFailure(String(cause instanceof Error ? cause.message : cause));
    }
  }

  function toggleTick(messageId: string, index: number) {
    setUnticked((was) => {
      const skipped = was[messageId] ?? [];
      return {
        ...was,
        [messageId]: skipped.includes(index)
          ? skipped.filter((i) => i !== index)
          : [...skipped, index],
      };
    });
  }

  if (!open) {
    return (
      <button
        className="as-pill"
        onClick={() => onToggle(true)}
        aria-expanded={false}
        aria-label="Open Lumi"
      >
        <span className="as-ic">
          <Icon name="spark" size={18} />
        </span>
        <span>Ask Lumi</span>
        {pending ? (
          <span className="as-badge" aria-label={`${pending} suggestion(s) waiting`}>
            {pending}
          </span>
        ) : (
          <kbd>⌘J</kbd>
        )}
      </button>
    );
  }

  return (
    <aside
      className={wide ? "as-panel wide" : "as-panel"}
      aria-label="Lumi"
      onKeyDown={(event) => {
        if (event.key === "Escape") onToggle(false);
      }}
    >
      <div className="as-h">
        <span className="av bot">
          <Icon name="spark" size={14} />
        </span>
        <div style={{ minWidth: 0, flex: 1 }}>
          <strong>Lumi</strong>
          <span className="muted" style={{ fontSize: "var(--fs-xs)" }}>
            {" "}
            · {workspace.lumiModel ?? "no model yet"}
          </span>
          <div className="muted as-ctx" style={{ fontSize: "var(--fs-xs)" }}>
            Looking at: {context}
          </div>
        </div>
        <button
          className="icon-btn"
          onClick={() => setWide(!wide)}
          aria-label={wide ? "Make Lumi smaller" : "Make Lumi larger"}
          title={wide ? "Smaller" : "Larger"}
        >
          <Icon name={wide ? "shrink" : "expand"} size={15} />
        </button>
        <button
          className="icon-btn"
          onClick={() => onToggle(false)}
          aria-label="Collapse Lumi"
          title="Collapse (Esc)"
        >
          <Icon name="chevdown" size={15} />
        </button>
      </div>

      <div className="as-body" ref={bodyRef} role="log" aria-live="polite">
        {configured ? (
          <Bubble>
            Hi, I&apos;m Lumi. Ask about the board, the agents or a task — I only change things
            after you press Apply. You&apos;re looking at {context}.
          </Bubble>
        ) : (
          <Bubble>
            Lumi has no harness yet, so there is nothing to answer with.{" "}
            <button className="linkbtn" onClick={onOpenSettings}>
              Pick one in Settings
            </button>
            .
          </Bubble>
        )}
        {messages.map((message) =>
          message.role === "user" ? (
            <div key={message.id} className="as-m user">
              <div className="as-c">{message.body}</div>
            </div>
          ) : (
            <div key={message.id} className="as-m bot">
              <span className="av bot">
                <Icon name="spark" size={14} />
              </span>
              <div className="as-c">
                <div className="as-md">{message.body}</div>
                <Proposal
                  message={message}
                  skipped={unticked[message.id] ?? []}
                  busy={applying === message.id}
                  onTick={(index) => toggleTick(message.id, index)}
                  onApply={apply}
                  onDismiss={() => void dismiss(message)}
                />
              </div>
            </div>
          ),
        )}
        {messages.length === 0 && configured ? (
          <div className="as-chips">
            {["What needs my attention?", "Clean up the board", "Which agent is free?"].map(
              (chip) => (
                <button key={chip} className="as-chip" onClick={() => void send(chip)}>
                  {chip}
                </button>
              ),
            )}
          </div>
        ) : null}
        {thinking ? (
          <div className="as-m bot">
            <span className="av bot">
              <Icon name="spark" size={14} />
            </span>
            <div className="as-c muted">Thinking…</div>
          </div>
        ) : null}
        {failure ? <p className="error">{failure}</p> : null}
      </div>

      <div className="as-in">
        <textarea
          ref={inputRef}
          rows={1}
          placeholder="Ask Lumi…"
          aria-label="Message Lumi"
          value={text}
          disabled={!configured}
          onChange={(event) => {
            setText(event.target.value);
            event.target.style.height = "auto";
            event.target.style.height = `${Math.min(event.target.scrollHeight + 2, 108)}px`;
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              void send(text);
            }
          }}
        />
        <button
          className="primary"
          disabled={!configured || !text.trim() || thinking}
          onClick={() => void send(text)}
        >
          Send
        </button>
      </div>
    </aside>
  );
}

function Bubble({ children }: { children: React.ReactNode }) {
  return (
    <div className="as-m bot">
      <span className="av bot">
        <Icon name="spark" size={14} />
      </span>
      <div className="as-c">
        <div className="as-md">{children}</div>
      </div>
    </div>
  );
}

/** The checkboxes of one proposal, or the chip saying what became of it. */
function Proposal({
  message,
  skipped,
  busy,
  onTick,
  onApply,
  onDismiss,
}: {
  message: LumiMessage;
  skipped: number[];
  busy: boolean;
  onTick: (index: number) => void;
  onApply: (message: LumiMessage, proposal: LumiProposal) => Promise<void>;
  onDismiss: () => void;
}) {
  if (!message.proposal) return null;
  let proposal: LumiProposal;
  try {
    proposal = JSON.parse(message.proposal) as LumiProposal;
  } catch {
    return null;
  }
  if (!Array.isArray(proposal.items) || proposal.items.length === 0) return null;
  const settled = message.proposalState !== "open";
  const ticked = proposal.items.length - skipped.length;
  const groups = [...new Set(proposal.items.map((item) => item.group || ""))];

  return (
    <div className="as-prop">
      <strong>{proposal.title}</strong>
      {groups.map((group) => (
        <div key={group} className="as-g">
          {group ? <div className="as-gh">{group}</div> : null}
          {proposal.items.map((item, index) =>
            (item.group || "") === group ? (
              <label key={index} className="as-it">
                <input
                  type="checkbox"
                  checked={!skipped.includes(index)}
                  disabled={settled || busy}
                  onChange={() => onTick(index)}
                />
                <span>{item.text}</span>
              </label>
            ) : null,
          )}
        </div>
      ))}
      {message.proposalState === "applied" ? (
        <span className="chip ok">
          <Icon name="check" size={12} /> Applied
        </span>
      ) : message.proposalState === "dismissed" ? (
        <span className="muted" style={{ fontSize: "var(--fs-xs)" }}>
          Dismissed
        </span>
      ) : (
        <div className="as-row">
          <button
            className="primary"
            disabled={!ticked || busy}
            onClick={() => void onApply(message, proposal)}
          >
            {busy ? "Applying…" : `Apply ${ticked} change${ticked === 1 ? "" : "s"}`}
          </button>
          <button className="ghost" disabled={busy} onClick={onDismiss}>
            Not now
          </button>
        </div>
      )}
    </div>
  );
}
