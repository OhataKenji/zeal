/** PING/PONG handshake that hands a trace buffer to the embedded Perfetto
 * viewer exactly once. The trace bytes are only read after the viewer proves
 * it is the real embedded UI: a `PONG` from the viewer origin and frame.
 * Anything else is ignored, the send is latched before the async read, and
 * cleanup guarantees no send outlives the caller. */

/** Minimal shape of the iframe's content window the handshake needs. */
interface ViewerWindow {
  postMessage(message: unknown, targetOrigin: string, transfer?: Transferable[]): void;
}

interface ViewerFrame {
  contentWindow: ViewerWindow | null;
}

/** Event target the handshake listens on; `window` in the app, a fake in tests. */
export interface MessageTarget {
  addEventListener(type: "message", listener: (event: Event) => void): void;
  removeEventListener(type: "message", listener: (event: Event) => void): void;
}

export interface TraceHandshake {
  /** Reads the current frame; `contentWindow` is re-read per event. */
  frame: () => ViewerFrame | null | undefined;
  /** Origin or URL of the viewer as returned by `perfetto_origin`. */
  viewer: string;
  /** Reads the trace bytes; invoked at most once, only after a verified PONG. */
  load: () => Promise<ArrayBuffer>;
  title: string;
  onSent: () => void;
  onError: (error: unknown) => void;
  onTimeout: () => void;
  target?: MessageTarget;
  pingMs?: number;
  timeoutMs?: number;
}

const DEFAULT_PING_MS = 150;
const DEFAULT_TIMEOUT_MS = 20_000;

export function startTraceHandshake(options: TraceHandshake): () => void {
  const viewerOrigin = new URL(options.viewer).origin;
  const pingMs = options.pingMs ?? DEFAULT_PING_MS;
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  const target: MessageTarget = options.target ?? window;
  // `finished` latches the terminal state (sent or timed out); `cancelled`
  // additionally suppresses a still-pending async send after cleanup.
  let finished = false;
  let cancelled = false;

  const release = () => {
    clearInterval(timer);
    clearTimeout(deadline);
    target.removeEventListener("message", onMessage);
  };

  const onMessage = (event: Event) => {
    const message = event as MessageEvent;
    const content = options.frame()?.contentWindow ?? null;
    if (
      finished ||
      cancelled ||
      content === null ||
      message.origin !== viewerOrigin ||
      message.source !== content ||
      message.data !== "PONG"
    ) {
      return;
    }
    // Latch before the async read so a repeated PONG cannot send twice.
    finished = true;
    release();
    void options
      .load()
      .then((buffer) => {
        if (cancelled) return;
        content.postMessage({ perfetto: { buffer, title: options.title } }, viewerOrigin, [buffer]);
        options.onSent();
      })
      .catch((failure: unknown) => {
        if (!cancelled) options.onError(failure);
      });
  };

  const timer = setInterval(() => {
    if (!finished && !cancelled) {
      options.frame()?.contentWindow?.postMessage("PING", viewerOrigin);
    }
  }, pingMs);
  const deadline = setTimeout(() => {
    if (finished || cancelled) return;
    finished = true;
    release();
    options.onTimeout();
  }, timeoutMs);

  target.addEventListener("message", onMessage);
  return () => {
    cancelled = true;
    finished = true;
    release();
  };
}
