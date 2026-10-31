import { expect, test, type Page } from "@playwright/test";

// macOS WebKit uses Option+Tab to include buttons without changing OS preferences.
const NEXT_CONTROL = process.platform === "darwin" ? "Alt+Tab" : "Tab";
const PREVIOUS_CONTROL = process.platform === "darwin" ? "Alt+Shift+Tab" : "Shift+Tab";

test.beforeEach(async ({ page, baseURL }) => {
  // The preview fakes only Tauri IPC. Render the actual App and block remote I/O.
  await page.route("**/*", (route) =>
    new URL(route.request().url()).origin === baseURL ? route.continue() : route.abort(),
  );
  await page.goto("/?preview");
  await expect(page.getByRole("button", { name: "Build", exact: true })).toBeEnabled();
});

test("opening Arguments focuses its editor and Escape returns to the toggle", async ({ page }) => {
  const toggle = page.getByRole("button", { name: /^Arguments/ });
  const editor = page.getByRole("textbox", { name: "Arguments", exact: true });

  await toggle.click();
  await expect(editor).toBeFocused();
  await editor.press("Escape");

  await expect(editor).toBeHidden();
  await expect(toggle).toBeFocused();
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
});

test("the argument draft survives closing and changing the command and target", async ({
  page,
}) => {
  const toggle = page.getByRole("button", { name: /^Arguments/ });
  const editor = page.getByRole("textbox", { name: "Arguments", exact: true });
  const draft = `--config=dev -- "two words" '$HOME'`;

  await toggle.click();
  await editor.fill(draft);
  await toggle.click();
  await expect(editor).toBeHidden();
  await expect(toggle).toHaveAccessibleName("Arguments (configured)");
  await page.getByRole("combobox", { name: "Bazel command" }).selectOption("run");
  await page.getByRole("combobox", { name: "Target label or pattern" }).fill("//app:server");
  await toggle.click();

  await expect(editor).toHaveValue(draft);
});

test("native keyboard navigation leaves the editor for Run without starting a command", async ({
  page,
}) => {
  await page.getByRole("combobox", { name: "Bazel command" }).selectOption("run");
  const run = page.getByRole("button", { name: "Run", exact: true });
  const editor = page.getByRole("textbox", { name: "Arguments", exact: true });

  await page.getByRole("button", { name: /^Arguments/ }).click();
  await expect(editor).toBeFocused();
  await editor.press(NEXT_CONTROL);

  await expect(run).toBeFocused();
  await expect(editor).toBeHidden();
  await expect(page.getByRole("button", { name: "Stop", exact: true })).toBeHidden();
});

test("backward keyboard navigation reaches the toggle and Escape still closes Arguments", async ({
  page,
}) => {
  const toggle = page.getByRole("button", { name: /^Arguments/ });
  const editor = page.getByRole("textbox", { name: "Arguments", exact: true });

  await toggle.click();
  await expect(editor).toBeFocused();
  await editor.press(PREVIOUS_CONTROL);
  await expect(toggle).toBeFocused();
  await expect(editor).toBeVisible();
  await toggle.press("Escape");

  await expect(editor).toBeHidden();
  await expect(toggle).toBeFocused();
});

test("Escape during composition leaves the argument editor open", async ({ page }) => {
  const editor = page.getByRole("textbox", { name: "Arguments", exact: true });
  await page.getByRole("button", { name: /^Arguments/ }).click();

  await editor.dispatchEvent("keydown", { key: "Escape", isComposing: true });

  await expect(editor).toBeFocused();
  await expect(editor).toBeVisible();
});

test("clicking page content dismisses Arguments without restoring toggle focus", async ({
  page,
}) => {
  const toggle = page.getByRole("button", { name: /^Arguments/ });
  const editor = page.getByRole("textbox", { name: "Arguments", exact: true });
  await toggle.click();
  await editor.fill("--config=dev");

  await page.getByRole("heading", { name: "//app/lib:parser", exact: true }).click();

  await expect(editor).toBeHidden();
  await expect(toggle).not.toBeFocused();
  await toggle.click();
  await expect(editor).toHaveValue("--config=dev");
});

test("an outside Run click starts one command and preserves the argument draft", async ({
  page,
}) => {
  const toggle = page.getByRole("button", { name: /^Arguments/ });
  const editor = page.getByRole("textbox", { name: "Arguments", exact: true });
  const runs = page.locator(".run-row");
  await expect(runs.first()).toBeVisible();
  const before = await runs.count();
  const draft = '--config=dev -- "two words"';
  await page.getByRole("combobox", { name: "Bazel command" }).selectOption("run");
  await toggle.click();
  await editor.fill(draft);

  await page.getByRole("button", { name: "Run", exact: true }).click();

  await expect(editor).toBeHidden();
  await expect(page.getByRole("button", { name: "Stop", exact: true })).toBeEnabled();
  await expect(runs).toHaveCount(before + 1);
  await toggle.click();
  await expect(editor).toHaveValue(draft);
});

