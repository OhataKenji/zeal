<script lang="ts">
  import { LIST_ID, PATTERN_HINTS, optionId, segments, type Match } from "./search";

  let {
    matches,
    index,
    onpick,
  }: {
    matches: Match[];
    index: number;
    onpick: (value: string) => void;
  } = $props();

  // The list is taller than the window shows, so the row Enter would pick has
  // to be brought into view whenever the selection moves.
  let rows: HTMLButtonElement[] = $state([]);

  $effect(() => {
    rows[index]?.scrollIntoView({ block: "nearest" });
  });
</script>

<div class="qo" id={LIST_ID} role="listbox">
  {#if matches.length === 0}
    <div class="match dim">一致する target はありません</div>
  {:else}
    {#each matches as match, i (`${match.candidate.kind}:${match.candidate.key}`)}
      <button
        bind:this={rows[i]}
        type="button"
        class="match"
        class:sel={i === index}
        id={optionId(i)}
        role="option"
        aria-selected={i === index}
        tabindex={-1}
        onmousedown={(event) => {
          event.preventDefault();
          onpick(match.candidate.insert);
        }}
      >
        <span
          >{#each segments(match.candidate.text, match.indices) as part, j (j)}<span
              class:hit={part.hit}>{part.text}</span
            >{/each}</span
        >
        <span class="label">{match.candidate.note}</span>
      </button>
    {/each}
  {/if}
  <div class="foot">
    {#each PATTERN_HINTS as hint (hint)}<code>{hint}</code>{/each}
  </div>
</div>
