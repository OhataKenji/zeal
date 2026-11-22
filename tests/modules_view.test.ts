import assert from "node:assert/strict";
import { test } from "node:test";
import { invoke } from "@tauri-apps/api/core";
import { emptyModulesState, type ModulesState } from "../src/lib/modules.ts";
import type { AppState, ModuleExplanation, ModulesSnapshot } from "../src/lib/types.ts";
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
  const explanation = await invoke<ModuleExplanation>("explain_module", {
    workspace,
    moduleKey: module.key,
  });
  assert.ok(explanation.text.includes(module.key));
  assert.equal(explanation.warning, null);

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
        view: { ...emptyModulesState(), ...overrides },
        bazelAvailable,
        onload: () => {},
        onselect: () => {},
        onexplain: () => {},
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

  await t.test("partial results show the module table and explanation with their warnings", () => {
    const html = output({
      snapshot: { ...snapshot, warning: "Graph extension failed" },
      selectedKey: module.key,
      explanation: { text: "<root> -> dependency@1", warning: "Explain extension failed" },
    });
    const [main, aside] = html.split("</main>");
    assert.match(main, /role="status"[^>]*>[\s\S]*Bazel returned a partial module graph/);
    assert.match(main, /<details[^>]*>[\s\S]*Show Bazel output[\s\S]*Graph extension failed/);
    assert.match(main, /<th[^>]*>Module<\/th>/);
    assert.match(aside, /role="status"[^>]*>[\s\S]*Bazel returned a partial explanation/);
    assert.match(aside, /<details[^>]*>[\s\S]*Show Bazel output[\s\S]*Explain extension failed/);
    assert.match(aside, /<pre[^>]*>&lt;root> -> dependency@1<\/pre>/);
  });

  await t.test(
    "inspector links to modules but never to root, including a cycle back to root",
    () => {
      const cyclic = {
        ...snapshot,
        rootKey: "workspace@1",
        rootName: "my_project",
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
        explanation: { text: "<script>example</script>", warning: null },
      });
      const aside = html.match(/<\/main>\s*(<aside[\s\S]*?<\/aside>)/)?.[1];
      assert.ok(aside);
      assert.equal(aside.match(/<li[^>]*>\s*workspace@1 \(my_project\)\s*<\/li>/g)?.length, 2);
      assert.ok(aside.includes(`${module.dependencies[0]}</button>`));
      assert.match(aside, /<pre[^>]*>&lt;script/);
    },
  );

  await t.test("an unnamed root shows just its graph key", () => {
    const html = output({
      snapshot: { ...snapshot, rootName: "" },
      selectedKey: module.key,
    });
    const aside = html.match(/<\/main>\s*(<aside[\s\S]*?<\/aside>)/)?.[1];
    assert.ok(aside);
    assert.match(aside, /<li[^>]*>\s*&lt;root>\s*<\/li>/);
  });
});
