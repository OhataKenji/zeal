import type { Transport } from "./session.ts";

export interface ConsoleOutput {
  data: number[];
  offset: number;
  done: boolean;
}

// One mounted terminal owns its byte offset. Reopening replays the
// durable byte stream; it never starts or stops a process.
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
  let error: string | null = null;
  function setError(message: string | null) {
    if (message !== error) {
      error = message;
      report(message);
    }
  }

  async function poll() {
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
        setError(null);
        if (result.data.length < 64 * 1024 || done) break;
      } while (!disposed);
    } catch (error) {
      setError(String(error));
    } finally {
      reading = false;
    }
  }

  return {
    poll,
    dispose() {
      disposed = true;
    },
  };
}
