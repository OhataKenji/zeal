<script lang="ts">
  import SearchDropdown from "./SearchDropdown.svelte";
  import { keyAction, moveIndex, optionId, search, type SearchIndex } from "./search";

  /**
   * The fuzzy combobox used by the command composer and every query input that
   * has candidates. `value` is controlled: the component reports every change
   * through `oninput`. Typing is always free-form; the dropdown only assists.
   */
  let {
    index,
    value,
    oninput,
    ...rest
  }: {
    index: SearchIndex | null;
    value: string;
    oninput: (value: string) => void;
  } & Record<string, unknown> = $props();

  // Per-instance listbox id — several comboboxes can be open at once.
  const listId = $props.id();
  let open = $state(false);
  let sel = $state(0);
  const matches = $derived(index ? search(index, value) : []);
  const expanded = $derived(open && index !== null);
  // The list can shrink under the selection, so clamp: the highlight,
  // aria-activedescendant and Enter all name the same row.
  const activeIdx = $derived(moveIndex(sel, 0, matches.length));
  const activeOpt = $derived(
    expanded && matches.length > 0 ? optionId(listId, activeIdx) : undefined,
  );

  function type(e: Event & { currentTarget: HTMLInputElement }) {
    oninput(e.currentTarget.value);
    sel = 0;
    open = e.currentTarget.value !== "";
  }

  function pick(v: string) {
    oninput(v);
    open = false;
    sel = 0;
  }

  function keydown(e: KeyboardEvent) {
    if (e.isComposing) return;
    const action = keyAction({ open: expanded, index: activeIdx }, e.key, matches);
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
        // Keep Escape from also cancelling the enclosing native <dialog>.
        e.preventDefault();
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
    aria-expanded={expanded}
    aria-controls={expanded ? listId : undefined}
    aria-autocomplete="list"
    aria-activedescendant={activeOpt}
    {value}
    oninput={type}
    onkeydown={keydown}
    onblur={() => (open = false)}
  />
  {#if expanded}
    <SearchDropdown {matches} index={activeIdx} onpick={pick} {listId} />
  {/if}
</div>
