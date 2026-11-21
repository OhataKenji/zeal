/** Index of the tab a key moves to, or -1 for non-navigation keys. */
export function nextTabIndex(key: string, index: number, count: number): number {
  switch (key) {
    case "ArrowRight":
      return (index + 1) % count;
    case "ArrowLeft":
      return (index + count - 1) % count;
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return -1;
  }
}

/** Arrow/Home/End activation and focus movement shared by tablists. */
export function tabKeydown(
  event: KeyboardEvent,
  index: number,
  count: number,
  select: (next: number) => void,
) {
  const next = nextTabIndex(event.key, index, count);
  if (next < 0) return;
  event.preventDefault();
  select(next);
  const buttons =
    event.currentTarget instanceof HTMLElement
      ? event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role="tab"]')
      : null;
  buttons?.[next]?.focus();
}
