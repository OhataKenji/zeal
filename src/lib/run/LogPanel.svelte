<script lang="ts">
  import { tick } from "svelte";
  import type { Section } from "../types";
  import { sectionData, sectionMessage } from "../run";

  let { section, liveLog, liveFromStart, running, loading, follow = $bindable() }: {
    section: Section<string> | undefined;
    liveLog: string;
    liveFromStart: boolean;
    running: boolean;
    loading: boolean;
    follow: boolean;
  } = $props();
  const log = $derived(running ? liveLog : section ? sectionData(section) : liveLog || null);
  let logElement = $state<HTMLTextAreaElement>();

  $effect(() => {
    void log;
    if (follow) void tick().then(() => {
      if (logElement) logElement.scrollTop = logElement.scrollHeight;
    });
  });
</script>

{#if running && !liveFromStart}<p class="run-notice">接続後の出力を表示中。完了後に保存済みの全 log を読み込みます。</p>{/if}
{#if !running && section && section.status !== "ok"}
  <p class="run-notice">{sectionMessage(section)}</p>
{/if}
{#if log !== null}
  <div class="log-panel">
    <div class="log-toolbar">
      <strong>Build log</strong>
      {#if running}<span class="log-live">● Live</span>{/if}
      <label><input type="checkbox" bind:checked={follow} /> Follow output</label>
    </div>
    <textarea class="run-log" aria-label="Build stdout and stderr" readonly wrap="off"
      bind:this={logElement} value={log || (running ? "出力待ち" : "(empty log)")}
      onscroll={() => {
        if (logElement) follow = logElement.scrollHeight - logElement.scrollTop - logElement.clientHeight < 24;
      }}></textarea>
  </div>
{:else if loading}<p class="panel-empty">Loading build log…</p>{/if}
