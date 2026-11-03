<script lang="ts">
  import type { Section } from "../types";
  import { sectionData, sectionMessage, sectionNoticeTone } from "../invocation";
  import Notice from "../Notice.svelte";
  import { logLines } from "../log";
  import { LOG_LINE_HEIGHT, TERMINAL_TEXT_STYLE } from "../terminal_style";

  let { section, loading }: {
    section: Section<string> | undefined;
    loading: boolean;
  } = $props();
  const log = $derived(sectionData(section));
  const lines = $derived(log === null ? [] : logLines(log));

  // Rows have a fixed height, so only the viewport plus an overscan skirt needs
  // to exist in the DOM. Spacers preserve the full scroll range, matching how
  // Perfetto's logs panel virtualizes long output.
  const OVERSCAN = 8;
  let scrollTop = $state(0);
  let viewHeight = $state(240);
  // Clamp first to the line count so a shrinking log cannot strand the viewport
  // behind an oversized top spacer.
  const first = $derived(
    Math.min(Math.max(0, Math.floor(scrollTop / LOG_LINE_HEIGHT) - OVERSCAN), lines.length),
  );
  const last = $derived(
    Math.min(
      lines.length,
      first + Math.ceil(viewHeight / LOG_LINE_HEIGHT) + 2 * OVERSCAN + 1,
    ),
  );
  const visible = $derived(lines.slice(first, last));
</script>

<h2 class="panel-heading">Logs</h2>
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
      <div style="height:{first * LOG_LINE_HEIGHT}px"></div>
      {#each visible as line}
        <div class="log-line" style="height:{LOG_LINE_HEIGHT}px"
          >{#each line as span}<span style={span.style}>{span.text}</span>{/each}</div
        >
      {/each}
      <div style="height:{(lines.length - last) * LOG_LINE_HEIGHT}px"></div>
    </div>
  </div>
{:else if loading}<p class="panel-empty">Loading invocation output…</p>{/if}
