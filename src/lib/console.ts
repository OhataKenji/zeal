import { COMMANDS } from "./commands.ts";
import type { Transport } from "./session.ts";

/** Bytes read from the core's live console stream at a requested offset. */
export interface ConsoleOutput {
  data: number[];
  offset: number;
}

/**
 * Creates a terminal stream reader and bounded input queue for one live
 * invocation. Polling advances from the saved offset while the view is
 * mounted; a zero-length write waits for the next poll to retry the same
 * queued bytes. Releasing the invocation unmounts the view, which disposes
 * the stream — there is no completion protocol.
 */
export function createConsole(
  workspace: string,
  id: string,
  transport: Pick<Transport, "invoke">,
  display: (bytes: Uint8Array) => Promise<void>,
  report: (error: string | null) => void,
) {
  let offset = 0;
  let disposed = false;
  let writable = true;
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

  /** Sends one chunk of the input queue; false stops the flush loop. */
  async function writeChunk(): Promise<boolean> {
    const data = Array.from(pending.subarray(0, 8192));
    const count = await transport.invoke<number>(COMMANDS.writeConsole, {
      workspace,
      id,
      data,
    });
    if (disposed) return false;
    pending = pending.slice(count);
    if (count > 0) setError("write", null);
    return count > 0; // Backpressure: the next poll retries the same bytes.
  }

  async function flush() {
    if (disposed || !writable || writing || !pending.length) return;
    writing = true;
    try {
      while (!disposed && writable && pending.length) {
        if (!(await writeChunk())) break;
      }
    } catch (error) {
      pending = new Uint8Array();
      setError("write", String(error));
    } finally {
      writing = false;
    }
  }

  function input(bytes: Uint8Array) {
    if (disposed || !writable) return;
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

  /** Reads and displays one chunk; false stops the read loop. */
  async function readChunk(): Promise<boolean> {
    const result = await transport.invoke<ConsoleOutput>(COMMANDS.readConsole, {
      workspace,
      id,
      offset,
    });
    if (disposed) return false;
    if (result.data.length) await display(Uint8Array.from(result.data));
    if (disposed) return false;
    offset = result.offset;
    setError("read", null);
    return result.data.length >= 64 * 1024;
  }

  async function poll() {
    void flush();
    if (disposed || reading) return;
    reading = true;
    try {
      do {
        if (!(await readChunk())) break;
      } while (!disposed);
    } catch (error) {
      setError("read", String(error));
    } finally {
      reading = false;
    }
  }

  async function resize(cols: number, rows: number) {
    if (disposed || !writable) return;
    try {
      await transport.invoke(COMMANDS.resizeConsole, { workspace, id, cols, rows });
      setError("resize", null);
    } catch (error) {
      setError("resize", String(error));
    }
  }

  return {
    setWritable(active: boolean) {
      writable = active;
      if (!active) pending = new Uint8Array();
    },
    input,
    poll,
    resize,
    dispose() {
      disposed = true;
      pending = new Uint8Array();
    },
  };
}
