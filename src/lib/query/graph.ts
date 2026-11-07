export interface GraphState {
  url: string | null;
  error: string | null;
  loading: boolean;
}

export type GraphWorker = Pick<Worker, "onmessage" | "onerror" | "postMessage" | "terminate">;

export function createGraphRenderer(
  createWorker: () => GraphWorker,
  changed: (state: GraphState) => void,
): { render(dot: string | null): void; dispose(): void } {
  // The Graphviz wasm is only fetched when a graph result is rendered. A
  // completed worker is reused within this renderer; abandoning a pending
  // layout terminates it, and disposing the renderer releases all resources.
  let worker: GraphWorker | null = null;
  let cancel: (() => void) | null = null;
  let url: string | null = null;
  let generation = 0;
  let disposed = false;

  function publish(state: GraphState) {
    if (!disposed) changed(state);
  }

  function dropWorker(w: GraphWorker) {
    w.terminate();
    if (worker === w) worker = null;
  }

  function cancelPending() {
    cancel?.();
    cancel = null;
  }

  function revokeUrl() {
    if (url !== null) {
      URL.revokeObjectURL(url);
      url = null;
    }
  }

  function render(dot: string | null): void {
    if (disposed) return;
    const gen = ++generation;
    cancelPending();
    revokeUrl();
    if (dot === null || dot.trim() === "") {
      publish({ url: null, error: null, loading: false });
      return;
    }
    const w = (worker ??= createWorker());
    publish({ url: null, error: null, loading: true });
    const settle = () => {
      cancel = null;
      w.onmessage = null;
      w.onerror = null;
      clearTimeout(timer);
    };
    const timer = setTimeout(() => {
      if (gen !== generation || disposed) return;
      settle();
      dropWorker(w);
      publish({
        url: null,
        error: "Graphviz could not lay out this result within 30s.",
        loading: false,
      });
    }, 30_000);
    w.onmessage = (e) => {
      if (gen !== generation || disposed) return;
      settle();
      const d = e.data as { svg?: string; error?: string };
      if (d.error) {
        publish({ url: null, error: d.error, loading: false });
        return;
      }
      url = URL.createObjectURL(new Blob([d.svg ?? ""], { type: "image/svg+xml" }));
      publish({ url, error: null, loading: false });
    };
    w.onerror = (e) => {
      if (gen !== generation || disposed) return;
      settle();
      dropWorker(w);
      publish({
        url: null,
        error: e.message || "the graph renderer crashed",
        loading: false,
      });
    };
    // Abandoning a render means killing the worker: the queued DOT would
    // otherwise delay the next layout by whatever time it still needs.
    cancel = () => {
      settle();
      dropWorker(w);
    };
    w.postMessage(dot);
  }

  function dispose(): void {
    if (disposed) return;
    disposed = true;
    generation++;
    cancelPending();
    if (worker) {
      worker.terminate();
      worker = null;
    }
    revokeUrl();
  }

  return { render, dispose };
}
