<script lang="ts">
  import { onMount } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import { invoke } from "@tauri-apps/api/core";
  import "@xterm/xterm/css/xterm.css";
  import type { RunRecord } from "./types";
  import { createConsole } from "./console";
  import { terminalFont, terminalTheme } from "./terminal-style";
  import RunHeader from "./RunHeader.svelte";
  import { runStatus } from "./run";

  let { record, running }: {
    record: RunRecord;
    running: boolean;
  } = $props();
  let host: HTMLDivElement;
  let error = $state<string | null>(null);
  const status = $derived(runStatus(record, running ? record.id : null));

  onMount(() => {
    const term = new Terminal({
      ...terminalFont, theme: terminalTheme,
      convertEol: true, cursorBlink: false, drawBoldTextInBrightColors: false,
      scrollback: 5000, disableStdin: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    const connection = createConsole(record.id, { invoke },
      (bytes) => new Promise<void>((resolve) => term.write(bytes, resolve)),
      (message) => { error = message; });
    const observer = new ResizeObserver(() => { if (host.clientWidth && host.clientHeight) fit.fit(); });
    observer.observe(host);
    fit.fit();
    void connection.poll();
    const timer = setInterval(() => { void connection.poll(); }, 50);
    return () => {
      clearInterval(timer);
      observer.disconnect();
      connection.dispose();
      term.dispose();
    };
  });
</script>

<section class="run-page console-page">
  <RunHeader {record} {status} />
  {#if error}<p class="run-notice err" role="alert">{error}</p>{/if}
  <div class="run-panel">
    <div class="panel-heading console-log-spacer" aria-hidden="true">Logs</div>
    <div class="console-terminal" bind:this={host}></div>
  </div>
</section>

<style>
  .console-page { height: 100%; }
  .console-log-spacer { visibility: hidden; }
  .console-terminal { flex: 1; min-height: 0; overflow: hidden; }
</style>
