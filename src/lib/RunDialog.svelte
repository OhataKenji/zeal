<script lang="ts">
  import { onMount } from "svelte";
  import { targetError } from "./target";
  import type { BazelCommand } from "./types";

  let {
    cmd,
    pattern,
    flags,
    canRun,
    onrun,
    onclose,
  }: {
    cmd: BazelCommand;
    pattern: string;
    flags: string;
    canRun: (target: string) => boolean;
    onrun: (cmd: BazelCommand, pattern: string, flags: string) => void;
    onclose: () => void;
  } = $props();

  let dialog: HTMLDialogElement;
  const error = $derived(pattern === "" ? null : targetError(pattern));
  const preview = $derived(
    `bazel ${cmd} ${pattern.trim() || "//..."}${flags.trim() ? ` ${flags.trim()}` : ""}`,
  );
  const submittable = $derived(pattern.trim() !== "" && error === null && canRun(pattern));

  onMount(() => {
    dialog.showModal();
    return () => {
      if (dialog.open) dialog.close();
    };
  });

  function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!submittable) return;
    dialog.close();
    onrun(cmd, pattern.trim(), flags.trim());
  }
</script>

<dialog
  class="modal"
  bind:this={dialog}
  aria-label="Run Bazel"
  onclose={onclose}
  onclick={(e) => {
    if (e.target === dialog) dialog.close();
  }}
>
  <div class="modal-head">
    <span>Run Bazel</span>
    <button type="button" class="button button-ghost" onclick={() => dialog.close()}>Esc</button>
  </div>
  <form class="modal-body" onsubmit={submit}>
    <div class="run-row">
      <select class="select" aria-label="Bazel command" bind:value={cmd}>
        <option value="build">build</option>
        <option value="test">test</option>
        <option value="run">run</option>
      </select>
      <input
        class="input mono"
        type="text"
        aria-label="Target pattern"
        aria-invalid={!!error}
        placeholder="//..."
        spellcheck={false}
        autocomplete="off"
        bind:value={pattern}
      />
    </div>
    {#if error}
      <p class="field-error" role="status">{error}</p>
    {/if}
    <input
      class="input mono"
      type="text"
      aria-label="Arguments"
      placeholder="Flags, e.g. --config=opt --test_output=errors"
      spellcheck={false}
      autocomplete="off"
      bind:value={flags}
    />
    <pre class="code-box">{preview}</pre>
    <div class="modal-actions">
      <button type="button" class="button" onclick={() => dialog.close()}>Cancel</button>
      <button type="submit" class="button button-primary" disabled={!submittable}>Execute</button>
    </div>
  </form>
</dialog>
