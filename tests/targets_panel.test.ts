import { test } from "node:test";
import assert from "node:assert/strict";
import type { Bep, Section } from "../src/lib/types.ts";
import { createSvelteHarness } from "./svelte.ts";

test("TargetsPanel distinguishes same-label results by aspect and configuration", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: TargetsPanel } = await load("/src/lib/invocation/TargetsPanel.svelte");

  function output(targets: Bep["targets"]): string {
    const section: Section<Bep> = {
      status: "ok",
      data: { targets, tests: [], truncated: false },
    };
    return render(TargetsPanel, { props: { section } }).body.replace(/<!--[\s\S]*?-->/g, "");
  }

  function rows(html: string): string[] {
    return Array.from(html.matchAll(/<li\b[^>]*>([\s\S]*?)<\/li>/g), ([, content]) =>
      content
        .replace(/<[^>]*>/g, " ")
        .replace(/\s+/g, " ")
        .trim(),
    );
  }

  await t.test("same-label results keep their full identity in each row", () => {
    const html = output([
      { label: "//pkg:lib", ok: true, configuration: "cfg-a" },
      { label: "//pkg:lib", ok: true, aspect: "rustfmt", configuration: "cfg-a" },
      { label: "//pkg:lib", ok: false, aspect: "clippy", configuration: "cfg-a" },
      { label: "//pkg:lib", ok: true, configuration: "cfg-b" },
    ]);
    assert.deepEqual(rows(html), [
      "✓ //pkg:lib Configuration: cfg-a Completed",
      "✓ //pkg:lib Aspect: rustfmt Configuration: cfg-a Completed",
      "✗ //pkg:lib Aspect: clippy Configuration: cfg-a Failed",
      "✓ //pkg:lib Configuration: cfg-b Completed",
    ]);
  });

  await t.test("missing aspect and configuration render no placeholder metadata", () => {
    const html = output([
      { label: "//pkg:bare", ok: true },
      { label: "//pkg:aspect-only", ok: false, aspect: "ObjcProtoAspect" },
    ]);
    assert.deepEqual(rows(html), [
      "✓ //pkg:bare Completed",
      "✗ //pkg:aspect-only Aspect: ObjcProtoAspect Failed",
    ]);
  });

  await t.test("aspect values are rendered escaped", () => {
    const html = output([{ label: "//pkg:x", ok: true, aspect: "<script>alert(1)</script>" }]);
    assert.doesNotMatch(html, /<script>alert/);
    assert.match(html, /&lt;script>alert\(1\)&lt;\/script>/);
  });

  await t.test("failed test rows render a Re-run button inheriting explicit flags", () => {
    const section: Section<Bep> = {
      status: "ok",
      data: {
        targets: [],
        tests: [
          { label: "//pkg:pass_test", status: "PASSED" },
          { label: "@@//pkg:fail_test", status: "FAILED" },
          { label: "//pkg:slow_test", status: "TIMEOUT" },
        ],
        truncated: false,
      },
    };
    const html = render(TargetsPanel, {
      props: {
        section,
        rerunFlags: "--config=asan --test_env=FOO=1",
        canRerunTest: (pattern: string) => pattern !== "//pkg:slow_test",
        onreruntest: () => {},
      },
    }).body.replace(/<!--[\s\S]*?-->/g, "");

    assert.doesNotMatch(html, /aria-label="Re-run \/\/pkg:pass_test"/);
    assert.match(
      html,
      /<button(?=[^>]*aria-label="Re-run @@\/\/pkg:fail_test")(?=[^>]*title="Re-run: bazel test \/\/pkg:fail_test --config=asan --test_env=FOO=1")(?![^>]*disabled)[^>]*>\s*Re-run\s*<\/button>/,
    );
    assert.match(
      html,
      /<button(?=[^>]*aria-label="Re-run \/\/pkg:slow_test")(?=[^>]*disabled)(?![^>]*title=)[^>]*>\s*Re-run\s*<\/button>/,
    );
  });
});
