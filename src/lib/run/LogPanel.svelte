<script lang="ts">
  import { tick } from "svelte";
  import type { Section } from "../types";
  import { sectionData, sectionMessage } from "../run";
  import { logSpans } from "../log";
  import { terminalTextStyle } from "../terminal-style";

  let { section, loading }: {
    section: Section<string> | undefined;
    loading: boolean;
  } = $props();
  const log = $derived(sectionData(section));
  const spans = $derived(logSpans(log ?? ""));
  let output = $state<HTMLDivElement>();
  let follow = true;

  $effect(() => {
    void log;
    if (follow) void tick().then(() => { if (output) output.scrollTop = output.scrollHeight; });
  });

</script>

<h2 class="panel-heading">Logs</h2>
{#if section && section.status !== "ok" && section.status !== "incomplete"}
  <p class="run-notice">{sectionMessage(section)}</p>
{/if}
{#if log !== null}
  <div class="log-panel">
    <div class="run-log" style={terminalTextStyle} aria-label="Build stdout and stderr" role="textbox" aria-readonly="true" aria-multiline="true" tabindex="0"
      bind:this={output}
      onscroll={() => { if (output) follow = output.scrollHeight - output.scrollTop - output.clientHeight < 32; }}
    >{#each spans as span}<span style={span.style}>{span.text}</span>{/each}</div>
  </div>
{:else if loading}<p class="panel-empty">Loading build log…</p>{/if}
