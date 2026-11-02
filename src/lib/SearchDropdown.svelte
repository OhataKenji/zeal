<script lang="ts">
  import { optionId, segments, type Match } from "./search";

  let {
    matches,
    index,
    onpick,
    listId,
  }: {
    matches: Match[];
    index: number;
    onpick: (value: string) => void;
    listId: string;
  } = $props();

  // The list is taller than the window shows, so the row Enter would pick has
  // to be brought into view whenever the selection moves.
  let rows: HTMLButtonElement[] = $state([]);

  $effect(() => {
    rows[index]?.scrollIntoView({ block: "nearest" });
  });
</script>

<div class="search-dropdown" id={listId} role="listbox">
  {#if matches.length === 0}
    <div class="match dim">No matches</div>
  {:else}
    {#each matches as match, i (match.target.label)}
      <button
        bind:this={rows[i]}
        type="button"
        class="match"
        class:selected={i === index}
        id={optionId(listId, i)}
        role="option"
        aria-selected={i === index}
        tabindex={-1}
        onmousedown={(event) => {
          event.preventDefault();
          onpick(match.target.label);
        }}
      >
        <span
          >{#each segments(match.target.label, match.indices) as part, j (j)}<span
              class:hit={part.hit}>{part.text}</span
            >{/each}</span
        >
        <span class="label">{match.target.ruleClass}</span>
      </button>
    {/each}
  {/if}
</div>
