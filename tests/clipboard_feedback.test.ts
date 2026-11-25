import assert from "node:assert/strict";
import { test } from "node:test";
import { createClipboardFeedback, type ClipboardFeedback } from "../src/lib/clipboard-feedback.ts";

function setup() {
  const writes: Array<{ text: string; resolve: () => void; reject: (error: Error) => void }> = [];
  const results: ClipboardFeedback[] = [];
  const clipboard = createClipboardFeedback(
    (text) => new Promise<void>((resolve, reject) => writes.push({ text, resolve, reject })),
    (feedback) => results.push(feedback),
  );
  return { clipboard, writes, results };
}

test("an older successful copy cannot replace a newer failure for the same text", async () => {
  const { clipboard, writes, results } = setup();
  const first = clipboard.copy("command");
  const second = clipboard.copy("command");
  assert.deepEqual(
    writes.map((write) => write.text),
    ["command", "command"],
  );
  writes[1].reject(new Error("clipboard unavailable"));
  await second;
  writes[0].resolve();
  await first;
  assert.deepEqual(results, [{ text: "command", ok: false }]);
});

test("disposing during a copy suppresses its feedback and all future writes", async () => {
  const { clipboard, writes, results } = setup();
  const pending = clipboard.copy("before");
  clipboard.dispose();
  writes[0].resolve();
  await pending;
  const after = clipboard.copy("after");
  assert.equal(writes.length, 1);
  await after;
  assert.deepEqual(results, []);
});
