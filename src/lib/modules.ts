import { COMMANDS } from "./commands.ts";
import type { Transport } from "./query.ts";
import type { ModuleNode, ModulesSnapshot } from "./types.ts";

export type ModuleDepth = "all" | "direct" | "transitive";

export function filterModules(modules: readonly ModuleNode[], query: string, depth: ModuleDepth) {
  const term = query.trim().toLowerCase();
  return modules.filter(
    (node) =>
      (depth === "all" || node.direct === (depth === "direct")) &&
      [node.name, node.version, node.key].some((field) => field.toLowerCase().includes(term)),
  );
}

export interface ModulesState {
  snapshot: ModulesSnapshot | null;
  selectedKey: string | null;
  busy: "load" | "explain" | null;
  loadError: string | null;
  explanation: string | null;
  explainError: string | null;
}

export function emptyModulesState(): ModulesState {
  return {
    snapshot: null,
    selectedKey: null,
    busy: null,
    loadError: null,
    explanation: null,
    explainError: null,
  };
}

/** One workspace-keyed view owns one in-flight request and at most one explanation. */
export function createModules(
  workspace: string,
  transport: Transport,
  state: ModulesState,
  bazelAvailable: () => boolean = () => true,
) {
  let disposed = false;

  function update(patch: Partial<ModulesState>) {
    if (disposed) return;
    Object.assign(state, patch);
  }

  async function load() {
    if (disposed || state.busy || !bazelAvailable()) return;
    update({ busy: "load", loadError: null });
    try {
      const snapshot = await transport.invoke<ModulesSnapshot>(COMMANDS.getModules, { workspace });
      const selectedKey = snapshot.modules.some((node) => node.key === state.selectedKey)
        ? state.selectedKey
        : null;
      update({ snapshot, selectedKey, busy: null, explanation: null, explainError: null });
    } catch (error) {
      update({ busy: null, loadError: String(error) });
    }
  }

  function select(key: string | null) {
    if (key !== state.selectedKey)
      update({ selectedKey: key, explanation: null, explainError: null });
  }

  async function explain() {
    const moduleKey = state.selectedKey;
    if (disposed || state.busy || !moduleKey || !bazelAvailable()) return;
    update({ busy: "explain", explanation: null, explainError: null });
    let result: Pick<ModulesState, "explanation" | "explainError">;
    try {
      const explanation = await transport.invoke<string>(COMMANDS.explainModule, {
        workspace,
        moduleKey,
      });
      result = { explanation, explainError: null };
    } catch (error) {
      result = { explanation: null, explainError: String(error) };
    }
    update({ busy: null, ...(state.selectedKey === moduleKey ? result : {}) });
  }

  return {
    load,
    select,
    explain,
    dispose: () => {
      disposed = true;
    },
  };
}
