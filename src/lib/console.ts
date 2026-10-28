import type { Transport } from "./session.ts";

/** Bytes read from the core's durable console stream at a requested offset. */
export interface ConsoleOutput {
  data: number[];
  offset: number;
  done: boolean;
}

/**
 * Creates a terminal stream reader and bounded input queue for one run.
 * Polling advances from the saved offset; a zero-length write waits for the
 * next poll to retry the same queued bytes.
 */
export function createConsole(
  id: string,
  transport: Pick<Transport, "invoke">,
  display: (bytes: Uint8Array) => Promise<void>,
  report: (error: string | null) => void,
) {
  let offset = 0;
  let disposed = false;
  let done = false;
  let reading = false;
  let writing = false;
  let pending = new Uint8Array();
  const errors: Record<"read" | "write" | "resize", string | null> = {
    read: null,
    write: null,
    resize: null,
  };
  let reported: string | null = null;

  function setError(operation: keyof typeof errors, message: string | null) {
    if (disposed) return;
    errors[operation] = message;
    const next = Object.values(errors).filter(Boolean).join("\n") || null;
    if (next !== reported) {
      reported = next;
      report(next);
    }
  }

  async function flush() {
    if (disposed || done || writing || !pending.length) return;
    writing = true;
    try {
      while (!disposed && !done && pending.length) {
        const data = Array.from(pending.subarray(0, 8192));
        const count = await transport.invoke<number>("write_console", { id, data });
        if (disposed) return;
        pending = pending.slice(count);
        if (count > 0) setError("write", null);
        if (count === 0) break; // Backpressure: the next poll retries the same bytes.
      }
    } catch (error) {
      pending = new Uint8Array();
      setError("write", String(error));
    } finally {
      writing = false;
    }
  }

  function input(bytes: Uint8Array) {
    if (disposed || done) return;
    if (pending.length + bytes.length > 1024 * 1024) {
      setError("write", "Terminal input is busy; paste a smaller selection.");
      return;
    }
    const next = new Uint8Array(pending.length + bytes.length);
    next.set(pending);
    next.set(bytes, pending.length);
    pending = next;
    void flush();
  }

  async function poll() {
    void flush();
    if (disposed || done || reading) return;
    reading = true;
    try {
      do {
        const result = await transport.invoke<ConsoleOutput>("read_console", { id, offset });
        if (disposed) return;
        if (result.data.length) await display(Uint8Array.from(result.data));
        if (disposed) return;
        offset = result.offset;
        done = result.done;
        setError("read", null);
        if (done) pending = new Uint8Array();
        if (result.data.length < 64 * 1024 || done) break;
      } while (!disposed);
    } catch (error) {
      setError("read", String(error));
    } finally {
      reading = false;
    }
  }

  async function resize(cols: number, rows: number) {
    if (disposed || done) return;
    try {
      await transport.invoke("resize_console", { id, cols, rows });
      setError("resize", null);
    } catch (error) {
      setError("resize", String(error));
    }
  }

  return {
    input,
    poll,
    resize,
    dispose() {
      disposed = true;
      pending = new Uint8Array();
    },
  };
}
