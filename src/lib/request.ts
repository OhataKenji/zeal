/**
 * Explicit owned async request state machine.
 *
 * States:
 *   - idle:     no active request, no value (initial or after invalidate)
 *   - loading:  in-flight request owned by `owner`, optionally retaining `value`
 *   - ready:    settled with `value`
 *   - failed:   settled with `error` (stale value cleared)
 *   - disposed: terminal; ignores all new requests and late replies
 *
 * Formal specification: `specs/AsyncRequest.tla`.
 */
export type RequestState<T> =
  | { kind: "idle"; value: null; error: null }
  | { kind: "loading"; value: T | null; error: null }
  | { kind: "ready"; value: T; error: null }
  | { kind: "failed"; value: null; error: string }
  | { kind: "disposed"; value: T | null; error: string | null };

export function idleRequest<T>(): RequestState<T> {
  return { kind: "idle", value: null, error: null };
}

export interface RequestController<T> {
  readonly state: RequestState<T>;
  run(work: () => Promise<T>, options?: { retainValue?: boolean }): Promise<T | undefined>;
  invalidate(): void;
  fail(error: string): void;
  dispose(): void;
}

export function createRequest<T>(changed: (state: RequestState<T>) => void): RequestController<T> {
  let state: RequestState<T> = idleRequest<T>();
  let owner: object | null = null;

  function publish(next: RequestState<T>) {
    state = next;
    changed(state);
  }

  async function run(
    work: () => Promise<T>,
    options?: { retainValue?: boolean },
  ): Promise<T | undefined> {
    if (state.kind === "disposed") return undefined;
    const token = {};
    owner = token;
    const retained = options?.retainValue ? state.value : null;
    publish({ kind: "loading", value: retained, error: null });
    try {
      const value = await work();
      if (owner !== token) return undefined;
      owner = null;
      publish({ kind: "ready", value, error: null });
      return value;
    } catch (error) {
      if (owner !== token) return undefined;
      owner = null;
      publish({ kind: "failed", value: null, error: String(error) });
      return undefined;
    }
  }

  function invalidate() {
    if (state.kind === "disposed") return;
    owner = null;
    publish(idleRequest<T>());
  }

  function fail(error: string) {
    if (state.kind === "disposed") return;
    owner = null;
    publish({ kind: "failed", value: null, error });
  }

  function dispose() {
    if (state.kind === "disposed") return;
    owner = null;
    state = { kind: "disposed", value: state.value, error: state.error };
  }

  return {
    get state() {
      return state;
    },
    run,
    invalidate,
    fail,
    dispose,
  };
}
