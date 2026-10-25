// WebKit emits cumulative gesture scales; Chromium/Firefox emit Ctrl+wheel.
export function pinchZoom(node: EventTarget, zoom: (factor: number, clientX: number) => void) {
  let gestureScale: number | null = null;
  const handlers: Record<string, EventListener> = {
    wheel(event) {
      const wheel = event as WheelEvent;
      if (!wheel.ctrlKey) return;
      event.preventDefault();
      if (gestureScale !== null) return;
      const unit = wheel.deltaMode === 1 ? 16 : wheel.deltaMode === 2 ? 800 : 1;
      zoom(Math.exp(Math.max(-1, Math.min(1, (-wheel.deltaY * unit) / 100))), wheel.clientX);
    },
    gesturestart(event) {
      event.preventDefault();
      gestureScale = 1;
    },
    gesturechange(event) {
      event.preventDefault();
      const { scale, clientX } = event as Event & { scale: number; clientX: number };
      if (gestureScale === null || !Number.isFinite(scale) || scale <= 0) return;
      zoom(scale / gestureScale, clientX);
      gestureScale = scale;
    },
    gestureend(event) {
      event.preventDefault();
      gestureScale = null;
    },
  };
  for (const [name, handler] of Object.entries(handlers))
    node.addEventListener(name, handler, { passive: false });
  return {
    destroy() {
      for (const [name, handler] of Object.entries(handlers))
        node.removeEventListener(name, handler);
    },
  };
}
