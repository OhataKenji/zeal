import { test } from "node:test";
import assert from "node:assert/strict";
import { createTelemetry } from "../src/lib/telemetry.ts";

test("disabled telemetry neither adds arguments nor invokes the backend", async () => {
  let calls = 0;
  const telemetry = createTelemetry(
    async <T>() => {
      calls++;
      return undefined as T;
    },
    () => false,
  );
  const operation = telemetry.begin("start_run");
  assert.deepEqual(operation.args, {});
  operation.finish("applied");
  telemetry.stateApplied(crypto.randomUUID());
  await Promise.resolve();
  assert.equal(calls, 0);
});

test("an operation reports once after the UI update with only allowlisted fields", async () => {
  let updated!: () => void;
  const update = new Promise<void>((resolve) => {
    updated = resolve;
  });
  let reported!: (args: Record<string, unknown>) => void;
  const report = new Promise<Record<string, unknown>>((resolve) => {
    reported = resolve;
  });
  let calls = 0;
  const telemetry = createTelemetry(
    async <T>(command, args) => {
      assert.equal(command, "record_ui_operation");
      calls++;
      reported(args!);
      return undefined as T;
    },
    () => true,
    () => update,
  );
  const operation = telemetry.begin("get_run");
  operation.finish("discarded");
  operation.finish("applied");
  await Promise.resolve();
  assert.equal(calls, 0);
  updated();
  const args = await report;
  const record = args.record as Record<string, unknown>;
  assert.deepEqual(Object.keys(record).sort(), ["command", "durationMs", "operationId", "outcome"]);
  assert.equal(record.operationId, operation.args.operationId);
  assert.equal(record.command, "get_run");
  assert.equal(record.outcome, "discarded");
  assert.ok(Number.isFinite(record.durationMs));
  assert.equal(calls, 1);
});

test("a stuck reporting channel has a fixed pending limit", async () => {
  const calls: unknown[] = [];
  let full!: () => void;
  const sent = new Promise<void>((resolve) => {
    full = resolve;
  });
  const telemetry = createTelemetry(
    <T>(_command: string, args?: Record<string, unknown>) => {
      calls.push(args);
      if (calls.length === 32) full();
      return new Promise<T>(() => {});
    },
    () => true,
  );
  for (let i = 0; i < 100; i++) telemetry.begin("list_runs").finish("applied");
  await sent;
  assert.equal(calls.length, 32);
});

test("a synchronous UI update failure cannot escape into an operation", async () => {
  const telemetry = createTelemetry(
    async <T>() => undefined as T,
    () => true,
    () => {
      throw new Error("update unavailable");
    },
  );
  assert.doesNotThrow(() => telemetry.begin("refresh").finish("failed"));
  await Promise.resolve();
});
