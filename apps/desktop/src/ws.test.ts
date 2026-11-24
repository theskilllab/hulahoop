import { act, renderHook } from "@testing-library/react";
import { createElement, StrictMode, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", () => ({
  connect: vi.fn(async () => ({ url: "http://127.0.0.1:7777", token: "secret" })),
}));

import { ACK_TIMEOUT_MS, socketUrl, subscribeAt, useRunDiff, WS_PROTOCOL } from "./ws";

/** Just enough of a WebSocket to watch what the client sends and to answer it. */
class FakeSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  static last: FakeSocket | null = null;
  static all: FakeSocket[] = [];

  readyState = FakeSocket.CONNECTING;
  sent: Record<string, unknown>[] = [];
  closedWith: number | null = null;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: ((event: { code: number; reason: string }) => void) | null = null;

  constructor(
    readonly url: string,
    readonly protocol: string,
  ) {
    FakeSocket.last = this;
    FakeSocket.all.push(this);
  }

  send(text: string) {
    this.sent.push(JSON.parse(text) as Record<string, unknown>);
  }

  close(code: number) {
    this.closedWith = code;
    this.readyState = FakeSocket.CLOSED;
  }

  open() {
    this.readyState = FakeSocket.OPEN;
    this.onopen?.();
  }

  receive(message: Record<string, unknown>) {
    this.onmessage?.({ data: JSON.stringify(message) });
  }

  drop(code: number, reason = "") {
    this.readyState = FakeSocket.CLOSED;
    this.onclose?.({ code, reason });
  }
}

const QUERY = "subscription ($runId: UUID!) { runDiff(runId: $runId) { patch live } }";

beforeEach(() => {
  vi.stubGlobal("WebSocket", FakeSocket);
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  FakeSocket.last = null;
  FakeSocket.all = [];
});

function start() {
  const handlers = { next: vi.fn(), error: vi.fn(), complete: vi.fn() };
  const stop = subscribeAt("http://127.0.0.1:7777", "secret", QUERY, { runId: "r1" }, handlers);
  const socket = FakeSocket.last!;
  return { handlers, stop, socket };
}

describe("socketUrl", () => {
  it("turns the server's http address into its socket address", () => {
    expect(socketUrl("http://127.0.0.1:7777")).toBe("ws://127.0.0.1:7777/graphql/ws");
    expect(socketUrl("https://hula.example.com/")).toBe("wss://hula.example.com/graphql/ws");
  });
});

describe("subscribeAt", () => {
  it("authenticates in connection_init, subscribes after the ack, and answers pings", () => {
    const { handlers, socket } = start();
    expect(socket.url).toBe("ws://127.0.0.1:7777/graphql/ws");
    expect(socket.protocol).toBe(WS_PROTOCOL);
    socket.open();
    expect(socket.sent).toEqual([{ type: "connection_init", payload: { token: "secret" } }]);

    socket.receive({ type: "connection_ack" });
    const subscribe = socket.sent[1];
    expect(subscribe).toMatchObject({ type: "subscribe", payload: { query: QUERY, variables: { runId: "r1" } } });

    socket.receive({ type: "ping" });
    expect(socket.sent[2]).toEqual({ type: "pong" });

    socket.receive({ type: "next", id: subscribe.id, payload: { data: { runDiff: { patch: "x", live: true } } } });
    expect(handlers.next).toHaveBeenCalledWith({ runDiff: { patch: "x", live: true } });

    // Another subscription's message is not ours.
    socket.receive({ type: "next", id: "other", payload: { data: { runDiff: { patch: "y", live: true } } } });
    expect(handlers.next).toHaveBeenCalledTimes(1);

    socket.receive({ type: "complete", id: subscribe.id });
    expect(handlers.complete).toHaveBeenCalled();
    expect(socket.closedWith).toBe(1000);
    expect(handlers.error).not.toHaveBeenCalled();
  });

  it("reports GraphQL errors from next and error messages", () => {
    const { handlers, socket } = start();
    socket.open();
    socket.receive({ type: "connection_ack" });
    const id = socket.sent[1].id;
    socket.receive({ type: "next", id, payload: { errors: [{ message: "no such run" }] } });
    expect(handlers.error).toHaveBeenLastCalledWith("no such run", false);
    socket.receive({ type: "error", id, payload: [{ message: "bad query" }] });
    expect(handlers.error).toHaveBeenLastCalledWith("bad query", false);
    expect(socket.closedWith).toBe(1000);
  });

  it("says when the server refuses the token", () => {
    const { handlers, socket } = start();
    socket.open();
    socket.drop(4403, "Forbidden");
    expect(handlers.error).toHaveBeenCalledWith("The server refused the token", false);
  });

  it("delivers the data of a result that also carries errors, then reports the errors", () => {
    const { handlers, socket } = start();
    socket.open();
    socket.receive({ type: "connection_ack" });
    const id = socket.sent[1].id;
    socket.receive({ type: "next", id, payload: { data: { runDiff: { patch: "x", live: true } }, errors: [{ message: "partly" }] } });
    expect(handlers.next).toHaveBeenCalledWith({ runDiff: { patch: "x", live: true } });
    expect(handlers.error).toHaveBeenCalledWith("partly", false);
  });

  it("says a dropped connection may come back", () => {
    const { handlers, socket } = start();
    socket.open();
    socket.receive({ type: "connection_ack" });
    socket.drop(1006);
    expect(handlers.error).toHaveBeenCalledWith("The live connection closed", true);
  });

  it("gives up when the server never acknowledges", () => {
    vi.useFakeTimers();
    const { handlers, socket } = start();
    socket.open();
    vi.advanceTimersByTime(ACK_TIMEOUT_MS - 1);
    expect(handlers.error).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(handlers.error).toHaveBeenCalledWith("No reply from the server", true);
    expect(socket.closedWith).toBe(1000);
    // The close that follows is not a second error.
    socket.drop(1000);
    expect(handlers.error).toHaveBeenCalledTimes(1);
  });

  it("stops waiting for the ack once it arrives", () => {
    vi.useFakeTimers();
    const { handlers, socket } = start();
    socket.open();
    socket.receive({ type: "connection_ack" });
    vi.advanceTimersByTime(ACK_TIMEOUT_MS * 2);
    expect(handlers.error).not.toHaveBeenCalled();
  });

  it("sends complete when the caller stops, and stays quiet after", () => {
    const { handlers, socket, stop } = start();
    socket.open();
    socket.receive({ type: "connection_ack" });
    const id = socket.sent[1].id;
    stop();
    expect(socket.sent.at(-1)).toEqual({ id, type: "complete" });
    expect(socket.closedWith).toBe(1000);
    socket.drop(1000);
    stop();
    expect(handlers.error).not.toHaveBeenCalled();
  });

  it("ignores frames that are not JSON", () => {
    const { handlers, socket } = start();
    socket.open();
    socket.onmessage?.({ data: "not json" });
    expect(handlers.next).not.toHaveBeenCalled();
    expect(handlers.error).not.toHaveBeenCalled();
  });
});

