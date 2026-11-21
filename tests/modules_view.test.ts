import assert from "node:assert/strict";
import { test } from "node:test";
import { invoke } from "@tauri-apps/api/core";
import { emptyModulesState, type ModulesState } from "../src/lib/modules.ts";
import type { AppState, ModulesSnapshot } from "../src/lib/types.ts";
import { createSvelteHarness } from "./svelte.ts";

test("module page and browser preview", async (t) => {
  const { load, render } = await createSvelteHarness(t);
  const { default: ModulesView } = await load("/src/lib/ModulesView.svelte");
  const { installPreview } = await load("/src/preview.ts");
  Object.assign(globalThis, { window: {}, location: new URL("http://localhost/") });
  t.after(() => {
    Reflect.deleteProperty(globalThis, "window");
    Reflect.deleteProperty(globalThis, "location");
  });
  installPreview();
  const workspace = (await invoke<AppState>("get_state")).lastWorkspace;
  const snapshot = await invoke<ModulesSnapshot>("get_modules", { workspace });
  const module = snapshot.modules.find((node) => node.direct && node.dependencies.length);
  assert.ok(module);
  const explanation = await invoke<string>("explain_module", { workspace, moduleKey: module.key });
  assert.ok(explanation.includes(module.key));

  await t.test(
    "preview rejects unopened workspaces, malformed keys and unknown modules",
    async () => {
      for (const command of ["get_modules", "explain_module"]) {
        await assert.rejects(
          invoke(command, { workspace: "/unknown", moduleKey: module.key }),
          /workspace/i,
        );
      }
      for (const moduleKey of ["--flag", "x".repeat(257), "x\n", ""]) {
        await assert.rejects(invoke("explain_module", { workspace, moduleKey }), /invalid/i);
      }
      await assert.rejects(
        invoke("explain_module", { workspace, moduleKey: "unknown@1" }),
        /unknown/i,
      );
    },
  );

  function output(overrides: Partial<ModulesState> = {}, bazelAvailable = true): string {
    return render(ModulesView, {
      props: {
        workspace,
        bazelAvailable,
        initialState: { ...emptyModulesState(), ...overrides },
      },
    }).body.replace(/<!--[\s\S]*?-->/g, "");
  }

  await t.test("unloaded page offers explicit load, unavailable and retry states", () => {
    assert.match(output(), />\s*Load modules\s*<\/button>/);
    assert.doesNotMatch(output(), /<aside/);
    assert.match(output({}, false), /<button[^>]*disabled[^>]*>\s*Load modules/);
    const error = output({ loadError: "graph failed" });
    assert.match(error, /role="alert"[^>]*>[\s\S]*graph failed[\s\S]*Retry/);
  });

  await t.test("partial results keep the module table visible with a warning", () => {
    const html = output({ snapshot: { ...snapshot, warning: "Bazel extension failed" } });
    assert.match(html, /role="status"[^>]*>[\s\S]*Bazel returned a partial module graph/);
    assert.match(html, /<details[^>]*>[\s\S]*Show Bazel output[\s\S]*Bazel extension failed/);
    assert.match(html, /<th[^>]*>Module<\/th>/);
  });

  await t.test(
    "inspector links to modules but never to root, including a cycle back to root",
    () => {
      const cyclic = {
        ...snapshot,
        rootKey: "workspace@1",
        modules: snapshot.modules.map((node) =>
          node.key === module.key
            ? {
                ...node,
                requiredBy: ["workspace@1"],
                dependencies: [...node.dependencies, "workspace@1"],
              }
            : node,
        ),
      };
      const html = output({
        snapshot: cyclic,
        selectedKey: module.key,
        explanation: "<script>example</script>",
      });
      const aside = html.match(/<\/main>\s*(<aside[\s\S]*?<\/aside>)/)?.[1];
      assert.ok(aside);
      assert.equal(aside.match(/workspace@1/g)?.length, 2);
      assert.doesNotMatch(aside, /<button[^>]*>\s*workspace@1/);
      assert.ok(aside.includes(`${module.dependencies[0]}</button>`));
      assert.match(aside, /<pre[^>]*>&lt;script/);
    },
  );
});
