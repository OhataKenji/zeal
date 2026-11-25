import assert from "node:assert/strict";
import { test } from "node:test";
import { createGraphRenderer } from "../src/lib/graph-renderer.ts";

type Reply = { svg?: string; error?: string };

function setup() {
  const jobs: Array<{ dot: string; reply: (data: Reply) => void }> = [];
  const workers: Array<{ stops: number }> = [];
  const renderer = createGraphRenderer(() => {
    const worker = {
      stops: 0,
      onmessage: null as ((event: MessageEvent<Reply>) => void) | null,
      onerror: null as ((event: ErrorEvent) => void) | null,
      postMessage(dot: string) {
        const handler = this.onmessage;
        assert.ok(handler);
        jobs.push({ dot, reply: (data) => handler(new MessageEvent("message", { data })) });
      },
      terminate() {
        assert.equal(this.onmessage, null);
        assert.equal(this.onerror, null);
        this.stops++;
      },
    };
    workers.push(worker);
    return worker;
  });
  return { renderer, workers, jobs };
}

test("stale cleanup and replies cannot disturb the next render on a warm worker", () => {
  const { renderer, workers, jobs } = setup();
  const results: string[] = [];
  const ok = (svg: string) => results.push(svg);
  const cancelFirst = renderer.render("first", ok, assert.fail);
  jobs[0].reply({ svg: "one" });
  renderer.render("second", ok, assert.fail);
  cancelFirst();
  jobs[0].reply({ error: "late" });
  jobs[1].reply({ svg: "two" });
  assert.deepEqual(results, ["one", "two"]);
  assert.deepEqual(
    jobs.map((job) => job.dot),
    ["first", "second"],
  );
  assert.equal(workers.length, 1);
  assert.equal(workers[0].stops, 0);
  renderer.dispose();
  renderer.dispose();
  assert.equal(workers[0].stops, 1);
});

test("replacement, abandonment and disposal each stop only the active job", () => {
  const { renderer, workers, jobs } = setup();
  const first = renderer.render("first", assert.fail, assert.fail);
  const second = renderer.render("second", assert.fail, assert.fail);
  first();
  assert.deepEqual(
    workers.map((worker) => worker.stops),
    [1, 0],
  );
  second();
  renderer.render("third", assert.fail, assert.fail);
  renderer.dispose();
  second();
  const after = renderer.render("after", assert.fail, assert.fail);
  after();
  assert.equal(jobs.length, 3);
  for (const job of jobs) job.reply({ svg: "late" });
  assert.deepEqual(
    workers.map((worker) => worker.stops),
    [1, 1, 1],
  );
});

test("a timed-out layout fails once and the next layout starts cold", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { renderer, workers, jobs } = setup();
  const errors: string[] = [];
  renderer.render("slow", assert.fail, (error) => errors.push(error));
  t.mock.timers.tick(29_999);
  assert.equal(errors.length, 0);
  t.mock.timers.tick(1);
  assert.deepEqual(errors, ["Graphviz could not lay out this result within 30s."]);
  assert.equal(workers[0].stops, 1);
  renderer.render("new", assert.fail, (error) => errors.push(error));
  jobs[0].reply({ svg: "late" });
  jobs[1].reply({ error: "invalid DOT" });
  t.mock.timers.tick(30_000);
  assert.deepEqual(errors, ["Graphviz could not lay out this result within 30s.", "invalid DOT"]);
  assert.equal(workers[1].stops, 0);
  renderer.dispose();
});
