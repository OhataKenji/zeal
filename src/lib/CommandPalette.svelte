<script lang="ts">
  import { createIndex, moveIndex, optionId } from "./search";
  import {
    paletteItems,
    type PaletteAction,
    type PaletteGroup,
    type PaletteInput,
    type PaletteItem,
  } from "./palette";
  import type { TargetIndex } from "./types";

  let {
    open = $bindable(false),
    snapshot,
    invocations,
    canRun,
    canRefresh,
    isMac,
    onselect,
  }: {
    open: boolean;
    snapshot: TargetIndex | null;
    invocations: PaletteInput["invocations"];
    canRun: boolean;
    canRefresh: boolean;
    isMac: boolean;
    onselect: (action: PaletteAction) => void;
  } = $props();

  const listId = $props.id();
  const GROUP_LABELS: Record<PaletteGroup, string> = {
    commands: "Commands",
    targets: "Targets",
    invocations: "Invocations",
  };

  let dialog = $state<HTMLDialogElement>();
  let input = $state<HTMLInputElement>();
  let query = $state("");
  let activeIndex = $state(0);
  // A stationary cursor must not fight keyboard scrolling: pointermove only
  // claims the highlight when the pointer actually moved.
  let pointerX = -1;
  let pointerY = -1;

  const targetIndex = $derived(snapshot === null ? null : createIndex(snapshot.targets));
  const items = $derived(
    open ? paletteItems({ query, targetIndex, invocations, canRun, canRefresh, isMac }) : [],
  );
  const activeIdx = $derived(moveIndex(activeIndex, 0, items.length));
  // Groups are contiguous in `items`; each option keeps its flat index so
  // aria-activedescendant and the highlight name the same row.
  const groups = $derived.by(() => {
    const result: Array<{
      group: PaletteGroup;
      items: Array<{ item: PaletteItem; index: number }>;
    }> = [];
    items.forEach((item, index) => {
      const last = result.at(-1);
      if (last !== undefined && last.group === item.group) {
        last.items.push({ item, index });
      } else {
        result.push({ group: item.group, items: [{ item, index }] });
      }
    });
    return result;
  });

  $effect(() => {
    if (!dialog) return;
    if (open && !dialog.open) {
      query = "";
      activeIndex = 0;
      dialog.showModal();
      input?.focus();
    } else if (!open && dialog.open) {
      dialog.close();
    }
  });

  $effect(() => {
    if (!open || items.length === 0) return;
    document.getElementById(optionId(listId, activeIdx))?.scrollIntoView({ block: "nearest" });
  });

  function choose(item: PaletteItem) {
    open = false;
    onselect(item.action);
  }

  function chooseActive() {
    const item = items[activeIdx];
    if (item !== undefined) choose(item);
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.isComposing) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      activeIndex = moveIndex(activeIdx, event.key === "ArrowDown" ? 1 : -1, items.length);
    } else if (event.key === "Enter" && !event.repeat) {
      event.preventDefault();
      chooseActive();
    }
  }

  function onOptionPointerMove(event: PointerEvent, index: number) {
    if (event.clientX === pointerX && event.clientY === pointerY) return;
    pointerX = event.clientX;
    pointerY = event.clientY;
    activeIndex = index;
  }
</script>

<dialog
  class="modal palette-modal"
  bind:this={dialog}
  aria-label="Command Palette"
  onclose={() => {
    // A queued close event can land after a rapid reopen; only a dialog that
    // is actually closed may push `open` back down.
    if (!dialog?.open) open = false;
  }}
  onkeydown={(event) => {
    // The native close event is queued; settling `open` here keeps a fast
    // ⌘K from toggling against a stale value.
    if (event.key === "Escape" && !event.isComposing) open = false;
  }}
  onclick={(event) => {
    if (event.target === dialog) open = false;
  }}
>
  <input
    class="palette-input"
    bind:this={input}
    type="text"
    aria-label="Search targets, invocations, commands"
    maxlength={512}
    placeholder="Search targets, invocations, commands…"
    autocomplete="off"
    spellcheck="false"
    role="combobox"
    aria-expanded="true"
    aria-controls={listId}
    aria-autocomplete="list"
    aria-activedescendant={items.length > 0 ? optionId(listId, activeIdx) : undefined}
    bind:value={query}
    oninput={() => (activeIndex = 0)}
    onkeydown={onKeydown}
  />
  <div class="palette-results" id={listId} role="listbox" aria-label="Palette results">
    {#each groups as group (group.group)}
      <div class="palette-group" role="group" aria-labelledby="{listId}-label-{group.group}">
        <div class="palette-group-label" id="{listId}-label-{group.group}">
          {GROUP_LABELS[group.group]}
        </div>
        {#each group.items as entry (entry.item.id)}
          <button
            type="button"
            class="palette-item"
            id={optionId(listId, entry.index)}
            role="option"
            aria-selected={entry.index === activeIdx}
            tabindex={-1}
            onpointermove={(event) => onOptionPointerMove(event, entry.index)}
            onmousedown={(event) => event.preventDefault()}
            onclick={() => choose(entry.item)}
          >
            <span class="palette-item-label">{entry.item.label}</span>
            {#if entry.item.detail !== undefined}
              <span class="palette-item-detail">{entry.item.detail}</span>
            {/if}
            {#if entry.item.shortcut !== undefined}
              <kbd class="palette-item-shortcut">{entry.item.shortcut}</kbd>
            {/if}
          </button>
        {/each}
      </div>
    {:else}
      <div class="palette-empty dim">No matches</div>
    {/each}
  </div>
</dialog>
