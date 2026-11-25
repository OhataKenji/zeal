import { test } from "node:test";
import { setImmediate } from "node:timers/promises";
import assert from "node:assert/strict";
import { createConsole } from "../src/lib/console.ts";

test("a successful output read clears its previous error and retries the same offset", async () => {
  const reports: Array<string | null> = [];
  const displayed: number[] = [];
  let fail = true;
  const console = createConsole(
    "/workspace",
    "owned",
    {
      invoke: async <T>(command: string, args?: Record<string, unknown>) => {
        assert.equal(command, "read_console");
        assert.equal(args?.offset, 0);
        if (fail) throw new Error("history storage permission or file safety error");
        return { data: [65], offset: 1 } as T;
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
  const reports: Array<string | null> = [];
  let failWrite = true;
  let failResize = true;
  const console = createConsole(
    "/workspace",
    "owned",
    {
      invoke: async <T>(command: string) => {
        if (command === "write_console") {
          if (failWrite) throw new Error("input failed");
          return 1 as T;
        }
        if (command === "resize_console" && failResize) throw new Error("resize failed");
        return { data: [], offset: 0 } as T;
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
    "/workspace",
    "owned",
    {
      invoke: async <T>(command: string, args?: Record<string, unknown>) => {
        assert.equal(command, "read_console");
        offsets.push(Number(args?.offset));
        const offset = Number(args?.offset);
        const data = Array.from(bytes.slice(offset, offset + 2));
        return { data, offset: offset + data.length } as T;
      },
    },
    async (data) => {
      displayed.push(...data);
    },
    (error) => {
      assert.ok(error);
      assert.fail(error);
    },
  );
  await console.poll();
  await console.poll();
  await console.poll();
  await console.poll();
  assert.deepEqual(offsets, [0, 2, 4, 6]);
  assert.deepEqual(displayed, Array.from(bytes));
  console.dispose();
});

test("partial writes and backpressure keep key bytes in order", async () => {
  const received: number[] = [];
  let blocked = true;
  const console = createConsole(
    "/workspace",
    "owned",
    {
      invoke: async <T>(command: string, args?: Record<string, unknown>) => {
        assert.equal(args?.id, "owned");
        if (command === "read_console") return { data: [], offset: 0 } as T;
        const data = args?.data as number[];
        if (blocked) return 0 as T;
        received.push(data[0]);
        return 1 as T;
      },
    },
    async () => {},
    (error) => {
      assert.ok(error);
      assert.fail(error);
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

for (const outcome of ["success", "failure"]) {
  test(`a stale write ${outcome} leaves reactivated input intact`, async () => {
    const writes: number[][] = [];
    const reports: Array<string | null> = [];
    let resolve!: (count: number) => void;
    let reject!: (error: Error) => void;
    const oldWrite = new Promise<number>((yes, no) => {
      resolve = yes;
      reject = no;
    });
    const console = createConsole(
      "/workspace",
      "owned",
      {
        invoke: async <T>(command: string, args?: Record<string, unknown>) => {
          assert.equal(command, "write_console");
          const data = args?.data as number[];
          writes.push(data);
          return (writes.length === 1 ? await oldWrite : data.length) as T;
        },
      },
      async () => {},
      (error) => reports.push(error),
    );
    console.input(Uint8Array.of(1));
    console.setWritable(false);
    console.setWritable(true);
    console.input(Uint8Array.of(2, 3));
    console.setWritable(true);
    assert.deepEqual(writes, [[1]]);
    if (outcome === "success") resolve(1);
    else reject(new Error("stale write"));
    await setImmediate();
    assert.deepEqual(reports, []);
    assert.deepEqual(writes, [[1], [2, 3]]);
  });
}

test("a stale resize reply does not clear the current resize error", async () => {
  const reports: Array<string | null> = [];
  let resolve!: () => void;
  const oldResize = new Promise<void>((yes) => {
    resolve = yes;
  });
  const console = createConsole(
    "/workspace",
    "owned",
    {
      invoke: async <T>(command: string, args?: Record<string, unknown>) => {
        assert.equal(command, "resize_console");
        if (args?.cols === 80) return (await oldResize) as T;
        throw new Error("current resize");
      },
    },
    async () => {},
    (error) => reports.push(error),
  );
  const resizing = console.resize(80, 24);
  console.setWritable(false);
  console.setWritable(true);
  await console.resize(90, 24);
  resolve();
  await resizing;
  assert.deepEqual(reports, ["Error: current resize"]);
});

test("closing a Console ignores late output without stopping the process", async () => {
  // The Promise executor assigns the callback before this helper returns.
  let resolve!: (value: unknown) => void;
  const calls: string[] = [];
  const console = createConsole(
    "/workspace",
    "owned",
    {
      invoke: <T>(command: string) => {
        calls.push(command);
        return new Promise<unknown>((yes) => {
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
  console.setWritable(false);
  console.setWritable(true);
  resolve({ data: [65], offset: 1 });
  await reading;
  console.input(Uint8Array.from([3]));
  await console.resize(80, 24);
  await console.poll();
  assert.deepEqual(calls, ["read_console"]);
});

test("a non-writable Console stops input and resize but keeps reading", async () => {
  const calls: string[] = [];
  const displayed: number[] = [];
  const replies = [
    { data: [65], offset: 1 },
    { data: [66, 67], offset: 3 },
    { data: [], offset: 3 },
  ];
  const console = createConsole(
    "/workspace",
    "owned",
    {
      invoke: async <T>(command: string) => {
        calls.push(command);
        assert.equal(command, "read_console");
        return replies.shift() as T;
      },
    },
    async (bytes) => {
      displayed.push(...bytes);
    },
    (error) => {
      assert.equal(error, null);
    },
  );
  await console.poll();
  console.setWritable(false);
  console.input(Uint8Array.of(68));
  await console.resize(80, 24);
  await console.poll();
  await console.poll();
  assert.deepEqual(displayed, [65, 66, 67]);
  assert.deepEqual(calls, ["read_console", "read_console", "read_console"]);
});

test("read, write, and resize payloads carry the captured workspace and id", async () => {
  const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
  const console = createConsole(
    "/workspace-a",
    "inv-a",
    {
      invoke: async <T>(command: string, args?: Record<string, unknown>) => {
        calls.push({ command, args: args ?? {} });
        if (command === "write_console") return 1 as T;
        if (command === "resize_console") return undefined as T;
        return { data: [], offset: 0 } as T;
      },
    },
    async () => {},
    (error) => {
      assert.equal(error, null);
    },
  );
  console.input(Uint8Array.of(66));
  await console.poll();
  await console.resize(80, 24);
  await console.poll();
  assert.deepEqual(
    calls.map((call) => call.command),
    ["write_console", "read_console", "resize_console", "read_console"],
  );
  for (const call of calls) {
    assert.equal(call.args.workspace, "/workspace-a");
    assert.equal(call.args.id, "inv-a");
  }
});

test("a non-writable Console discards pending input under backpressure but keeps reading", async () => {
  const calls: string[] = [];
  const console = createConsole(
    "/workspace",
    "owned",
    {
      invoke: async <T>(command: string) => {
        calls.push(command);
        return (command === "write_console" ? 0 : { data: [], offset: 0 }) as T;
      },
    },
    async () => {},
    (error) => {
      assert.equal(error, null);
    },
  );
  console.input(Uint8Array.of(65));
  await console.poll();
  const writes = calls.filter((command) => command === "write_console").length;
  console.setWritable(false);
  await console.poll();
  assert.equal(calls.filter((command) => command === "write_console").length, writes);
  assert.equal(calls.at(-1), "read_console");
});
