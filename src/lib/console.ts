import { COMMANDS } from "./commands.ts";
import type { Transport } from "./session.ts";

/** Bytes read from the core's live console stream at a requested offset. */
export interface ConsoleOutput {
  data: number[];
  offset: number;
}

type InputQueue = { pending: Uint8Array };

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
  let inputOwner: InputQueue | "readonly" | "disposed" = { pending: new Uint8Array() };
  let reading = false;
  let writing = false;
  const errors: Record<"read" | "write" | "resize", string | null> = {
    read: null,
    write: null,
    resize: null,
  };
  let reported: string | null = null;

  function isDisposed() {
    return inputOwner === "disposed";
  }

  function setError(operation: keyof typeof errors, message: string | null) {
    if (isDisposed()) return;
    errors[operation] = message;
    const next = Object.values(errors).filter(Boolean).join("\n") || null;
    if (next !== reported) {
      reported = next;
      report(next);
    }
  }

  /** Sends one chunk of the input queue; false stops the flush loop. */
  async function writeChunk(queue: InputQueue): Promise<boolean> {
    const count = await transport.invoke<number>(COMMANDS.writeConsole, {
      workspace,
      id,
      data: Array.from(queue.pending.subarray(0, 8192)),
    });
    if (queue !== inputOwner || count === 0) return false;
    queue.pending = queue.pending.slice(count);
    setError("write", null);
    return true;
  }

  function failWrite(queue: InputQueue, error: unknown) {
    if (queue !== inputOwner) return;
    queue.pending = new Uint8Array();
    setError("write", String(error));
  }

  async function flush() {
    const queue = inputOwner;
    if (typeof queue === "string" || writing || !queue.pending.length) return;
    writing = true;
    try {
      while (queue === inputOwner && queue.pending.length) {
        if (!(await writeChunk(queue))) break;
      }
    } catch (error) {
      failWrite(queue, error);
    } finally {
      writing = false;
      if (queue !== inputOwner) void flush();
    }
  }

  function input(bytes: Uint8Array) {
    if (typeof inputOwner === "string") return;
    const { pending } = inputOwner;
    if (pending.length + bytes.length > 1024 * 1024) {
      setError("write", "Terminal input is busy; paste a smaller selection.");
      return;
    }
    const next = new Uint8Array(pending.length + bytes.length);
    next.set(pending);
    next.set(bytes, pending.length);
    inputOwner.pending = next;
    void flush();
  }

  /** Reads and displays one chunk; false stops the read loop. */
  async function readChunk(): Promise<boolean> {
    const result = await transport.invoke<ConsoleOutput>(COMMANDS.readConsole, {
      workspace,
      id,
      offset,
    });
    if (isDisposed()) return false;
    if (result.data.length) await display(Uint8Array.from(result.data));
    if (isDisposed()) return false;
    offset = result.offset;
    setError("read", null);
    return result.data.length >= 64 * 1024;
  }

  async function poll() {
    void flush();
    if (isDisposed() || reading) return;
    reading = true;
    try {
      do {
        if (!(await readChunk())) break;
      } while (!isDisposed());
    } catch (error) {
      setError("read", String(error));
    } finally {
      reading = false;
    }
  }

  async function resize(cols: number, rows: number) {
    const queue = inputOwner;
    if (typeof queue === "string") return;
    try {
      await transport.invoke(COMMANDS.resizeConsole, { workspace, id, cols, rows });
      if (queue === inputOwner) setError("resize", null);
    } catch (error) {
      if (queue === inputOwner) setError("resize", String(error));
    }
  }

  return {
    setWritable(active: boolean) {
      if (isDisposed()) return;
      if (!active) inputOwner = "readonly";
      else if (inputOwner === "readonly") inputOwner = { pending: new Uint8Array() };
    },
    input,
    poll,
    resize,
    dispose() {
      inputOwner = "disposed";
    },
  };
}
