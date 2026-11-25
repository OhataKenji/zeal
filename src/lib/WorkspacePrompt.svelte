<script lang="ts">
  import { untrack } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import BesSetup from "./BesSetup.svelte";
  import Notice from "./Notice.svelte";
  import type { TargetScope } from "./types";

  let {
    initial = "",
    initialScope,
    bazelAvailable,
    besEndpoint,
    onopen,
    onclose,
    opening = $bindable(false),
  }: {
    initial?: string;
    initialScope: TargetScope;
    bazelAvailable: boolean;
    besEndpoint: string | null;
    onopen: (path: string, scope: TargetScope) => Promise<boolean>;
    onclose?: () => void;
    opening?: boolean;
  } = $props();

  // Props seed this form once.
  let path = $state(untrack(() => initial));
  let include = $state(untrack(() => initialScope.include.join("\n")));
  type PromptAction =
    | { kind: "idle" }
    | { kind: "choosing" }
    | { kind: "submitting" }
    | { kind: "failed"; error: string };
  let action = $state<PromptAction>({ kind: "idle" });
  const busy = $derived(opening || action.kind === "choosing" || action.kind === "submitting");
  const canSubmit = $derived(
    bazelAvailable && !busy && path.trim() !== "" && include.trim() !== "",
  );
  const wholeWorkspace = $derived(include.split("\n").some((pattern) => pattern.trim() === "//..."));

  async function selectWorkspace() {
    if (busy) return;
    action = { kind: "choosing" };
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Select Bazel workspace",
        defaultPath: path.trim() || undefined,
      });
      if (selected !== null) path = selected;
      action = { kind: "idle" };
    } catch (e) {
      action = { kind: "failed", error: String(e) };
    }
  }

  async function submit() {
    if (!canSubmit) return;
    action = { kind: "submitting" };
    opening = true;
    try {
      if (await onopen(path.trim(), { include: include.split("\n") })) onclose?.();
      action = { kind: "idle" };
    } catch (e) {
      action = { kind: "failed", error: String(e) };
    } finally {
      opening = false;
    }
  }
</script>

{#if !bazelAvailable}
  <Notice tone="error"
    >Bazel or Bazelisk was not found on your PATH. Please install either tool
    and add it to your PATH, then restart Zelith.</Notice
  >
{/if}
<form
  class="ws-prompt"
  onsubmit={(e) => {
    e.preventDefault();
    void submit();
  }}
>
  <header>
    <h1>Open a workspace</h1>
    <p class="dim">
      Choose a Bazel repository to explore its targets and build history.
    </p>
  </header>
  <div class="workspace-folder">
    <label class="workspace-selection">
      <span>Workspace</span>
      <input
        class="input workspace-path"
        type="text"
        bind:value={path}
        placeholder="/path/to/workspace"
        spellcheck={false}
        disabled={busy}
        required
      />
    </label>
    <button class="button" type="button" onclick={selectWorkspace} disabled={busy}>
      Choose folder…
    </button>
  </div>
  <Notice tone="warning">Do not open an untrusted repository.</Notice>
  <label>
    Target patterns
    <textarea
      class="input scope-patterns"
      bind:value={include}
      placeholder={"//services/payments/...\n//libs/money/...\n//tools:generator"}
      rows="4"
      aria-describedby={wholeWorkspace ? "scope-help scope-warning" : "scope-help"}
      disabled={busy}
      required
    ></textarea>
  </label>
  <p class="scope-help" id="scope-help">
    One Bazel target pattern per line. Default: //... (all packages in the main
    repository). The target tree and search show matching rules.
  </p>
  {#if wholeWorkspace}
    <Notice tone="warning" id="scope-warning">
      //... may be slow in large workspaces.
      Consider using a narrower scope, like //app/....
    </Notice>
  {/if}
  <div class="ws-actions">
    <button
      class="button button-primary"
      type="submit"
      disabled={!canSubmit}>Open workspace</button
    >
    {#if onclose}
      <button class="button" type="button" onclick={onclose} disabled={busy}>Cancel</button>
    {/if}
  </div>
  {#if action.kind === "failed" && action.error}
    <Notice tone="error">{action.error}</Notice>
  {/if}
  <BesSetup endpoint={besEndpoint} />
</form>
