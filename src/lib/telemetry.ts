type Command = "open_workspace" | "refresh" | "start_run" | "stop_run" | "list_runs" | "get_run";
type Outcome = "applied" | "discarded" | "failed";
type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export interface UiOperation {
  args: Record<string, unknown>;
  finish(outcome: Outcome): void;
}

export interface Telemetry {
  begin(command: Command): UiOperation;
  stateApplied(operationId: string): void;
}

const noopOperation: UiOperation = { args: {}, finish() {} };
export const disabledTelemetry: Telemetry = {
  begin: () => noopOperation,
  stateApplied() {},
};

// Only fixed operation names, random IDs, outcomes and durations leave the UI.
// Human-paced operations need no durable buffer; cap pending reports and drop on failure.
export function createTelemetry(
  invoke: Invoke,
  enabled: () => boolean,
  afterUpdate: () => Promise<void> = () => Promise.resolve(),
): Telemetry {
  let pending = 0;

  function report(
    command: Command | "state_applied",
    operationId: string,
    started: number,
    outcome: Outcome,
  ) {
    if (!enabled() || pending >= 32) return;
    pending++;
    void Promise.resolve()
      .then(afterUpdate)
      .then(() =>
        invoke("record_ui_operation", {
          record: { command, operationId, durationMs: performance.now() - started, outcome },
        }),
      )
      .catch(() => {})
      .finally(() => pending--);
  }

  return {
    begin(command) {
      if (!enabled()) return noopOperation;
      const operationId = crypto.randomUUID();
      const started = performance.now();
      let finished = false;
      return {
        args: { operationId },
        finish(outcome) {
          if (finished) return;
          finished = true;
          report(command, operationId, started, outcome);
        },
      };
    },
    stateApplied(operationId) {
      report("state_applied", operationId, performance.now(), "applied");
    },
  };
}
