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

const busyByPhase = {
  idle: null,
  loading: "load",
  explaining: "explain",
  draining: "explain",
} as const;

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
    busy: busyByPhase.idle,
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
  let phase: keyof typeof busyByPhase | "disposed" = "idle";

  function update(next: keyof typeof busyByPhase, patch: Partial<Omit<ModulesState, "busy">>) {
    if (phase === "disposed") return;
    phase = next;
    state = { ...state, ...patch, busy: busyByPhase[phase] };
    changed();
  }

  async function load() {
    if (phase !== "idle" || !bazelAvailable()) return;
    update("loading", { loadError: null });
    try {
      const snapshot = await transport.invoke<ModulesSnapshot>(COMMANDS.getModules, { workspace });
      const selectedKey = snapshot.modules.some((node) => node.key === state.selectedKey)
        ? state.selectedKey
        : null;
      update("idle", { snapshot, selectedKey, explanation: null });
    } catch (error) {
      update("idle", { loadError: String(error) });
    }
  }

  function select(key: string | null) {
    if (key === state.selectedKey || phase === "disposed") return;
    update(phase === "explaining" ? "draining" : phase, { selectedKey: key, explanation: null });
  }

  function finishExplanation(explanation: ExplanationResult) {
    update("idle", { explanation: phase === "explaining" ? explanation : null });
  }

  async function explain() {
    const moduleKey = state.selectedKey;
    if (phase !== "idle" || !moduleKey || !bazelAvailable()) return;
    update("explaining", { explanation: null });
    let explanation: ExplanationResult;
    try {
      explanation = await transport.invoke<ModuleExplanation>(COMMANDS.explainModule, {
        workspace,
        moduleKey,
      });
    } catch (error) {
      explanation = { error: String(error) };
    }
    finishExplanation(explanation);
  }

  return {
    load,
    select,
    explain,
    get state() {
      return state;
    },
    dispose: () => {
      phase = "disposed";
    },
  };
}
