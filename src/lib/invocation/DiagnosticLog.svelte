<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { COMMANDS } from "../commands";
  import type { ProcessDiagnostic, Section } from "../types";
  import LogPanel from "./LogPanel.svelte";
  import Notice from "../Notice.svelte";

  let { diagnostic }: { diagnostic: ProcessDiagnostic } = $props();

  // One bounded read per mounted diagnostic; the parent keys this component
  // by invocation so a late or rejected read can never land on another row.
  let section = $state<Section<string>>();
  let error = $state<string | null>(null);

  onMount(() => {
    let cancelled = false;
    invoke<{ data: number[] }>(COMMANDS.readConsole, {
      workspace: diagnostic.workspace,
      id: diagnostic.id,
      offset: 0,
    })
      .then((result) => {
        if (cancelled) return;
        section = {
          status: "ok",
          data: new TextDecoder().decode(Uint8Array.from(result.data)),
        };
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        error = String(cause);
      });
    return () => {
      cancelled = true;
    };
  });
</script>

{#if error !== null}
  <Notice tone="error">Could not load invocation output: {error}</Notice>
{/if}
<LogPanel {section} loading={error === null && section === undefined} />
