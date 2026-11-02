<script lang="ts">
  import Notice from "./Notice.svelte";
  import { onMount } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import { invoke } from "@tauri-apps/api/core";
  import "@xterm/xterm/css/xterm.css";
  import type { InvocationRecord } from "./types";
  import { createConsole } from "./console";
  import { TERMINAL_FONT, TERMINAL_THEME } from "./terminal_style";
  import InvocationHeader from "./InvocationHeader.svelte";
  import { invocationStatus } from "./invocation";

  let { record, running, stopping = false, visible }: {
    record: InvocationRecord;
    running: boolean;
    stopping?: boolean;
    visible: boolean;
  } = $props();
  let host: HTMLDivElement;
  let terminal = $state.raw<Terminal>();
  let connection = $state.raw<ReturnType<typeof createConsole>>();
  let error = $state<string | null>(null);
  const status = $derived(invocationStatus(record, running ? record.id : null, stopping));

  $effect(() => {
    if (terminal) terminal.options.disableStdin = !running;
    connection?.setRunning(running);
  });

  $effect(() => { if (visible && terminal) terminal.focus(); });

  onMount(() => {
    const term = new Terminal({
      ...TERMINAL_FONT, theme: TERMINAL_THEME,
      convertEol: false, cursorBlink: true, drawBoldTextInBrightColors: false,
      scrollback: 5000, disableStdin: !running,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    terminal = term;
    const stream = createConsole(record.id, { invoke },
      (bytes) => new Promise<void>((resolve) => term.write(bytes, resolve)),
      (message) => { error = message; });
    stream.setRunning(running);
    connection = stream;
    const keys = term.onData((data) => {
      if (running) stream.input(new TextEncoder().encode(data));
    });
    const binary = term.onBinary((data) => {
      if (running) stream.input(Uint8Array.from(data, (ch) => ch.charCodeAt(0)));
    });
    const sizes = term.onResize(({ cols, rows }) => { void stream.resize(cols, rows); });
    const observer = new ResizeObserver(() => { if (host.clientWidth && host.clientHeight) fit.fit(); });
    observer.observe(host);
    fit.fit();
    void stream.resize(term.cols, term.rows);
    void stream.poll();
    const timer = setInterval(() => { void stream.poll(); }, 50);
    term.focus();
    return () => {
      clearInterval(timer);
      observer.disconnect();
      stream.dispose();
      keys.dispose(); binary.dispose(); sizes.dispose();
      term.dispose();
    };
  });
</script>

<section class="invocation-page console-page">
  <InvocationHeader {record} {status} />
  {#if error}<Notice tone="error">{error}</Notice>{/if}
  <div class="invocation-panel">
    <div class="panel-heading console-log-spacer" aria-hidden="true">Logs</div>
    <div class="console-terminal" bind:this={host}></div>
  </div>
</section>

<style>

  .console-page {
    height: 100%;
  }

  .console-log-spacer {
    visibility: hidden;
  }

  .console-terminal {
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
</style>
