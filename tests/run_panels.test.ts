import { after, test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";
import type { Bep, Section } from "../src/lib/types.ts";

const server = await createServer({
  root: fileURLToPath(new URL("..", import.meta.url)),
  server: { middlewareMode: true, watch: null },
});
after(() => server.close());
const { render } = await server.ssrLoadModule("svelte/server");
const { default: TargetsPanel } = await server.ssrLoadModule("/src/lib/run/TargetsPanel.svelte");
const { default: LogPanel } = await server.ssrLoadModule("/src/lib/run/LogPanel.svelte");

const EMPTY_BEP: Bep = { targets: [], tests: [], truncated: false };

function targets(section?: Section<Bep>): string {
  return render(TargetsPanel, { props: { section } }).body;
}

function subsections(html: string): string[] {
  return html.split('<h2 class="panel-heading">').slice(1);
}

test("missing results retain both headings before their missing messages", () => {
  const sections = subsections(targets({ status: "missing" }));
  assert.equal(sections.length, 2);
  for (const [index, title] of ["Targets", "Tests"].entries()) {
    assert.match(sections[index], new RegExp(`^${title} .*?</h2>.*missing — no record`));
    assert.doesNotMatch(sections[index], /<span>/);
  }
});

test("loading results retain headings without claiming data is missing", () => {
  const html = targets();
  assert.equal(subsections(html).length, 2);
  assert.doesNotMatch(html, /missing|No results|<span>/);
});

test("valid empty results show zero counts and consistent empty messages", () => {
  for (const section of subsections(targets({ status: "ok", data: EMPTY_BEP }))) {
    assert.match(section, /<span>0<\/span>/);
    assert.match(section, /No results recorded\./);
    assert.doesNotMatch(section, /missing/);
  }
});

test("target and test results remain visible independently", () => {
  const target = { label: "//examples/cpp/helloworld:greeter_server", ok: false };
  const result = { label: "//app:unit_test", status: "PASSED" };
  for (const data of [
    { ...EMPTY_BEP, targets: [target] },
    { ...EMPTY_BEP, tests: [result] },
    { ...EMPTY_BEP, targets: [target], tests: [result] },
  ]) {
    const [targetsSection, testsSection] = subsections(targets({ status: "ok", data }));
    assert.match(targetsSection, new RegExp(`<span>${data.targets.length}</span>`));
    assert.match(testsSection, new RegExp(`<span>${data.tests.length}</span>`));
    if (data.targets.length) {
      assert.ok(targetsSection.includes(target.label));
      assert.match(targetsSection, /✗.*Failed/);
      assert.doesNotMatch(targetsSection, /No results/);
    } else {
      assert.match(targetsSection, /No results recorded\./);
    }
    if (data.tests.length) {
      assert.ok(testsSection.includes(result.label));
      assert.match(testsSection, /PASSED/);
      assert.doesNotMatch(testsSection, /No results/);
    } else {
      assert.match(testsSection, /No results recorded\./);
    }
  }
});

test("incomplete results retain their data and explain both subsections", () => {
  const html = targets({
    status: "incomplete",
    data: { ...EMPTY_BEP, targets: [{ label: "//app:server", ok: true }] },
    note: "stream ended early",
  });
  for (const section of subsections(html)) {
    assert.match(section, /incomplete — stream ended early/);
    assert.doesNotMatch(section, /missing/);
  }
  assert.match(html, /Completed/);
});

for (const status of ["parse_error", "read_error", "not_available"] as const) {
  test(`${status} stays labeled without being replaced by a missing message`, () => {
    for (const section of subsections(targets({ status, note: "reason" }))) {
      assert.ok(section.includes(`${status.replaceAll("_", " ")} — reason`));
      assert.doesNotMatch(section, /missing|No results|<span>/);
    }
  });
}

test("logs use the same missing message under their heading", () => {
  const html = render(LogPanel, {
    props: { section: { status: "missing" }, loading: false },
  }).body;
  assert.match(html, /<h2 class="panel-heading">Logs<\/h2>.*missing — no record/);
});

test("log loading is not mistaken for missing data", () => {
  const html = render(LogPanel, { props: { section: undefined, loading: true } }).body;
  assert.match(html, /Logs<\/h2>.*Loading build log…/);
  assert.doesNotMatch(html, /missing/);
});
