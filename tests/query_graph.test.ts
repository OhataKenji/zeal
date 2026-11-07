import { test, type TestContext } from "node:test";
import assert from "node:assert/strict";
import { createGraphRenderer, type GraphState, type GraphWorker } from "../src/lib/query/graph.ts";

class FakeWorker {
  onmessage: GraphWorker["onmessage"] = null;
  onerror: GraphWorker["onerror"] = null;
  posted: unknown[] = [];
  terminated = 0;
  postMessage(data: unknown) {
    this.posted.push(data);
  }
  terminate() {
    this.terminated++;
  }
  reply(data: { svg?: string; error?: string }) {
    const handler = this.onmessage as ((e: { data: unknown }) => void) | null;
    handler?.({ data });
  }
  crash(message: string) {
    const handler = this.onerror as ((e: { message: string }) => void) | null;
    handler?.({ message });
  }
}

function harness(t: TestContext) {
  const workers: FakeWorker[] = [];
  const states: GraphState[] = [];
  let nextUrl = 0;
  const createUrl = t.mock.method(URL, "createObjectURL", () => `blob:fake-${nextUrl++}`);
  const revokeUrl = t.mock.method(URL, "revokeObjectURL", () => {});
  const renderer = createGraphRenderer(
    () => {
      const w = new FakeWorker();
      workers.push(w);
      return w as unknown as GraphWorker;
    },
    (s) => states.push(s),
  );
  t.after(() => renderer.dispose());
  return {
    workers,
    states,
    createUrl,
    revokeUrl,
    renderer,
    last: () => states.at(-1) as GraphState,
    revoked: () => revokeUrl.mock.calls.map((c) => c.arguments[0]),
  };
}

test("null or blank input publishes an empty state and creates no worker", (t) => {
  const h = harness(t);
  h.renderer.render(null);
  assert.equal(h.workers.length, 0);
  assert.deepEqual(h.last(), { url: null, error: null, loading: false });
  h.renderer.render("   ");
  assert.equal(h.workers.length, 0);
  assert.deepEqual(h.last(), { url: null, error: null, loading: false });
  assert.equal(h.createUrl.mock.callCount(), 0);
});

test("a successful render publishes the SVG blob URL and reuses the warm worker", (t) => {
  const h = harness(t);
  h.renderer.render("digraph { a }");
  assert.equal(h.workers.length, 1);
  assert.deepEqual(h.last(), { url: null, error: null, loading: true });
  assert.deepEqual(h.workers[0].posted, ["digraph { a }"]);
  h.workers[0].reply({ svg: "<svg>a</svg>" });
  assert.deepEqual(h.last(), {
    url: "blob:fake-0",
    error: null,
    loading: false,
  });
  h.renderer.render("digraph { b }");
  assert.equal(h.workers.length, 1, "the settled worker is retained");
  assert.equal(h.workers[0].terminated, 0);
  assert.deepEqual(h.workers[0].posted, ["digraph { a }", "digraph { b }"]);
  assert.deepEqual(h.revoked(), ["blob:fake-0"]);
  h.workers[0].reply({ svg: "<svg>b</svg>" });
  assert.equal(h.last().url, "blob:fake-1");
});

test("replacing a pending render terminates its worker; the stale reply cannot publish", (t) => {
  const h = harness(t);
  h.renderer.render("digraph { a }");
  const stale = h.workers[0];
  const staleReply = stale.onmessage;
  h.renderer.render("digraph { b }");
  assert.equal(stale.terminated, 1);
  assert.equal(stale.onmessage, null);
  assert.equal(h.workers.length, 2);
  staleReply?.call(
    stale as unknown as Worker,
    {
      data: { svg: "old" },
    } as MessageEvent,
  );
  assert.deepEqual(h.last(), { url: null, error: null, loading: true });
  assert.equal(h.createUrl.mock.callCount(), 0);
  h.workers[1].reply({ svg: "<svg>b</svg>" });
  assert.equal(h.last().url, "blob:fake-0");
});

test("rendering null revokes the URL and abandons the pending render", (t) => {
  const h = harness(t);
  h.renderer.render("digraph { a }");
  h.workers[0].reply({ svg: "<svg>a</svg>" });
  h.renderer.render("digraph { b }");
  assert.deepEqual(h.revoked(), ["blob:fake-0"]);
  assert.equal(h.workers.length, 1, "the pending render reuses the warm worker");
  h.renderer.render(null);
  assert.equal(h.workers[0].terminated, 1);
  assert.deepEqual(h.last(), { url: null, error: null, loading: false });
});

test("a 30s timeout terminates the worker; the next render starts a fresh one", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const h = harness(t);
  h.renderer.render("digraph { a }");
  t.mock.timers.tick(30_000);
  assert.equal(h.workers[0].terminated, 1);
  assert.deepEqual(h.last(), {
    url: null,
    error: "Graphviz could not lay out this result within 30s.",
    loading: false,
  });
  h.renderer.render("digraph { b }");
  assert.equal(h.workers.length, 2);
  h.workers[1].reply({ svg: "<svg>b</svg>" });
  assert.equal(h.last().url, "blob:fake-0");
  t.mock.timers.tick(60_000);
  assert.equal(h.workers[1].terminated, 0);
});

test("a worker-reported layout error keeps the worker warm for reuse", (t) => {
  const h = harness(t);
  h.renderer.render("digraph { bad }");
  h.workers[0].reply({ error: "layout failed" });
  assert.deepEqual(h.last(), {
    url: null,
    error: "layout failed",
    loading: false,
  });
  assert.equal(h.workers[0].terminated, 0);
  h.renderer.render("digraph { ok }");
  assert.equal(h.workers.length, 1, "the error leaves the worker usable");
  assert.deepEqual(h.workers[0].posted, ["digraph { bad }", "digraph { ok }"]);
});

test("a worker crash terminates it and reports the crash", (t) => {
  const h = harness(t);
  h.renderer.render("digraph { a }");
  h.workers[0].crash("wasm exploded");
  assert.equal(h.workers[0].terminated, 1);
  assert.deepEqual(h.last(), {
    url: null,
    error: "wasm exploded",
    loading: false,
  });
  h.renderer.render("digraph { b }");
  assert.equal(h.workers.length, 2);
  h.workers[1].crash("");
  assert.equal(h.last().error, "the graph renderer crashed");
});

test("dispose during a pending render releases it and ignores the late reply", (t) => {
  const h = harness(t);
  h.renderer.render("digraph { a }");
  const staleReply = h.workers[0].onmessage;
  h.renderer.dispose();
  assert.equal(h.workers[0].terminated, 1);
  staleReply?.call(
    h.workers[0] as unknown as Worker,
    {
      data: { svg: "late" },
    } as MessageEvent,
  );
  assert.equal(h.states.length, 1, "nothing publishes after dispose");
  assert.equal(h.createUrl.mock.callCount(), 0);
  h.renderer.render("digraph { b }");
  assert.equal(h.workers.length, 1, "render after dispose is a no-op");
  h.renderer.dispose();
  assert.equal(h.workers[0].terminated, 1, "second dispose is a no-op");
});

test("dispose after success revokes the URL and terminates the retained worker", (t) => {
  const h = harness(t);
  h.renderer.render("digraph { a }");
  h.workers[0].reply({ svg: "<svg>a</svg>" });
  h.renderer.dispose();
  assert.equal(h.workers[0].terminated, 1);
  assert.deepEqual(h.revoked(), ["blob:fake-0"]);
  h.renderer.dispose();
  assert.equal(h.workers[0].terminated, 1);
  assert.equal(h.revokeUrl.mock.callCount(), 1, "no double revocation");
});