/** Lets the hook's connect() settle and its socket appear. */
const settle = () => act(async () => {});

const diff = (patch: string, live: boolean) => ({ runDiff: { runId: "r1", patch, sha: "abc", live, updatedAt: "2026-01-01T00:00:00Z" } });

/** Open the newest socket, ack it, and return its subscription id. */
function ready(socket: FakeSocket) {
  socket.open();
  socket.receive({ type: "connection_ack" });
  return socket.sent[1].id as string;
}

describe("useRunDiff", () => {
  it("follows the run's diff until its last emission, then stays put", async () => {
    vi.useFakeTimers();
    const { result } = renderHook(() => useRunDiff("r1"));
    await settle();
    const socket = FakeSocket.last!;
    const id = ready(socket);
    act(() => socket.receive({ type: "next", id, payload: { data: diff("one", true) } }));
    expect(result.current).toMatchObject({ patch: "one", live: true, error: null, reconnecting: false });

    act(() => socket.receive({ type: "next", id, payload: { data: diff("two", false) } }));
    expect(result.current).toMatchObject({ patch: "two", live: false });

    // The run is over: a dropped socket now is not worth another attempt.
    act(() => socket.drop(1006));
    await act(async () => vi.advanceTimersByTimeAsync(30_000));
    expect(FakeSocket.all).toHaveLength(1);
    expect(result.current.reconnecting).toBe(false);
  });

  it("ends on complete", async () => {
    vi.useFakeTimers();
    const { result } = renderHook(() => useRunDiff("r1"));
    await settle();
    const socket = FakeSocket.last!;
    const id = ready(socket);
    act(() => socket.receive({ type: "next", id, payload: { data: diff("one", true) } }));
    act(() => socket.receive({ type: "complete", id }));
    expect(result.current.live).toBe(false);
    await act(async () => vi.advanceTimersByTimeAsync(30_000));
    expect(FakeSocket.all).toHaveLength(1);
  });

  it("keeps one socket open under StrictMode's double mount", async () => {
    const wrapper = ({ children }: { children: ReactNode }) => createElement(StrictMode, null, children);
    const { result } = renderHook(() => useRunDiff("r1"), { wrapper });
    await settle();
    expect(FakeSocket.all.filter((socket) => socket.closedWith === null)).toHaveLength(1);
    const socket = FakeSocket.all.find((each) => each.closedWith === null)!;
    const id = ready(socket);
    act(() => socket.receive({ type: "next", id, payload: { data: diff("one", true) } }));
    expect(result.current.patch).toBe("one");
  });

  it("tries again with backoff when the server never acknowledges, while the run is live", async () => {
    vi.useFakeTimers();
    const { result } = renderHook(() => useRunDiff("r1"));
    await settle();
    FakeSocket.last!.open();
    await act(async () => vi.advanceTimersByTimeAsync(ACK_TIMEOUT_MS));
    expect(result.current.reconnecting).toBe(true);
    expect(result.current.error).toBeNull();
    expect(FakeSocket.all).toHaveLength(1);

    // The first retry waits a second.
    await act(async () => vi.advanceTimersByTimeAsync(999));
    expect(FakeSocket.all).toHaveLength(1);
    await act(async () => vi.advanceTimersByTimeAsync(1));
    expect(FakeSocket.all).toHaveLength(2);

    // It drops again: the next wait is two seconds.
    act(() => FakeSocket.last!.drop(1006));
    await act(async () => vi.advanceTimersByTimeAsync(1_999));
    expect(FakeSocket.all).toHaveLength(2);
    await act(async () => vi.advanceTimersByTimeAsync(1));
    expect(FakeSocket.all).toHaveLength(3);

    const socket = FakeSocket.last!;
    const id = ready(socket);
    act(() => socket.receive({ type: "next", id, payload: { data: diff("back", true) } }));
    expect(result.current).toMatchObject({ patch: "back", live: true, reconnecting: false });
  });

  it("does not try again when the server refuses the token", async () => {
    vi.useFakeTimers();
    const { result } = renderHook(() => useRunDiff("r1"));
    await settle();
    act(() => {
      FakeSocket.last!.open();
      FakeSocket.last!.drop(4403, "Forbidden");
    });
    expect(result.current).toMatchObject({ error: "The server refused the token", reconnecting: false, live: false });
    await act(async () => vi.advanceTimersByTimeAsync(30_000));
    expect(FakeSocket.all).toHaveLength(1);
  });
});
