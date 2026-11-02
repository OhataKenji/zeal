<script lang="ts">
  import { untrack } from "svelte";
  import { openWorkspace } from "./state.svelte";
  import Notice from "./Notice.svelte";
  import type { TargetScope } from "./types";

  let {
    initial = "",
    initialScope,
    onclose,
  }: {
    initial?: string;
    initialScope: TargetScope;
    onclose?: () => void;
  } = $props();

  // Props seed this form once.
  let path = $state(untrack(() => initial));
  let include = $state(untrack(() => initialScope.include.join("\n")));
  let error = $state<string | null>(null);
  let busy = $state(false);
  const canSubmit = $derived(!busy && path.trim() !== "" && include.trim() !== "");

  async function submit() {
    if (!canSubmit) return;
    const close = onclose;
    busy = true;
    error = null;
    try {
      await openWorkspace(path.trim(), {
        include: include.split("\n"),
      });
      close?.();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }
</script>

<form
  class="ws-prompt"
  onsubmit={(e) => {
    e.preventDefault();
    submit();
  }}
>
  <label>
    repository
    <input
      class="input workspace-path"
      bind:value={path}
      placeholder="/path/to/bazel/workspace"
      disabled={busy}
    />
  </label>
  <Notice tone="warning">Do not open an untrusted repository.</Notice>
  <label>
    target patterns
    <textarea
      class="input scope-patterns"
      bind:value={include}
      placeholder={"//services/payments/...\n//libs/money/...\n//tools:generator"}
      rows="4"
      aria-describedby="scope-help"
      disabled={busy}
      required
    ></textarea>
  </label>
  <p class="scope-help" id="scope-help">
    One Bazel target pattern per line. Default: //... (whole
    repository). Affects the target tree and fuzzy target search.
  </p>
  <div class="ws-actions">
    <button
      class="button button-primary"
      type="submit"
      disabled={!canSubmit}>open</button
    >
    {#if onclose}
      <button class="button" type="button" onclick={onclose} disabled={busy}>cancel</button>
    {/if}
  </div>
  {#if error}<Notice tone="error">{error}</Notice>{/if}
</form>
