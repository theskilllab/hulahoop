import { useEffect, useState } from "react";
import { connect, type RunDiff } from "./api";

/**
 * A small client for `graphql-transport-ws`, the protocol the server speaks on
 * `/graphql/ws`. One socket per subscription: the app holds at most one or two
 * at a time (a run's live diff), so multiplexing would buy nothing.
 *
 * A browser cannot set headers on a WebSocket, so the server token travels in
 * `connection_init`'s payload instead of `Authorization`.
 */

export interface SubscriptionHandlers<T> {
  next: (data: T) => void;
  /**
   * `retryable` is true when the connection failed (it dropped, or the server
   * never answered) and trying again may work; false when the server refused
   * the token or the subscription itself.
   */
  error?: (message: string, retryable: boolean) => void;
  complete?: () => void;
}

type Message =
  | { type: "connection_ack"; payload?: unknown }
  | { type: "ping"; payload?: unknown }
  | { type: "pong"; payload?: unknown }
  | { type: "next"; id: string; payload: { data?: unknown; errors?: { message: string }[] } }
  | { type: "error"; id: string; payload: { message: string }[] }
  | { type: "complete"; id: string };

export const WS_PROTOCOL = "graphql-transport-ws";

/** `http://host:port` → `ws://host:port/graphql/ws`, and https to wss. */
export function socketUrl(serverUrl: string): string {
  return `${serverUrl.replace(/^http/, "ws").replace(/\/+$/, "")}/graphql/ws`;
}

/** How long the server has to acknowledge `connection_init` before the client gives up. */
export const ACK_TIMEOUT_MS = 10_000;

let nextId = 0;

/**
 * Subscribe on a known server. Returns a function that stops the subscription
 * and closes the socket; calling it twice is harmless.
 */
export function subscribeAt<T>(
  serverUrl: string,
  token: string,
  query: string,
  variables: Record<string, unknown>,
  handlers: SubscriptionHandlers<T>,
): () => void {
  const id = String(++nextId);
  const socket = new WebSocket(socketUrl(serverUrl), WS_PROTOCOL);
  let finished = false;
  // A socket that stalls before opening, or a server that never acks, would otherwise say "Connecting…" forever.
  let ackTimer: ReturnType<typeof setTimeout> | undefined = setTimeout(() => {
    ackTimer = undefined;
    if (finished) return;
    finished = true;
    handlers.error?.("No reply from the server", true);
    finish();
  }, ACK_TIMEOUT_MS);
  const stopTimer = () => {
    if (ackTimer !== undefined) clearTimeout(ackTimer);
    ackTimer = undefined;
  };

  const send = (message: Record<string, unknown>) => {
    if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message));
  };
  const finish = () => {
    finished = true;
    stopTimer();
    if (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING) socket.close(1000);
  };

  socket.onopen = () => send({ type: "connection_init", payload: { token } });

  socket.onmessage = (event: MessageEvent) => {
    let message: Message;
    try {
      message = JSON.parse(String(event.data)) as Message;
    } catch {
      return;
    }
    switch (message.type) {
      case "connection_ack":
        stopTimer();
        send({ id, type: "subscribe", payload: { query, variables } });
        break;
      case "ping":
        send({ type: "pong" });
        break;
      case "next":
        if (message.id !== id) break;
        // A result can carry data and errors together: deliver the data, and report the errors.
        if (message.payload.data !== undefined && message.payload.data !== null) {
          handlers.next(message.payload.data as T);
        }
        if (message.payload.errors?.length) {
          handlers.error?.(message.payload.errors.map((entry) => entry.message).join("\n"), false);
        }
        break;
      case "error":
        if (message.id !== id) break;
        finished = true;
        handlers.error?.(message.payload.map((entry) => entry.message).join("\n"), false);
        finish();
        break;
      case "complete":
        if (message.id !== id) break;
        finished = true;
        handlers.complete?.();
        finish();
        break;
    }
  };

  socket.onclose = (event: CloseEvent) => {
    stopTimer();
    if (finished) return;
    finished = true;
    // 4403 is the protocol's "forbidden": the token did not match, and trying again will not change that.
    if (event.code === 4403) handlers.error?.("The server refused the token", false);
    else handlers.error?.(event.reason || "The live connection closed", true);
  };

  return () => {
    if (finished) return;
    if (socket.readyState === WebSocket.OPEN) send({ id, type: "complete" });
    finish();
  };
}

/** Subscribe on the server this app is connected to. */
export async function subscribe<T>(
  query: string,
  variables: Record<string, unknown>,
  handlers: SubscriptionHandlers<T>,
): Promise<() => void> {
  const { url, token } = await connect();
  return subscribeAt(url, token, query, variables, handlers);
}

export const RUN_DIFF_SUBSCRIPTION = `subscription ($runId: UUID!) { runDiff(runId: $runId) { runId patch sha live updatedAt } }`;

/** Waits between reconnects while a run is live: 1 s, 2 s, 5 s, then every 10 s. */
export const RETRY_DELAYS_MS = [1_000, 2_000, 5_000, 10_000];

export interface RunDiffState {
  patch: string;
  /** True while the run still works; the last emission flips it off. */
  live: boolean;
  updatedAt: string | null;
  error: string | null;
  /** The connection dropped while the run was live; another attempt is due. */
  reconnecting: boolean;
}

const idle = (runId: string | null): RunDiffState => ({
  patch: "",
  live: Boolean(runId),
  updatedAt: null,
  error: null,
  reconnecting: false,
});

/**
 * A run's working diff, streamed while it runs. A null run id subscribes to
 * nothing. A dropped connection (a server restart, the Mac sleeping) comes back
 * on its own with backoff until the run's last emission; a refused token or
 * subscription does not.
 */
export function useRunDiff(runId: string | null): RunDiffState {
  const [state, setState] = useState<RunDiffState>(() => idle(runId));

  useEffect(() => {
    setState(idle(runId));
    if (!runId) return;
    let stop: (() => void) | null = null;
    let cancelled = false;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let attempt = 0;
    // The final emission (live: false) or complete arrived: nothing more to wait for.
    let ended = false;

    const again = () => {
      if (cancelled || ended) return;
      stop = null;
      const delay = RETRY_DELAYS_MS[Math.min(attempt, RETRY_DELAYS_MS.length - 1)];
      attempt += 1;
      setState((current) => ({ ...current, error: null, reconnecting: true }));
      retry = setTimeout(open, delay);
    };

    function open() {
      retry = undefined;
      subscribe<{ runDiff: RunDiff }>(RUN_DIFF_SUBSCRIPTION, { runId }, {
        next: (data) => {
          if (cancelled) return;
          attempt = 0;
          if (!data.runDiff.live) ended = true;
          setState({
            patch: data.runDiff.patch,
            live: data.runDiff.live,
            updatedAt: data.runDiff.updatedAt,
            error: null,
            reconnecting: false,
          });
        },
        error: (message, retryable) => {
          if (cancelled) return;
          if (retryable) {
            again();
            return;
          }
          setState((current) => ({ ...current, live: false, error: message, reconnecting: false }));
        },
        complete: () => {
          if (cancelled) return;
          ended = true;
          setState((current) => ({ ...current, live: false, reconnecting: false }));
        },
      })
        .then((unsubscribe) => {
          if (cancelled) unsubscribe();
          else stop = unsubscribe;
        })
        .catch(() => again());
    }

    open();
    return () => {
      cancelled = true;
      if (retry !== undefined) clearTimeout(retry);
      stop?.();
    };
  }, [runId]);

  return state;
}
