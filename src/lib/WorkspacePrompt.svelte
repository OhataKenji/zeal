<script lang="ts">
  import { untrack } from "svelte";
  import { openWorkspace } from "./state.svelte";

  let { initial = "", onclose }: { initial?: string; onclose?: () => void } =
    $props();

  // The prop only seeds the input once.
  let path = $state(untrack(() => initial));
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function submit() {
    const close = onclose;
    busy = true;
    error = null;
    try {
      await openWorkspace(path.trim());
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
    workspace
    <input
      class="input workspace-path"
      bind:value={path}
      placeholder="/path/to/bazel/workspace"
      disabled={busy}
    />
  </label>
  <button
    class="button button-primary"
    type="submit"
    disabled={busy || path.trim() === ""}>open</button
  >
  {#if onclose}
    <button class="button" type="button" onclick={onclose}>cancel</button>
  {/if}
</form>
{#if error}<p class="error">{error}</p>{/if}
