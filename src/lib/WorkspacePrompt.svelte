<script lang="ts">
  import { Input } from "$lib/components/ui/input/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { untrack } from "svelte";
  import { openWorkspace } from "./state.svelte";

  let { initial = "", onclose }: { initial?: string; onclose?: () => void } =
    $props();

  // The prop only seeds the input once.
  let path = $state(untrack(() => initial));
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function submit() {
    busy = true;
    error = null;
    try {
      await openWorkspace(path.trim());
      onclose?.();
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
    <Input
      class="font-mono"
      bind:value={path}
      placeholder="/path/to/bazel/workspace"
      disabled={busy}
    />
  </label>
  <Button
    type="submit"
    variant="default"
    size="sm"
    disabled={busy || path.trim() === ""}>open</Button
  >
  {#if onclose}
    <Button type="button" variant="outline" size="sm" onclick={onclose}
      >cancel</Button
    >
  {/if}
</form>
{#if error}<p class="err">{error}</p>{/if}
