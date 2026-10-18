<script lang="ts">
  import type { Section } from "../types";
  import { sectionData, sectionMessage } from "../run";

  let { section, running, loading }: {
    section: Section<string> | undefined;
    running: boolean;
    loading: boolean;
  } = $props();
  const log = $derived(sectionData(section));
</script>

{#if running}
  <p class="run-notice" role="status">実行中。ログは完了後に表示します。</p>
{:else}
  {#if section && section.status !== "ok"}
    <p class="run-notice">{sectionMessage(section)}</p>
  {/if}
  {#if log !== null}
    <div class="log-panel">
      <div class="log-toolbar"><strong>Build log</strong></div>
      <textarea class="run-log" aria-label="Build stdout and stderr" readonly wrap="off"
        value={log || "(empty log)"}></textarea>
    </div>
  {:else if loading}<p class="panel-empty">Loading build log…</p>{/if}
{/if}
