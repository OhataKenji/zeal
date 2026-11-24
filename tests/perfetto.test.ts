import test from "node:test";
import assert from "node:assert/strict";
import { startTraceHandshake } from "../src/lib/perfetto.ts";

const VIEWER = "http://127.0.0.1:45678";
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function fakeViewer() {
  const posts: Array<{ message: unknown; origin: string; transfer?: unknown }> = [];
  const contentWindow = {
    postMessage(message: unknown, origin: string, transfer?: unknown) {
      posts.push({ message, origin, transfer });
    },
  };
  const frame = { contentWindow };
  return { frame, contentWindow, posts };
}

function harness(overrides: Record<string, unknown> = {}) {
  const target = new EventTarget();
  const viewer = fakeViewer();
  const state = {
    loads: 0,
    sent: 0,
    errors: [] as unknown[],
    timeouts: 0,
    buffer: new ArrayBuffer(8),
  };
  const { load, ...rest } = overrides;
  const stop = startTraceHandshake({
    frame: () => viewer.frame,
    viewer: VIEWER,
    load: async () => {
      state.loads += 1;
      return typeof load === "function" ? (load as () => Promise<ArrayBuffer>)() : state.buffer;
    },
    title: "fixture",
    onSent: () => {
      state.sent += 1;
    },
    onError: (failure) => {
      state.errors.push(failure);
    },
    onTimeout: () => {
      state.timeouts += 1;
    },
    target,
    pingMs: 5,
    timeoutMs: 200,
    ...rest,
  });
  const pong = (fields: Record<string, unknown> = {}) =>
    target.dispatchEvent(
      Object.assign(new Event("message"), {
        origin: VIEWER,
        source: viewer.contentWindow,
        data: "PONG",
        ...fields,
      }),
    );
  return { target, viewer, state, stop, pong };
}

test("sends the trace once after a verified PONG and pings meanwhile", async () => {
  const h = harness();
  await sleep(20);
  assert.ok(
    h.viewer.posts.some((p) => p.message === "PING" && p.origin === VIEWER),
    "expected PING retries before PONG",
  );
  assert.equal(h.state.loads, 0, "trace must not be read before PONG");
  h.pong();
  await sleep(0);
  h.pong();
  await sleep(0);
  assert.equal(h.state.loads, 1);
  assert.equal(h.state.sent, 1);
  const sent = h.viewer.posts.filter((p) => typeof p.message === "object");
  assert.equal(sent.length, 1);
  assert.deepEqual(sent[0].message, {
    perfetto: { buffer: h.state.buffer, title: "fixture" },
  });
  assert.equal(sent[0].origin, VIEWER);
  assert.deepEqual(sent[0].transfer, [h.state.buffer]);
  h.stop();
});

test("ignores impostor PONGs from other origins, sources and payloads", async () => {
  const h = harness();
  h.pong({ origin: "https://evil.example" });
  h.pong({ source: {} });
  h.pong({ data: "PING" });
  h.pong({ data: { perfetto: {} } });
  await sleep(0);
  assert.equal(h.state.loads, 0);
  assert.equal(h.state.sent, 0);
  h.pong();
  await sleep(0);
  assert.equal(h.state.loads, 1);
  assert.equal(h.state.sent, 1);
  h.stop();
});

test("cleanup before PONG suppresses pings and any later send", async () => {
  const h = harness();
  await sleep(12);
  const before = h.viewer.posts.length;
  h.stop();
  await sleep(12);
  assert.equal(h.viewer.posts.length, before, "PINGs must stop after cleanup");
  h.pong();
  await sleep(0);
  assert.equal(h.state.loads, 0);
  assert.equal(h.state.sent, 0);
});

test("cleanup while the trace read is pending suppresses the send", async () => {
  let release!: (buffer: ArrayBuffer) => void;
  const pending = new Promise<ArrayBuffer>((resolve) => {
    release = resolve;
  });
  const h = harness({ load: () => pending });
  h.pong();
  await sleep(0);
  assert.equal(h.state.loads, 1);
  h.stop();
  release(h.state.buffer);
  await sleep(0);
  assert.equal(h.state.sent, 0);
  assert.equal(h.viewer.posts.filter((p) => typeof p.message === "object").length, 0);
});

test("times out without a PONG and ignores a late PONG", async () => {
  const h = harness({ timeoutMs: 30 });
  await sleep(60);
  assert.equal(h.state.timeouts, 1);
  assert.equal(h.state.loads, 0);
  h.pong();
  await sleep(0);
  assert.equal(h.state.sent, 0);
  h.stop();
});

test("a failed trace read reports the error and does not send", async () => {
  const failure = new Error("store missing");
  const h = harness({
    load: async () => {
      throw failure;
    },
  });
  h.pong();
  await sleep(0);
  assert.deepEqual(h.state.errors, [failure]);
  assert.equal(h.state.sent, 0);
  assert.equal(h.viewer.posts.filter((p) => typeof p.message === "object").length, 0);
  h.stop();
});
