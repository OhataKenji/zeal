import test, { mock } from "node:test";
import assert from "node:assert/strict";
import { setImmediate as flush } from "node:timers/promises";
import { startTraceHandshake } from "../src/lib/perfetto.ts";
import type { TraceHandshake } from "../src/lib/perfetto.ts";

const VIEWER = "http://127.0.0.1:45678";

function harness(overrides: Partial<TraceHandshake> = {}) {
  const { load: loadImpl, ...rest } = overrides;
  const target = new EventTarget();
  const buffer = new ArrayBuffer(8);
  const posts: Array<{
    message: unknown;
    origin: string;
    transfer: Transferable[] | undefined;
  }> = [];
  const errors: unknown[] = [];
  const contentWindow = {
    postMessage: mock.fn((message: unknown, origin: string, transfer?: Transferable[]) => {
      posts.push({ message, origin, transfer });
    }),
  };
  const frame = { contentWindow };
  const load = mock.fn(loadImpl ?? (async () => buffer));
  const onSent = mock.fn();
  const onError = mock.fn((failure: unknown) => {
    errors.push(failure);
  });
  const onTimeout = mock.fn();
  const stop = startTraceHandshake({
    frame: () => frame,
    viewer: VIEWER,
    load,
    title: "fixture",
    onSent,
    onError,
    onTimeout,
    target,
    pingMs: 5,
    timeoutMs: 200,
    ...rest,
  });
  const pong = (fields: Record<string, unknown> = {}) =>
    target.dispatchEvent(
      Object.assign(new Event("message"), {
        origin: VIEWER,
        source: contentWindow,
        data: "PONG",
        ...fields,
      }),
    );
  const sends = () => posts.filter((p) => typeof p.message === "object");
  return { buffer, posts, errors, load, onSent, onError, onTimeout, stop, pong, sends };
}

test("ignores impostor PONGs, then sends the trace once after verified PONGs", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval"] });
  const h = harness();
  t.mock.timers.tick(10);
  assert.ok(
    h.posts.some((p) => p.message === "PING" && p.origin === VIEWER),
    "expected PINGs while waiting for the viewer",
  );
  h.pong({ origin: "https://evil.example" });
  h.pong({ source: {} });
  h.pong({ data: "PING" });
  h.pong({ data: { perfetto: {} } });
  await flush();
  assert.equal(h.load.mock.callCount(), 0);
  assert.equal(h.sends().length, 0);

  h.pong();
  await flush();
  h.pong();
  await flush();
  assert.equal(h.load.mock.callCount(), 1);
  assert.equal(h.onSent.mock.callCount(), 1);
  const sent = h.sends();
  assert.equal(sent.length, 1);
  assert.deepEqual(sent[0].message, {
    perfetto: { buffer: h.buffer, title: "fixture" },
  });
  assert.equal(sent[0].origin, VIEWER);
  assert.deepEqual(sent[0].transfer, [h.buffer]);
  h.stop();
});

test("cleanup while the trace read is pending suppresses the send", async () => {
  let release!: (buffer: ArrayBuffer) => void;
  const gate = new Promise<ArrayBuffer>((resolve) => {
    release = resolve;
  });
  const h = harness({ load: () => gate });
  h.pong();
  assert.equal(h.load.mock.callCount(), 1);
  h.stop();
  release(h.buffer);
  await flush();
  assert.equal(h.sends().length, 0);
  assert.equal(h.onSent.mock.callCount(), 0);
});

test("times out on schedule and ignores a late PONG", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval"] });
  const h = harness({ timeoutMs: 100 });
  t.mock.timers.tick(10);
  assert.ok(h.posts.some((p) => p.message === "PING"));
  t.mock.timers.tick(100);
  assert.equal(h.onTimeout.mock.callCount(), 1);
  h.pong();
  await flush();
  assert.equal(h.load.mock.callCount(), 0);
  assert.equal(h.sends().length, 0);
  h.stop();
});

test("a failed trace read reports the error and sends nothing", async () => {
  const failure = new Error("store missing");
  const h = harness({
    load: async () => {
      throw failure;
    },
  });
  h.pong();
  await flush();
  assert.deepEqual(h.errors, [failure]);
  assert.equal(h.sends().length, 0);
  h.stop();
});
