<script lang="ts">
  import type { SnapshotKey } from "fuzzysort";
  import SearchDropdown from "./SearchDropdown.svelte";
  import { keyAction, moveIndex, optionId, search } from "./search";
  import type { Target } from "./types";

  /**
   * The fuzzy combobox used by the command composer and every query input that
   * has candidates. `value` is controlled: the component reports every change
   * through `oninput`, and additionally fires `onpick` when a candidate is
   * chosen (Enter or click) so callers can react beyond filling the field.
   * Typing is always free-form; the dropdown only assists.
   */
  let {
    index = null,
    value,
    oninput,
    onpick,
    ...rest
  }: {
    index: SnapshotKey<Target> | null;
    value: string;
    oninput?: (value: string) => void;
    onpick?: (value: string) => void;
  } & Record<string, unknown> = $props();

  // Per-instance listbox id — several comboboxes can be open at once.
  const listId = $props.id();
  let open = $state(false);
  let sel = $state(0);
  const matches = $derived(index ? search(index, value) : []);
  // The list can shrink under the selection, so clamp: the highlight,
  // aria-activedescendant and Enter all name the same row.
  const activeIdx = $derived(moveIndex(sel, 0, matches.length));
  const activeOpt = $derived(
    open && matches.length > 0 ? optionId(listId, activeIdx) : undefined,
  );

  function type(e: Event & { currentTarget: HTMLInputElement }) {
    oninput?.(e.currentTarget.value);
    sel = 0;
    open = e.currentTarget.value !== "";
  }

  function pick(v: string) {
    oninput?.(v);
    onpick?.(v);
    open = false;
    sel = 0;
  }

  function keydown(e: KeyboardEvent) {
    if (e.isComposing) return;
    const action = keyAction({ open, index: activeIdx }, e.key, matches);
    switch (action.kind) {
      case "move":
        e.preventDefault();
        open = true;
        sel = action.index;
        break;
      case "pick":
        e.preventDefault();
        pick(action.value);
        break;
      case "close":
        open = false;
        break;
      case "none":
        break;
    }
  }
</script>

<div class="search-input">
  <input
    {...rest}
    type="text"
    autocomplete="off"
    spellcheck="false"
    role="combobox"
    aria-expanded={open && !!index}
    aria-controls={open && index ? listId : undefined}
    aria-autocomplete="list"
    aria-activedescendant={activeOpt}
    {value}
    oninput={type}
    onkeydown={keydown}
    onblur={() => (open = false)}
  />
  {#if open && index}
    <SearchDropdown {matches} index={activeIdx} onpick={pick} {listId} />
  {/if}
</div>
