<script lang="ts">
  import Notice from "./Notice.svelte";
  import { onMount } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import { invoke } from "@tauri-apps/api/core";
  import "@xterm/xterm/css/xterm.css";
  import type { InvocationRecord, OwnedInvocation } from "./types";
  import { createConsole } from "./console";
  import { TERMINAL_FONT, TERMINAL_TEXT_STYLE, TERMINAL_THEME } from "./terminal_style";
  import InvocationHeader from "./InvocationHeader.svelte";
  import { invocationStatus } from "./invocation";

  let { record, execution, onstop }: {
    record: InvocationRecord;
    execution: OwnedInvocation;
    onstop: (id: string) => void;
  } = $props();
  let host: HTMLDivElement;
  let terminal = $state.raw<Terminal>();
  let connection = $state.raw<ReturnType<typeof createConsole>>();
  let error = $state<string | null>(null);
  let pausedOutput = $state<string | null>(null);
  const writable = $derived(execution.stoppable && !execution.stopping);
  const status = $derived(invocationStatus(record, execution));

  $effect(() => {
    if (terminal) terminal.options.disableStdin = !writable;
    connection?.setWritable(writable);
  });

  onMount(() => {
    const term = new Terminal({
      ...TERMINAL_FONT, theme: TERMINAL_THEME,
      convertEol: false, cursorBlink: true, drawBoldTextInBrightColors: false,
      scrollback: 5000, disableStdin: !writable,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    terminal = term;
    function showPausedOutput() {
      // WebKit can pause xterm's paint loop while the buffer keeps filling.
      const buffer = term.buffer.active;
      const lines = [];
      for (let row = 0; row < term.rows; row++) {
        lines.push(buffer.getLine(buffer.viewportY + row)?.translateToString(true) ?? "");
      }
      pausedOutput = lines.join("\n");
    }
    const stream = createConsole(execution.workspace, execution.id, { invoke },
      (bytes) => new Promise<void>((resolve) => {
        term.write(bytes, () => {
          if (document.hidden) showPausedOutput();
          resolve();
        });
      }),
      (message) => { error = message; });
    stream.setWritable(writable);
    connection = stream;
    const keys = term.onData((data) => {
      if (writable) stream.input(new TextEncoder().encode(data));
    });
    const binary = term.onBinary((data) => {
      if (writable) stream.input(Uint8Array.from(data, (ch) => ch.charCodeAt(0)));
    });
    const sizes = term.onResize(({ cols, rows }) => { void stream.resize(cols, rows); });
    function fitTerminal() {
      if (host.clientWidth && host.clientHeight) fit.fit();
    }
    function updateVisibility() {
      if (document.hidden) {
        showPausedOutput();
        return;
      }
      fitTerminal();
      term.refresh(0, term.rows - 1);
      void stream.poll();
    }
    const rendered = term.onRender(() => {
      if (!document.hidden) pausedOutput = null;
    });
    document.addEventListener("visibilitychange", updateVisibility);
    const observer = new ResizeObserver(fitTerminal);
    observer.observe(host);
    fitTerminal();
    void stream.resize(term.cols, term.rows);
    void stream.poll();
    const timer = setInterval(() => { void stream.poll(); }, 50);
    term.focus();
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", updateVisibility);
      observer.disconnect();
      stream.dispose();
      keys.dispose(); binary.dispose(); sizes.dispose();
      rendered.dispose();
      term.dispose();
    };
  });
</script>

<section class="invocation-page console-page">
  <InvocationHeader {record} {status} {execution} {onstop} />
  {#if error}<Notice tone="error">{error}</Notice>{/if}
  <div class="invocation-panel">
    <div class="console-terminal">
      <div class="console-host" bind:this={host}></div>
      {#if pausedOutput !== null}
        <pre class="console-paused" style={TERMINAL_TEXT_STYLE} aria-hidden="true">{pausedOutput}</pre>
      {/if}
    </div>
  </div>
</section>

<style>

  .console-page {
    height: 100%;
  }

  .console-terminal {
    flex: 1;
    min-height: 0;
    overflow: hidden;
    position: relative;
  }

  .console-host {
    height: 100%;
  }

  .console-paused {
    position: absolute;
    inset: 0;
    margin: 0;
    overflow: hidden;
    pointer-events: none;
  }
</style>
