<script lang="ts">
  import type { TargetAction, TargetActions } from "./types";
  import { commandLine } from "./model";
  import { tick } from "svelte";

  let {
    actions,
    error,
    oninputclick,
  }: {
    actions: TargetActions | null;
    error: string | null;
    oninputclick?: (path: string, owner: string) => void;
  } = $props();

  let copyState = $state<{ index: number; status: "copied" | "failed" } | null>(null);
  let open = $state<boolean[]>([]);
  let highlighted = $state<number | null>(null);
  const details: HTMLDetailsElement[] = [];

  $effect.pre(() => {
    void actions;
    copyState = null;
    open = [];
    highlighted = null;
  });

  async function trace(path: string, owner: string) {
    const producer = actions?.actions.findIndex((a) => a.outputs.includes(path)) ?? -1;
    if (producer < 0) {
      oninputclick?.(path, owner);
      return;
    }
    open[producer] = true;
    highlighted = producer;
    await tick();
    details[producer]?.scrollIntoView({ block: "nearest" });
  }

  async function copyCommand(action: TargetAction, index: number) {
    const current = actions;
    try {
      await navigator.clipboard.writeText(commandLine(action));
      if (actions === current) copyState = { index, status: "copied" };
    } catch {
      if (actions === current) copyState = { index, status: "failed" };
    }
  }
</script>

<section class="target-actions">
  <h2>
    Actions{#if actions}
      <span class="dim"
        >{actions.actions.length}{actions.truncated ? "+" : ""}</span
      >{/if}
  </h2>
  {#if error}
    <p class="dim">Could not load actions: {error}</p>
  {:else if actions === null}
    <p class="dim">Loading actions…</p>
  {:else if actions.actions.length === 0}
    <p class="dim">None</p>
  {:else}
    {#key actions}
      <div class="actions-scroll">
        <table>
          <thead
            ><tr
              ><th scope="col">Mnemonic</th><th scope="col">Primary output</th></tr
            ></thead
          >
          <tbody>
            {#each actions.actions as action, index}
              {@const extraOutputs =
                action.outputs.length -
                (action.outputs.includes(action.output) ? 1 : 0)}
              <tr>
                <td colspan="2" class="action-cell">
                  <details bind:this={details[index]} bind:open={open[index]} class:highlighted={highlighted === index}>
                    <summary class="action-summary">
                      <span class="action-mnemonic" title={action.mnemonic}
                        >{action.mnemonic}</span
                      >
                      <span class="action-output" title={action.output}
                        >{action.output}{#if extraOutputs > 0}<span
                            class="dim action-extra"
                            title="{extraOutputs} additional output{extraOutputs === 1
                              ? ''
                              : 's'}">+{extraOutputs}</span
                          >{/if}</span
                      >
                    </summary>
                    {#if open[index]}
                    <div class="action-details">
                      <section class="action-section action-command-section">
                        <h3>Command</h3>
                        {#if action.arguments.length > 0}
                          <div class="action-command-row">
                            <code class="action-command"
                              >{commandLine(action)}</code
                            >
                            <button
                              type="button"
                              class="button button-ghost action-copy"
                              onclick={() => copyCommand(action, index)}>Copy</button
                            >
                            {#if copyState && copyState.index === index}
                              {#if copyState.status === "copied"}<span
                                  class="dim"
                                  role="status">Copied</span
                                >{:else}<span class="dim" role="status"
                                  >Copy failed</span
                                >{/if}
                            {/if}
                          </div>
                        {:else}
                          <p class="dim">No command line provided</p>
                        {/if}
                      </section>
                      <section class="action-section">
                        <h3>Inputs</h3>
                        {#if action.inputs.length > 0}
                          <ul class="action-paths">
                            {#each action.inputs as path}
                              <li>
                                {#if oninputclick}
                                  <button type="button" class="button button-ghost" title="Trace producer" onclick={() => trace(path, action.owner)}><code>{path}</code></button>
                                {:else}
                                  <code>{path}</code>
                                {/if}
                              </li>
                            {/each}
                          </ul>
                        {:else if !action.inputsTruncated}
                          <p class="dim">None</p>
                        {/if}
                        {#if action.inputsTruncated}<p class="dim">Additional inputs not shown.</p>{/if}
                      </section>
                      <section class="action-section">
                        <h3>Outputs</h3>
                        {#if action.outputs.length > 0}
                          <ul class="action-paths">
                            {#each action.outputs as path}<li
                                ><code>{path}</code></li
                              >{/each}
                          </ul>
                        {:else}
                          <p class="dim">None</p>
                        {/if}
                      </section>
                      <section class="action-section">
                        <h3>Environment</h3>
                        {#if action.environmentVariables.length > 0}
                          <ul class="action-paths">
                            {#each action.environmentVariables as pair}<li
                                ><code>{pair.key}={pair.value}</code></li
                              >{/each}
                          </ul>
                        {:else}
                          <p class="dim">None</p>
                        {/if}
                      </section>
                      <section class="action-section">
                        <h3>Configuration</h3>
                        {#if action.configuration}
                          <dl class="action-config">
                            <dt>Mnemonic</dt>
                            <dd>{action.configuration.mnemonic}</dd>
                            <dt>Platform</dt>
                            <dd>{action.configuration.platformName}</dd>
                            <dt>Checksum</dt>
                            <dd><code>{action.configuration.checksum}</code></dd>
                            <dt>Tool</dt>
                            <dd>{action.configuration.isTool ? "Yes" : "No"}</dd>
                          </dl>
                        {:else}
                          <p class="dim">Not provided</p>
                        {/if}
                      </section>
                      <section class="action-section">
                        <h3>Execution platform</h3>
                        {#if action.executionPlatform}
                          <code class="action-platform"
                            >{action.executionPlatform}</code
                          >
                        {:else}
                          <p class="dim">Not provided</p>
                        {/if}
                      </section>
                      <section class="action-section">
                        <h3>Owner</h3>
                        <code>{action.owner}</code>
                      </section>
                    </div>
                    {/if}
                  </details>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/key}
    {#if actions.truncated}
      <p class="dim">
        Showing first {actions.actions.length} actions; more actions not shown.
      </p>
    {/if}
  {/if}
</section>
