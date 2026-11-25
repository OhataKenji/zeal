export interface ClipboardFeedback {
  text: string;
  ok: boolean;
}

export function createClipboardFeedback(
  write: (text: string) => Promise<void>,
  publish: (feedback: ClipboardFeedback) => void,
) {
  let owner: symbol | "disposed" | undefined;

  async function copy(text: string) {
    if (owner === "disposed") return;
    const attempt = Symbol();
    owner = attempt;
    let ok = true;
    try {
      await write(text);
    } catch {
      ok = false;
    }
    if (owner === attempt) publish({ text, ok });
  }

  function dispose() {
    owner = "disposed";
  }

  return { copy, dispose };
}
