<script lang="ts">
  import type { TargetAction, TargetActions } from "./types";
  import { commandLine, shortOutputPath } from "./model";
  import { tick } from "svelte";
  import CopyButton from "./CopyButton.svelte";

  let {
    actions,
    error,
    oninputclick,
    canTraceInput,
    showHeading = true,
    showOwner = false,
  }: {
    actions: TargetActions | null;
    error: string | null;
    oninputclick?: (path: string, owner: string) => void;
    canTraceInput?: (path: string, owner: string) => boolean;
    showHeading?: boolean;
    showOwner?: boolean;
  } = $props();

  const produced = $derived(
    new Set(actions?.actions.flatMap((action) => action.outputs) ?? []),
  );

  let open = $state<boolean[]>([]);
  let highlighted = $state<number | null>(null);
  const details: HTMLDetailsElement[] = [];

  $effect.pre(() => {
    void actions;
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

  function actionMeta(action: TargetAction): { text: string; title: string } | null {
    const parts: string[] = [];
    // QueryView lists actions from many owners, so the owner stays visible
    // there; in the target view the owner is already the selected target.
    if (showOwner) parts.push(action.owner);
    const mnemonic = action.configuration?.mnemonic;
    if (mnemonic) parts.push(mnemonic);
    const platform = action.executionPlatform || action.configuration?.platformName;
    if (platform) parts.push(platform);
    if (parts.length === 0) return null;
    const text = parts.join(" · ");
    const checksum = action.configuration?.checksum;
    return { text, title: checksum ? `${text} · ${checksum}` : text };
  }
</script>

{#snippet inputList(action: TargetAction)}
  <ul class="action-paths">
    {#each action.inputs as path}
      {@const inTarget = produced.has(path)}
      {@const traceable =
        inTarget ||
        Boolean(oninputclick && (canTraceInput ? canTraceInput(path, action.owner) : showOwner))}
      <li>
        {#if traceable}
          <button
            type="button"
            class="link action-input"
            title={`${inTarget ? "Jump to producing action" : "Open upstream target"} · ${path}`}
            onclick={() => trace(path, action.owner)}
            ><code>{shortOutputPath(path)}</code></button
          >
        {:else}
          <code title={path}>{shortOutputPath(path)}</code>
        {/if}
      </li>
    {/each}
  </ul>
{/snippet}

<section class="target-actions">
  {#if showHeading}
    <h2>
      Actions{#if actions}
        <span class="dim"
          >{actions.actions.length}{actions.truncated ? "+" : ""}</span
        >{/if}
    </h2>
  {/if}
  {#if error}
    <p class="dim">Could not load actions: {error}</p>
  {:else if actions === null}
    <p class="dim">Loading actions…</p>
  {:else if actions.actions.length === 0}
    <p class="dim">None</p>
  {:else}
    {#key actions}
      <div class="action-list">
        {#each actions.actions as action, index}
          {@const extraOutputs =
            action.outputs.length -
            (action.outputs.includes(action.output) ? 1 : 0)}
          {@const meta = actionMeta(action)}
          <details
            bind:this={details[index]}
            bind:open={open[index]}
            class="action-item"
            class:highlighted={highlighted === index}
          >
            <summary class="action-summary">
              <span class="action-mnemonic" title={action.mnemonic}
                >{action.mnemonic}</span
              >
              <span class="action-output" title={action.output}
                >{shortOutputPath(action.output)}</span
              >
              {#if extraOutputs > 0}
                <span
                  class="action-extra dim"
                  title="{extraOutputs} additional output{extraOutputs === 1
                    ? ''
                    : 's'}">+{extraOutputs}</span
                >
              {/if}
              {#if action.configuration?.isTool}
                <span class="badge action-exec" title="Tool configuration">exec</span>
              {/if}
            </summary>
            {#if open[index]}
              {@const command = commandLine(action)}
              <div class="action-details">
                <section class="action-section action-command-section">
                  <div class="action-section-head">
                    <h3>Command</h3>
                    {#if action.arguments.length > 0}
                      <CopyButton
                        text={command}
                        label={`Copy ${action.mnemonic} command`}
                      />
                    {/if}
                  </div>
                  {#if action.arguments.length > 0}
                    <pre class="action-command">{command}</pre>
                  {:else}
                    <p class="dim">No command line provided</p>
                  {/if}
                </section>
                <section class="action-section">
                  <h3>Outputs ({action.outputs.length})</h3>
                  {#if action.outputs.length > 0}
                    <ul class="action-paths">
                      {#each action.outputs as path}<li
                          ><code title={path}>{shortOutputPath(path)}</code></li
                        >{/each}
                    </ul>
                  {:else}
                    <p class="dim">None</p>
                  {/if}
                </section>
                <section class="action-section">
                  {#if action.inputs.length > 5}
                    <details class="action-inputs">
                      <summary class="action-inputs-summary"
                        >Inputs ({action.inputs.length}{action.inputsTruncated
                          ? "+"
                          : ""})</summary
                      >
                      {@render inputList(action)}
                    </details>
                  {:else}
                    <h3>
                      Inputs ({action.inputs.length}{action.inputsTruncated ? "+" : ""})
                    </h3>
                    {#if action.inputs.length > 0}
                      {@render inputList(action)}
                    {:else if !action.inputsTruncated}
                      <p class="dim">None</p>
                    {/if}
                  {/if}
                  {#if action.inputsTruncated}
                    <p class="dim">Additional inputs not shown.</p>
                  {/if}
                </section>
                {#if action.environmentVariables.length > 0}
                  <section class="action-section">
                    <h3>Env ({action.environmentVariables.length})</h3>
                    <ul class="action-paths">
                      {#each action.environmentVariables as pair}<li
                          ><code title="{pair.key}={pair.value}"
                            >{pair.key}={pair.value}</code
                          ></li
                        >{/each}
                    </ul>
                  </section>
                {/if}
                {#if meta}
                  <p class="action-meta mono dim" title={meta.title}>{meta.text}</p>
                {/if}
              </div>
            {/if}
          </details>
        {/each}
      </div>
    {/key}
    {#if actions.truncated}
      <p class="dim">
        Showing first {actions.actions.length} actions; more actions not shown.
      </p>
    {/if}
  {/if}
</section>
