import { test } from "node:test";
import assert from "node:assert/strict";

test("viz worker applies dark defaults without overriding explicit colors", async (t) => {
  const globals = globalThis as Record<string, unknown>;
  const previousOnMessage = globals.onmessage;
  const previousPostMessage = globals.postMessage;
  const messages: Array<{ svg?: string; error?: string }> = [];
  globals.onmessage = null;
  globals.postMessage = (message: { svg?: string; error?: string }) => {
    messages.push(message);
  };
  try {
    await import("../src/lib/viz-worker.ts");
    const onmessage = globals.onmessage as (event: { data: string }) => Promise<void>;
    assert.equal(typeof onmessage, "function");

    async function render(dot: string): Promise<string> {
      messages.length = 0;
      await onmessage({ data: dot });
      const message = messages.at(-1);
      assert.ok(message?.svg, `render failed: ${message?.error}`);
      return message.svg;
    }

    await t.test("default render is transparent with light text and muted strokes", async () => {
      const svg = await render("digraph { a -> b }");
      assert.doesNotMatch(svg, /fill="white"/);
      assert.match(svg, /fill="#e1e8ed"/);
      assert.match(svg, /stroke="#718494"/);
      assert.match(svg, /font-family="Courier,monospace"/);
    });

    await t.test("explicit edge color survives the defaults", async () => {
      const svg = await render("digraph { a -> b [color=red] }");
      assert.match(svg, /<g id="edge1"[\s\S]*?stroke="red"/);
      assert.match(svg, /stroke="#718494"/);
    });
  } finally {
    globals.onmessage = previousOnMessage;
    globals.postMessage = previousPostMessage;
  }
});
