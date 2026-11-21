import assert from "node:assert/strict";
import { test } from "node:test";
import {
  createModules,
  emptyModulesState,
  filterModules,
  filterRepos,
} from "../src/lib/modules.ts";
import type { ModulesSnapshot } from "../src/lib/types.ts";

const workspace = "/demo";
const a = {
  key: "alpha@1",
  name: "alpha",
  version: "1",
  direct: true,
  requiredBy: ["<root>"],
  dependencies: ["beta@2"],
};
const b = {
  key: "beta@2",
  name: "beta",
  version: "2",
  direct: false,
  requiredBy: [a.key],
  dependencies: [],
};
const snapshot: ModulesSnapshot = {
  rootKey: "<root>",
  modules: [a, b],
  repoMappingError: null,
  repos: [{ apparent: "alias", canonical: "generated+name" }],
};

function harness(available = true) {
  const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];
  let resolve!: (value: unknown) => void;
  let reject!: (error: Error) => void;
  let state = emptyModulesState();
  const controller = createModules(
    workspace,
    {
      invoke<T>(command: string, args?: Record<string, unknown>) {
        calls.push({ command, args });
        return new Promise<unknown>((ok, fail) => {
          resolve = ok;
          reject = fail;
        }) as Promise<T>;
      },
    },
    (next) => (state = next),
    () => available,
  );
  return {
    ...controller,
    calls,
    get state() {
      return state;
    },
    succeed: (value: unknown) => resolve(value),
    fail: () => reject(new Error("server busy")),
  };
}

test("opening and browsing are local; Bazel must be available for an explicit request", async () => {
  const h = harness(false);
  assert.equal(h.state.snapshot, null);
  h.select(a.key);
  await h.load();
  await h.explain();
  assert.deepEqual(h.calls, []);
});

for (const outcome of ["success", "error"]) {
  test(`busy explain discards a stale ${outcome} without queuing another command`, async () => {
    const h = harness();
    const load = h.load();
    assert.equal(h.state.busy, "load");
    await h.load();
    h.succeed(snapshot);
    await load;
    h.select(a.key);
    assert.deepEqual(h.calls, [{ command: "get_modules", args: { workspace } }]);

    const explain = h.explain();
    h.select(b.key);
    await h.explain();
    await h.load();
    assert.equal(h.state.busy, "explain");
    if (outcome === "error") h.fail();
    else h.succeed("details of alpha");
    await explain;
    assert.equal(h.state.selectedKey, b.key);
    assert.equal(h.state.busy, null);
    assert.equal(h.state.explanation, null);
    assert.equal(h.state.explainError, null);
    assert.deepEqual(h.calls.slice(1), [
      { command: "explain_module", args: { workspace, moduleKey: a.key } },
    ]);
  });
}

test("loads can be retried, failed refreshes preserve the last good snapshot", async () => {
  const h = harness();
  const first = h.load();
  h.fail();
  await first;
  assert.match(h.state.loadError ?? "", /server busy/);
  const retry = h.load();
  h.succeed(snapshot);
  await retry;
  assert.equal(h.state.loadError, null);
  const refresh = h.load();
  h.fail();
  await refresh;
  assert.equal(h.state.snapshot, snapshot);
  assert.match(h.state.loadError ?? "", /server busy/);
});

test("refresh clears explanations without re-explaining, preserving only an existing selection", async () => {
  const h = harness();
  const initial = h.load();
  h.succeed(snapshot);
  await initial;
  for (const modules of [[a, b], [b]]) {
    h.select(a.key);
    const explain = h.explain();
    h.succeed("reason");
    await explain;
    assert.equal(h.state.explanation, "reason");
    const refresh = h.load();
    h.succeed({ ...snapshot, modules });
    await refresh;
    assert.equal(h.state.explanation, null);
    assert.equal(h.state.selectedKey, modules.includes(a) ? a.key : null);
  }
  assert.equal(h.calls.length, 5);
});

test("a failed explanation can be retried, but returning to a selection does not restore a cache", async () => {
  const h = harness();
  const load = h.load();
  h.succeed(snapshot);
  await load;
  h.select(b.key);
  const first = h.explain();
  h.fail();
  await first;
  assert.match(h.state.explainError ?? "", /server busy/);
  const retry = h.explain();
  h.succeed("reason");
  await retry;
  assert.equal(h.state.explainError, null);
  assert.equal(h.state.explanation, "reason");
  h.select(a.key);
  h.select(b.key);
  assert.equal(h.state.explanation, null);
  assert.equal(h.calls.length, 3);
});

test("disposal suppresses late callbacks and new commands", async () => {
  const h = harness();
  const request = h.load();
  const before = h.state;
  h.dispose();
  h.succeed(snapshot);
  await request;
  await h.load();
  assert.equal(h.state, before);
  assert.equal(h.calls.length, 1);
});

test("module filters combine text and depth; repo searches match either displayed name", () => {
  assert.deepEqual(filterModules(snapshot.modules, " ALPHA@1 ", "direct"), [a]);
  assert.deepEqual(filterModules(snapshot.modules, "alpha", "transitive"), []);
  assert.deepEqual(filterModules(snapshot.modules, "", "transitive"), [b]);
  assert.deepEqual(filterModules(snapshot.modules, "2", "all"), [b]);
  for (const term of ["Alias", "@@generated", "+name"]) {
    assert.deepEqual(filterRepos(snapshot.repos, term), snapshot.repos);
  }
  assert.deepEqual(filterRepos(snapshot.repos, "absent"), []);
});
