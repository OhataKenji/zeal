import { COMMANDS, type Transport } from "../commands.ts";
import type { InvocationDetail, InvocationRecord, Profile, Section } from "../types.ts";

export interface InvocationData {
  detail: InvocationDetail | null;
  detailLoading: boolean;
  detailError: string | null;
  profile: Section<Profile> | null;
  profileLoading: boolean;
  profileError: string | null;
}

export interface InvocationsView extends InvocationData {
  records: InvocationRecord[];
  error: string | null;
}

export interface InvocationsContext {
  workspace: string | null;
  selectedId: string | null;
  activeId: string | null;
  canReadArtifacts: boolean;
}

export function emptyInvocations(): InvocationsView {
  return {
    records: [],
    error: null,
    detail: null,
    detailLoading: false,
    detailError: null,
    profile: null,
    profileLoading: false,
    profileError: null,
  };
}

export function createInvocations(
  transport: Pick<Transport, "invoke">,
  changed: (view: InvocationsView) => void,
): {
  sync(context: InvocationsContext): void;
  clearSelected(): void;
  loadList(): Promise<void>;
  loadDetail(): Promise<void>;
  loadProfile(): Promise<void>;
  loadSelected(): void;
  dispose(): void;
} {
  let view = emptyInvocations();
  let context: InvocationsContext = {
    workspace: null,
    selectedId: null,
    activeId: null,
    canReadArtifacts: false,
  };
  let disposed = false;
  let listGeneration = 0;
  let listing = false;
  let listPending = false;
  let detailGeneration = 0;
  let detailRequestInFlight = false;
  let profileGeneration = 0;
  let profileRequestInFlight = false;

  function update(patch: Partial<InvocationsView>) {
    if (disposed) return;
    view = { ...view, ...patch };
    changed(view);
  }

  // Clearing a slice invalidates its in-flight loads, so a late response
  // cannot repopulate cleared data.
  function clearDetail() {
    ++detailGeneration;
    return { detail: null, detailLoading: false, detailError: null };
  }
  function clearProfile() {
    ++profileGeneration;
    return { profile: null, profileLoading: false, profileError: null };
  }
  function clearInvocations() {
    ++listGeneration;
    return { records: [], error: null };
  }
  function clearSelectedData() {
    return { ...clearDetail(), ...clearProfile() };
  }

  async function loadList() {
    if (!context.workspace || disposed) return;
    if (listing) {
      listPending = true;
      return;
    }
    listing = true;
    const generation = ++listGeneration;
    try {
      const records = await transport.invoke<InvocationRecord[]>(COMMANDS.listInvocations);
      acceptLoadedInvocations(generation, records);
    } catch (error) {
      if (generation === listGeneration) {
        update({ error: String(error) });
      }
    } finally {
      listing = false;
      if (listPending && !disposed) {
        listPending = false;
        void loadList();
      }
    }
  }

  // A changed selected record means its saved observation may have completed.
  function acceptLoadedInvocations(generation: number, records: InvocationRecord[]) {
    if (generation !== listGeneration || disposed) return;
    if (JSON.stringify(records) !== JSON.stringify(view.records) || view.error) {
      update({ records, error: null });
    }
    const selected = records.find((inv) => inv.id === context.selectedId);
    if (
      !selected ||
      !view.detail ||
      view.detailLoading ||
      view.detailError ||
      JSON.stringify(selected) === JSON.stringify(view.detail.record)
    ) {
      return;
    }
    loadSelected();
  }

  async function loadDetail() {
    const id = context.selectedId;
    // Saved observations are read after the active invocation ends.
    if (
      !id ||
      disposed ||
      !context.canReadArtifacts ||
      detailRequestInFlight ||
      id === context.activeId
    ) {
      return;
    }
    const generation = ++detailGeneration;
    detailRequestInFlight = true;
    update({ detailLoading: true, detailError: null });
    try {
      const detail = await transport.invoke<InvocationDetail>(COMMANDS.getInvocation, { id });
      if (generation === detailGeneration) {
        update({ detail, detailLoading: false });
      }
    } catch (error) {
      if (generation === detailGeneration) {
        update({ detailError: String(error), detailLoading: false });
      }
    } finally {
      detailRequestInFlight = false;
      if (generation !== detailGeneration) void loadDetail();
    }
  }

  async function loadProfile() {
    const id = context.selectedId;
    if (
      !id ||
      disposed ||
      !context.canReadArtifacts ||
      profileRequestInFlight ||
      id === context.activeId
    ) {
      return;
    }
    const generation = ++profileGeneration;
    profileRequestInFlight = true;
    update({ profileLoading: true, profileError: null });
    try {
      const profile = await transport.invoke<Section<Profile>>(COMMANDS.getInvocationProfile, {
        id,
      });
      if (generation === profileGeneration) {
        update({ profile, profileLoading: false });
      }
    } catch (error) {
      if (generation === profileGeneration) {
        update({ profileError: String(error), profileLoading: false });
      }
    } finally {
      profileRequestInFlight = false;
      if (generation !== profileGeneration) void loadProfile();
    }
  }

  function loadSelected() {
    void loadDetail();
    void loadProfile();
  }

  function sync(next: InvocationsContext): void {
    if (disposed) return;
    const workspaceChanged = next.workspace !== context.workspace;
    const selectedChanged = next.selectedId !== context.selectedId;
    context = next;
    if (workspaceChanged) {
      update({ ...clearInvocations(), ...clearSelectedData() });
    } else if (selectedChanged) {
      update(clearSelectedData());
    }
  }

  function clearSelected() {
    if (disposed) return;
    update(clearSelectedData());
  }

  function dispose() {
    disposed = true;
    ++listGeneration;
    ++detailGeneration;
    ++profileGeneration;
    listPending = false;
  }

  return {
    sync,
    clearSelected,
    loadList,
    loadDetail,
    loadProfile,
    loadSelected,
    dispose,
  };
}
