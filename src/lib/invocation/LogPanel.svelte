<script lang="ts">
  import type { Section } from "../types";
  import { sectionData, sectionMessage, sectionNoticeTone } from "../invocation";
  import Notice from "../Notice.svelte";
  import { logColumns, logLines, logWindow } from "../log";
  import { LOG_LINE_HEIGHT, TERMINAL_TEXT_STYLE } from "../terminal_style";

  let { section, loading }: {
    section: Section<string> | undefined;
    loading: boolean;
  } = $props();
  const log = $derived(sectionData(section));
  const lines = $derived(log === null ? [] : logLines(log));
  const columns = $derived(logColumns(lines));

  // Rows have a fixed height, so only the viewport plus an overscan skirt needs
  // to exist in the DOM. Spacers preserve the full scroll range, matching how
  // Perfetto's logs panel virtualizes long output.
  let scrollTop = $state(0);
  let viewHeight = $state(240);
  const win = $derived(logWindow(lines.length, scrollTop, viewHeight));
  const visible = $derived(lines.slice(win.first, win.last));
</script>

{#if section && section.status !== "ok"}
  <Notice tone={sectionNoticeTone(section)}>{sectionMessage(section)}</Notice>
{/if}
{#if log !== null}
  <div class="log-panel">
    <div
      class="invocation-log"
      style={TERMINAL_TEXT_STYLE}
      aria-label="Invocation output"
      role="textbox"
      aria-readonly="true"
      aria-multiline="true"
      tabindex="0"
      bind:clientHeight={viewHeight}
      onscroll={(event) => (scrollTop = event.currentTarget.scrollTop)}
    >
      <div style="height:{win.first * LOG_LINE_HEIGHT}px;width:{columns}ch"></div>
      {#each visible as line}
        <div class="log-line" style="height:{LOG_LINE_HEIGHT}px"
          >{#each line as span}<span style={span.style}>{span.text}</span>{/each}</div
        >
      {/each}
      <div style="height:{(lines.length - win.last) * LOG_LINE_HEIGHT}px"></div>
    </div>
  </div>
{:else if loading}<p class="panel-empty">Loading invocation output…</p>{/if}
