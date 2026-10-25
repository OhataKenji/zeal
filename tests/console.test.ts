import { test } from "node:test";
import assert from "node:assert/strict";
import { createConsole } from "../src/lib/console.ts";

test("a successful output read clears its previous error and retries the same offset", async () => {
  const reports: (string | null)[] = [];
  const displayed: number[] = [];
  let fail = true;
  const console = createConsole(
    "owned",
    {
      invoke: async <T>(_command: string, args?: Record<string, unknown>) => {
        assert.equal(args?.offset, 0);
        if (fail) throw new Error("history storage permission or file safety error");
        return { data: [65], offset: 1, done: true } as T;
      },
    },
    async (data) => {
      displayed.push(...data);
    },
    (error) => {
      reports.push(error);
    },
  );
  await console.poll();
  await console.poll();
  fail = false;
  await console.poll();
  assert.deepEqual(reports, ["Error: history storage permission or file safety error", null]);
  assert.deepEqual(displayed, [65]);
});

test("output is delivered once in byte order, including split UTF-8", async () => {
  const bytes = new TextEncoder().encode("日本\r\n");
  const displayed: number[] = [];
  const offsets: number[] = [];
  const console = createConsole(
    "owned",
    {
      invoke: async <T>(_command: string, args?: Record<string, unknown>) => {
        offsets.push(Number(args?.offset));
        const offset = Number(args?.offset);
        const data = Array.from(bytes.slice(offset, offset + 2));
        return {
          data,
          offset: offset + data.length,
          done: offset + data.length === bytes.length,
        } as T;
      },
    },
    async (data) => {
      displayed.push(...data);
    },
    (error) => {
      assert.fail(error!);
    },
  );
  await console.poll();
  await console.poll();
  await console.poll();
  await console.poll();
  await console.poll();
  assert.deepEqual(offsets, [0, 2, 4, 6]);
  assert.deepEqual(displayed, Array.from(bytes));
});
