interface GraphWorker {
  onmessage: ((event: MessageEvent<{ svg?: string; error?: string }>) => void) | null;
  onerror: ((event: ErrorEvent) => void) | null;
  postMessage(dot: string): void;
  terminate(): void;
}

type State =
  | { kind: "disposed" }
  | { kind: "cold" }
  | { kind: "warm"; worker: GraphWorker }
  | { kind: "rendering"; worker: GraphWorker; owner: () => void };

const VIZ_TIMEOUT = 30_000;

export function createGraphRenderer(makeWorker: () => GraphWorker) {
  let state: State = { kind: "cold" };

  function render(dot: string, ok: (svg: string) => void, fail: (error: string) => void) {
    if (state.kind === "disposed") return () => {};
    if (state.kind === "rendering") state.owner();
    const worker = state.kind === "warm" ? state.worker : makeWorker();
    // The cancel function identifies this render even if the worker is reused.
    function cancel() {
      finish("cold");
    }
    function finish(next: "cold" | "warm") {
      if (state.kind !== "rendering" || state.owner !== cancel) return false;
      clearTimeout(timer);
      worker.onmessage = null;
      worker.onerror = null;
      if (next === "warm") {
        state = { kind: "warm", worker };
      } else {
        state = { kind: "cold" };
        worker.terminate();
      }
      return true;
    }

    state = { kind: "rendering", worker, owner: cancel };
    const timer = setTimeout(() => {
      if (finish("cold")) fail("Graphviz could not lay out this result within 30s.");
    }, VIZ_TIMEOUT);
    worker.onmessage = ({ data }) => {
      if (!finish("warm")) return;
      if (data.error) fail(data.error);
      else ok(data.svg ?? "");
    };
    worker.onerror = (event) => {
      if (finish("cold")) fail(event.message || "the graph renderer crashed");
    };
    worker.postMessage(dot);
    return cancel;
  }

  function dispose() {
    if (state.kind === "rendering") state.owner();
    else if (state.kind === "warm") state.worker.terminate();
    state = { kind: "disposed" };
  }

  return { render, dispose };
}
