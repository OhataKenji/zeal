<script lang="ts">
  import { untrack } from "svelte";
  import { openWorkspace } from "./state.svelte";
  import type { TargetScope } from "./types";

  let {
    initial = "",
    initialScope = { include: ["//..."], exclude: [] },
    onclose,
  }: {
    initial?: string;
    initialScope?: TargetScope;
    onclose?: () => void;
  } = $props();

  // Props seed this form once. A different repository starts with the default scope.
  let path = $state(untrack(() => initial));
  let include = $state(untrack(() => (initialScope.include.length ? initialScope.include : ["//..."]).join("\n")));
  let exclude = $state(untrack(() => initialScope.exclude.join("\n")));
  let error = $state<string | null>(null);
  let busy = $state(false);

  function patterns(value: string): string[] {
    return [...new Set(value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean))];
  }

  const scope = $derived({ include: patterns(include), exclude: patterns(exclude) });

  function changePath(event: Event & { currentTarget: HTMLInputElement }) {
    if (event.currentTarget.value.trim() !== initial.trim()) {
      include = "//...";
      exclude = "";
    }
  }

  async function submit() {
    if (busy || !path.trim() || scope.include.length === 0) return;
    const close = onclose;
    busy = true;
    error = null;
    try {
      await openWorkspace(path.trim(), scope);
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
      oninput={changePath}
      placeholder="/path/to/bazel/workspace"
      disabled={busy}
    />
  </label>
  <label>
    include target patterns
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
  <label>
    exclude target patterns (optional)
    <textarea
      class="input scope-patterns"
      bind:value={exclude}
      placeholder="//services/payments/legacy/..."
      rows="3"
      aria-describedby="scope-help"
      disabled={busy}
    ></textarea>
  </label>
  <p class="scope-help" id="scope-help">
    One Bazel target pattern per line. Exclusions take precedence. This scope controls target
    discovery; dependencies may point outside it. Default: //... (whole repository). Narrow it to limit discovery.
  </p>
  <div class="ws-actions">
    <button
      class="button button-primary"
      type="submit"
      disabled={busy || path.trim() === "" || scope.include.length === 0}>open</button
    >
    {#if onclose}
      <button class="button" type="button" onclick={onclose} disabled={busy}>cancel</button>
    {/if}
  </div>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</form>
