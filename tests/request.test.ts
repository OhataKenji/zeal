import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { createRequest, type RequestState } from "../src/lib/request.ts";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("createRequest", () => {
  it("transitions idle -> loading -> ready and retains value when requested", async () => {
    const states: Array<RequestState<string>> = [];
    const req = createRequest<string>((s) => states.push(s));
    assert.deepEqual(req.state, { kind: "idle", value: null, error: null });

    const first = deferred<string>();
    const p1 = req.run(() => first.promise);
    assert.deepEqual(req.state, { kind: "loading", value: null, error: null });
    first.resolve("v1");
    assert.equal(await p1, "v1");
    assert.deepEqual(req.state, { kind: "ready", value: "v1", error: null });

    const second = deferred<string>();
    const p2 = req.run(() => second.promise, { retainValue: true });
    assert.deepEqual(req.state, { kind: "loading", value: "v1", error: null });
    second.resolve("v2");
    assert.equal(await p2, "v2");
    assert.deepEqual(req.state, { kind: "ready", value: "v2", error: null });
    assert.equal(states.length, 4);
  });

  it("ignores stale replies from superseded requests", async () => {
    const states: Array<RequestState<string>> = [];
    const req = createRequest<string>((s) => states.push(s));

    const first = deferred<string>();
    const second = deferred<string>();
    const p1 = req.run(() => first.promise);
    const p2 = req.run(() => second.promise);

    first.resolve("stale");
    assert.equal(await p1, undefined);
    assert.equal(req.state.kind, "loading");

    second.resolve("fresh");
    assert.equal(await p2, "fresh");
    assert.deepEqual(req.state, { kind: "ready", value: "fresh", error: null });
    assert.equal(states.length, 3);
  });

  it("clears retained data on failure and invalidation", async () => {
    const req = createRequest<string>(() => {});
    await req.run(async () => "v1");
    assert.equal(req.state.value, "v1");

    const failing = deferred<string>();
    const p = req.run(() => failing.promise, { retainValue: true });
    assert.equal(req.state.value, "v1");
    failing.reject(new Error("boom"));
    assert.equal(await p, undefined);
    assert.deepEqual(req.state, { kind: "failed", value: null, error: "Error: boom" });

    req.invalidate();
    assert.deepEqual(req.state, { kind: "idle", value: null, error: null });
  });

  it("stops publishing and ignores late replies after dispose", async () => {
    const states: Array<RequestState<string>> = [];
    const req = createRequest<string>((s) => states.push(s));
    const pending = deferred<string>();
    const p = req.run(() => pending.promise);
    assert.equal(states.length, 1);

    req.dispose();
    assert.equal(req.state.kind, "disposed");
    assert.equal(states.length, 1);

    pending.resolve("late");
    assert.equal(await p, undefined);
    assert.equal(states.length, 1);

    assert.equal(await req.run(async () => "ignored"), undefined);
    req.invalidate();
    req.fail("ignored");
    assert.equal(states.length, 1);
  });
});