test("Arguments and target suggestions dismiss each other without stealing target focus", async ({
  page,
}) => {
  const toggle = page.getByRole("button", { name: /^Arguments/ });
  const editor = page.getByRole("textbox", { name: "Arguments", exact: true });
  const target = page.getByRole("combobox", { name: "Target label or pattern" });

  await toggle.click();
  await expect(editor).toBeFocused();
  await target.click();
  await expect(editor).toBeHidden();
  await expect(target).toBeFocused();
  await target.fill("parser app");
  await expect(page.getByRole("listbox")).toBeVisible();
  await expect(page.getByRole("listbox").getByRole("option").first()).toBeVisible();
  await toggle.click();

  await expect(page.getByRole("listbox")).toBeHidden();
  await expect(editor).toBeFocused();
  await expect(target).toHaveValue("parser app");
});

test("Enter confirms the fuzzy candidate without starting a run", async ({ page }) => {
  const target = page.getByRole("combobox", { name: "Target label or pattern" });
  await page.getByRole("combobox", { name: "Bazel command" }).selectOption("run");
  await target.fill("parser app");
  await expect(page.getByRole("listbox").getByRole("option").first()).toContainText(
    "//app/lib:parser",
  );

  await target.press("Enter");

  await expect(target).toHaveValue("//app/lib:parser");
  await expect(page.getByRole("listbox")).toBeHidden();
  await expect(page.getByRole("button", { name: "Run", exact: true })).toBeEnabled();
  await expect(page.getByRole("button", { name: "Stop", exact: true })).toBeHidden();
});

test("native keyboard navigation leaves fuzzy search without selecting a candidate", async ({
  page,
}) => {
  const target = page.getByRole("combobox", { name: "Target label or pattern" });
  await target.fill("parser app");
  await expect(page.getByRole("listbox")).toBeVisible();

  await target.press(NEXT_CONTROL);

  await expect(page.getByRole("button", { name: /^Arguments/ })).toBeFocused();
  await expect(target).toHaveValue("parser app");
  await expect(page.getByRole("listbox")).toBeHidden();
  await expect(page.getByRole("textbox", { name: "Arguments", exact: true })).toBeHidden();
});

test("Enter during composition does not select a fuzzy candidate or start a run", async ({
  page,
}) => {
  const target = page.getByRole("combobox", { name: "Target label or pattern" });
  await page.getByRole("combobox", { name: "Bazel command" }).selectOption("run");
  await target.fill("parser app");
  await expect(page.getByRole("listbox")).toBeVisible();

  await target.dispatchEvent("keydown", { key: "Enter", isComposing: true });

  await expect(target).toHaveValue("parser app");
  await expect(page.getByRole("listbox")).toBeVisible();
  await expect(page.getByRole("button", { name: "Run", exact: true })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Stop", exact: true })).toBeHidden();
});

async function assertPopoverLayout(page: Page, width: number) {
  await page.setViewportSize({ width, height: 780 });
  const toolbar = page.locator(".toolbar");
  const body = page.locator(".win-body");
  const toolbarBefore = await toolbar.boundingBox();
  const bodyBefore = await body.boundingBox();
  expect(toolbarBefore).not.toBeNull();
  expect(bodyBefore).not.toBeNull();
  await page.getByRole("button", { name: /^Arguments/ }).click();
  await expect(page.getByRole("textbox", { name: "Arguments", exact: true })).toBeFocused();
  const target = await page
    .getByRole("combobox", { name: "Target label or pattern" })
    .boundingBox();
  const popover = await page.locator("#run-arguments-popover").boundingBox();
  const editor = await page.getByRole("textbox", { name: "Arguments", exact: true }).boundingBox();

  expect(await toolbar.boundingBox()).toEqual(toolbarBefore);
  expect(await body.boundingBox()).toEqual(bodyBefore);
  expect(target).not.toBeNull();
  expect(popover).not.toBeNull();
  expect(editor).not.toBeNull();
  expect(popover!.x).toBeCloseTo(target!.x, 1);
  expect(popover!.width).toBeCloseTo(target!.width, 1);
  expect(popover!.width).toBeGreaterThan(0);
  expect(popover!.x).toBeGreaterThanOrEqual(0);
  expect(popover!.x + popover!.width).toBeLessThanOrEqual(width);
  expect(popover!.y).toBeGreaterThanOrEqual(target!.y + target!.height);
  expect(editor!.width).toBeLessThanOrEqual(target!.width);
  expect(target!.width - editor!.width).toBeLessThanOrEqual(24);
}

test("Arguments matches the target width without moving the page on desktop", async ({ page }) => {
  await assertPopoverLayout(page, 1220);
});

test("Arguments matches the target width and fits a narrow viewport", async ({ page }) => {
  await assertPopoverLayout(page, 360);
});
