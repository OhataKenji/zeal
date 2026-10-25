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

test("successful reads do not hide failed input or resize operations", async () => {
  const reports: (string | null)[] = [];
  let failWrite = true;
  let failResize = true;
  const console = createConsole(
    "owned",
    {
      invoke: async <T>(command: string) => {
        if (command === "write_console") {
          if (failWrite) throw new Error("input failed");
          return 1 as T;
        }
        if (command === "resize_console" && failResize) throw new Error("resize failed");
        return { data: [], offset: 0, done: false } as T;
      },
    },
    async () => {},
    (error) => {
      reports.push(error);
    },
  );
  console.input(Uint8Array.of(65));
  await console.poll();
  await console.resize(80, 24);
  await console.poll();
  assert.deepEqual(reports, ["Error: input failed", "Error: input failed\nError: resize failed"]);
  failWrite = false;
  console.input(Uint8Array.of(66));
  await console.poll();
  assert.equal(reports.at(-1), "Error: resize failed");
  failResize = false;
  await console.resize(80, 24);
  assert.equal(reports.at(-1), null);
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

test("partial writes and backpressure keep key bytes in order", async () => {
  const received: number[] = [];
  let blocked = true;
  const console = createConsole(
    "owned",
    {
      invoke: async <T>(command: string, args?: Record<string, unknown>) => {
        assert.equal(args?.id, "owned");
        if (command === "read_console") return { data: [], offset: 0, done: false } as T;
        const data = args?.data as number[];
        if (blocked) return 0 as T;
        received.push(data[0]);
        return 1 as T;
      },
    },
    async () => {},
    (error) => {
      assert.fail(error!);
    },
  );
  console.input(Uint8Array.from([27, 91, 65]));
  await console.poll();
  blocked = false;
  console.input(Uint8Array.from([3]));
  // flush awaits each IPC response; a bounded microtask drain makes this deterministic.
  await console.poll();
  await console.poll();
  await console.poll();
  assert.deepEqual(received, [27, 91, 65, 3]);
});

test("closing a Console ignores late output without stopping the process", async () => {
  let resolve!: (value: unknown) => void;
  const calls: string[] = [];
  const console = createConsole(
    "owned",
    {
      invoke: <T>(command: string) => {
        calls.push(command);
        return new Promise((yes) => {
          resolve = yes;
        }) as Promise<T>;
      },
    },
    async () => {
      assert.fail("late output rendered");
    },
    () => {
      assert.fail("late error");
    },
  );
  const reading = console.poll();
  console.dispose();
  resolve({ data: [65], offset: 1, done: false });
  await reading;
  console.input(Uint8Array.from([3]));
  await console.poll();
  assert.deepEqual(calls, ["read_console"]);
});
