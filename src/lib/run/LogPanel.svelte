<script lang="ts">
  import type { Section } from "../types";
  import { sectionData, sectionMessage } from "../run";
  import { logSpans } from "../log";
  import { TERMINAL_TEXT_STYLE } from "../terminal_style";

  let { section, loading }: {
    section: Section<string> | undefined;
    loading: boolean;
  } = $props();
  const log = $derived(sectionData(section));
  const spans = $derived(logSpans(log ?? ""));
</script>

<h2 class="panel-heading">Logs</h2>
{#if section && section.status !== "ok" && section.status !== "incomplete"}
  <p class="run-notice">{sectionMessage(section)}</p>
{/if}
{#if log !== null}
  <div class="log-panel">
    <div class="run-log" style={TERMINAL_TEXT_STYLE} aria-label="Build stdout and stderr" role="textbox" aria-readonly="true" aria-multiline="true" tabindex="0"
    >{#each spans as span}<span style={span.style}>{span.text}</span>{/each}</div>
  </div>
{:else if loading}<p class="panel-empty">Loading build log…</p>{/if}
