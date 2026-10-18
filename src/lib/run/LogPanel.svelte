<script lang="ts">
  import type { Section } from "../types";
  import { sectionData, sectionMessage } from "../run";

  let { section, loading }: {
    section: Section<string> | undefined;
    loading: boolean;
  } = $props();
  const log = $derived(sectionData(section));
</script>

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
