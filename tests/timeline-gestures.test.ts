import { test } from "node:test";
import assert from "node:assert/strict";
import { pinchZoom } from "../src/lib/timeline-gestures.ts";
import { zoomRange } from "../src/lib/timeline.ts";

function dispatch(node: EventTarget, type: string, fields = {}) {
  const event = Object.assign(new Event(type, { cancelable: true }), fields);
  node.dispatchEvent(event);
  return event;
}

test("trackpad wheel pinch zooms about its pointer and prevents page zoom", () => {
  const node = new EventTarget();
  const domain = { start: 0, end: 1000 };
  let range = domain;
  pinchZoom(node, (factor, clientX) => {
    range = zoomRange(range, domain, factor, clientX / 1000);
  });
  const event = dispatch(node, "wheel", {
    ctrlKey: true,
    deltaY: -Math.log(2) * 100,
    deltaMode: 0,
    clientX: 250,
  });
  assert.equal(event.defaultPrevented, true);
  assert.deepEqual(range, { start: 125, end: 625 });
  assert.equal(range.start + (range.end - range.start) * 0.25, 250);
});

test("ordinary two-finger scrolling remains available without zooming", () => {
  const node = new EventTarget();
  const zooms: number[] = [];
  pinchZoom(node, (factor) => zooms.push(factor));
  const event = dispatch(node, "wheel", { ctrlKey: false, deltaY: 80, deltaMode: 0 });
  assert.equal(event.defaultPrevented, false);
  assert.deepEqual(zooms, []);
});

test("WebKit cumulative pinch scales can enlarge and return to the full range", () => {
  const node = new EventTarget();
  const domain = { start: 0, end: 1000 };
  let range = domain;
  pinchZoom(node, (factor) => {
    range = zoomRange(range, domain, factor);
  });
  assert.equal(dispatch(node, "gesturestart").defaultPrevented, true);
  dispatch(node, "gesturechange", { scale: 2, clientX: 500 });
  assert.deepEqual(range, { start: 250, end: 750 });
  dispatch(node, "gesturechange", { scale: 4, clientX: 500 });
  assert.deepEqual(range, { start: 375, end: 625 });
  dispatch(node, "gesturechange", { scale: 1, clientX: 500 });
  assert.deepEqual(range, domain);
  assert.equal(dispatch(node, "gestureend").defaultPrevented, true);
});

test("wheel events do not apply the same pinch twice during a WebKit gesture", () => {
  const node = new EventTarget();
  const zooms: number[] = [];
  pinchZoom(node, (factor) => zooms.push(factor));
  dispatch(node, "gesturestart");
  dispatch(node, "gesturechange", { scale: 2, clientX: 500 });
  const wheel = { ctrlKey: true, deltaY: -Math.log(2) * 100, deltaMode: 0, clientX: 500 };
  assert.equal(dispatch(node, "wheel", wheel).defaultPrevented, true);
  assert.deepEqual(zooms, [2]);
  dispatch(node, "gestureend");
  dispatch(node, "wheel", wheel);
  assert.deepEqual(zooms, [2, 2]);
});

test("invalid gesture scales do not corrupt subsequent zooms", () => {
  const node = new EventTarget();
  const zooms: number[] = [];
  pinchZoom(node, (factor) => zooms.push(factor));
  dispatch(node, "gesturestart");
  dispatch(node, "gesturechange", { scale: 0, clientX: 500 });
  dispatch(node, "gesturechange", { scale: NaN, clientX: 500 });
  dispatch(node, "gesturechange", { scale: 2, clientX: 500 });
  assert.deepEqual(zooms, [2]);
});

test("leaving Timing removes its gesture handlers", () => {
  const node = new EventTarget();
  const zooms: number[] = [];
  const action = pinchZoom(node, (factor) => zooms.push(factor));
  action.destroy();
  assert.equal(dispatch(node, "gesturestart").defaultPrevented, false);
  assert.equal(dispatch(node, "gesturechange", { scale: 2 }).defaultPrevented, false);
  assert.equal(dispatch(node, "wheel", { ctrlKey: true, deltaY: -10 }).defaultPrevented, false);
  assert.deepEqual(zooms, []);
});
