<script lang="ts">
  import type { TargetAction, TargetActions } from "./types";
  import { commandLine } from "./model";

  let {
    actions,
    error,
  }: {
    actions: TargetActions | null;
    error: string | null;
  } = $props();

  let copyState = $state<{ index: number; status: "copied" | "failed" } | null>(null);

  $effect.pre(() => {
    void actions;
    copyState = null;
  });

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
                  <details>
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
                            {#each action.inputs as path}<li
                                ><code>{path}</code></li
                              >{/each}
                          </ul>
                        {:else}
                          <p class="dim">None</p>
                        {/if}
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
