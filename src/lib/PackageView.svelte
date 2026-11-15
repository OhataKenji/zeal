<script lang="ts">
  import type { TargetIndex } from "./types";

  let { index, pkg, onselect }: {
    index: TargetIndex;
    pkg: string;
    onselect: (label: string) => void;
  } = $props();
  const targets = $derived(index.targets.filter((target) => target.pkg === pkg).sort((a, b) => a.name.localeCompare(b.name)));
</script>

<header class="target-head">
  <h1>//{pkg}</h1>
  <span class="badge">Package</span>
</header>
<h2 class="attributes-head">Contents</h2>
<dl class="relations">
  <dt>Targets <small>{targets.length}</small></dt>
  <dd>
    <div class="chips">
      {#each targets as target (target.label)}
        <button class="chip" title={target.label} onclick={() => onselect(target.label)}>
          <span>:{target.name}</span><small>{target.ruleClass}</small>
        </button>
      {:else}<span class="dim">No targets in this directory in the loaded scope.</span>{/each}
    </div>
  </dd>
</dl>
