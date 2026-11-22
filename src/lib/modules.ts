import { COMMANDS } from "./commands.ts";
import type { Transport } from "./query.ts";
import type { ModuleExplanation, ModuleNode, ModulesSnapshot } from "./types.ts";

export type ModuleDepth = "all" | "direct" | "transitive";

export function filterModules(modules: readonly ModuleNode[], query: string, depth: ModuleDepth) {
  const term = query.trim().toLowerCase();
  return modules.filter(
    (node) =>
      (depth === "all" || node.direct === (depth === "direct")) &&
      [node.name, node.version, node.key].some((field) => field.toLowerCase().includes(term)),
  );
}

type ExplanationResult = ModuleExplanation | { error: string };

export interface ModulesState {
  snapshot: ModulesSnapshot | null;
  selectedKey: string | null;
  busy: "load" | "explain" | null;
  loadError: string | null;
  explanation: ExplanationResult | null;
}

export function emptyModulesState(): ModulesState {
  return {
    snapshot: null,
    selectedKey: null,
    busy: null,
    loadError: null,
    explanation: null,
  };
}

/** Tracks at most one in-flight module request and one explanation for the selected workspace. */
export function createModules(
  workspace: string,
  transport: Transport,
  bazelAvailable: () => boolean,
  changed: () => void,
) {
  let state = emptyModulesState();
  let disposed = false;

  function update(patch: Partial<ModulesState>) {
    if (disposed) return;
    state = { ...state, ...patch };
    changed();
  }

  async function load() {
    if (disposed || state.busy || !bazelAvailable()) return;
    update({ busy: "load", loadError: null });
    try {
      const snapshot = await transport.invoke<ModulesSnapshot>(COMMANDS.getModules, { workspace });
      const selectedKey = snapshot.modules.some((node) => node.key === state.selectedKey)
        ? state.selectedKey
        : null;
      update({ snapshot, selectedKey, busy: null, explanation: null });
    } catch (error) {
      update({ busy: null, loadError: String(error) });
    }
  }

  function select(key: string | null) {
    if (key !== state.selectedKey) update({ selectedKey: key, explanation: null });
  }

  async function explain() {
    const moduleKey = state.selectedKey;
    if (disposed || state.busy || !moduleKey || !bazelAvailable()) return;
    update({ busy: "explain", explanation: null });
    let explanation: ExplanationResult;
    try {
      explanation = await transport.invoke<ModuleExplanation>(COMMANDS.explainModule, {
        workspace,
        moduleKey,
      });
    } catch (error) {
      explanation = { error: String(error) };
    }
    update({ busy: null, explanation: state.selectedKey === moduleKey ? explanation : null });
  }

  return {
    load,
    select,
    explain,
    get state() {
      return state;
    },
    dispose: () => {
      disposed = true;
    },
  };
}
