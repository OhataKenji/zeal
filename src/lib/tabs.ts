function nextIndex(key: string, index: number, count: number): number {
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

/** Arrow/Home/End on a tab: focus the target tab and activate it. */
export function tabKeydown(event: KeyboardEvent) {
  const current = event.currentTarget;
  if (!(current instanceof HTMLElement)) return;
  const list = current.closest('[role="tablist"]');
  if (!list) return;
  const tabs = Array.from(list.querySelectorAll<HTMLButtonElement>('button[role="tab"]'));
  const index = tabs.indexOf(current as HTMLButtonElement);
  if (index < 0) return;
  const next = nextIndex(event.key, index, tabs.length);
  if (next < 0) return;
  event.preventDefault();
  tabs[next].focus();
  tabs[next].click();
}
