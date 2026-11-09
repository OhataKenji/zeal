<script lang="ts">
  import { untrack } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { app, getSession, openWorkspace } from "./state.svelte";
  import CopyButton from "./CopyButton.svelte";
  import Notice from "./Notice.svelte";
  import type { TargetScope } from "./types";

  let {
    initial = "",
    initialScope,
    bazelAvailable,
    onclose,
  }: {
    initial?: string;
    initialScope: TargetScope;
    bazelAvailable: boolean;
    onclose?: () => void;
  } = $props();

  // Props seed this form once.
  let path = $state(untrack(() => initial));
  let include = $state(untrack(() => initialScope.include.join("\n")));
  let error = $state<string | null>(null);
  let choosing = $state(false);
  let copyError = $state("");
  const busy = $derived(choosing || app.view.opening);
  const canSubmit = $derived(
    bazelAvailable && !busy && path.trim() !== "" && include.trim() !== "",
  );
  const wholeWorkspace = $derived(include.split("\n").some((pattern) => pattern.trim() === "//..."));

  async function selectWorkspace() {
    if (busy) return;
    choosing = true;
    error = null;
    getSession().setWorkspacePromptOpen(true);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Select Bazel workspace",
        defaultPath: path || undefined,
      });
      if (selected !== null) path = selected;
    } catch (e) {
      error = String(e);
    } finally {
      choosing = false;
    }
  }

  function submit() {
    if (!canSubmit) return;
    error = null;
    openWorkspace(path.trim(), {
      include: include.split("\n"),
    });
  }

  const BAZELRC = `build --bes_backend=grpc://127.0.0.1:4718
build --experimental_build_event_upload_strategy=local
build --bes_timeout=5s
build --experimental_collect_system_network_usage`;
</script>

{#if !bazelAvailable}
  <Notice tone="error"
    >Bazel or Bazelisk was not found on your PATH. Please install either tool
    and add it to your PATH, then restart Zeal.</Notice
  >
{/if}
<form
  class="ws-prompt"
  onsubmit={(e) => {
    e.preventDefault();
    submit();
  }}
>
  <div class="workspace-folder">
    <div class="workspace-selection">
      <span>Workspace</span>
      <span class="workspace-path">{path || "No folder selected"}</span>
    </div>
    <button class="button" type="button" onclick={selectWorkspace} disabled={busy}>
      Choose folder…
    </button>
  </div>
  <Notice tone="warning">Do not open an untrusted repository.</Notice>
  <label>
    target patterns
    <textarea
      class="input scope-patterns"
      bind:value={include}
      placeholder={"//services/payments/...\n//libs/money/...\n//tools:generator"}
      rows="4"
      aria-describedby={wholeWorkspace ? "scope-help scope-warning" : "scope-help"}
      disabled={busy}
      required
    ></textarea>
  </label>
  <p class="scope-help" id="scope-help">
    One Bazel target pattern per line. Default: //... (all packages in the main
    repository). The target tree and search show matching rules.
  </p>
  {#if wholeWorkspace}
    <Notice tone="warning" id="scope-warning">
      //... may be slow in large workspaces.
      Consider using a narrower scope, like //app/....
    </Notice>
  {/if}
  <div class="ws-actions">
    <button
      class="button button-primary"
      type="submit"
      disabled={!canSubmit}>open</button
    >
    {#if onclose}
      <button class="button" type="button" onclick={onclose} disabled={busy}>cancel</button>
    {/if}
  </div>
  {#if error ?? app.view.workspaceError}
    <Notice tone="error">{error ?? app.view.workspaceError}</Notice>
  {/if}
  <div class="scope-help">
    <p class="scope-help">
      To also record CLI invocations, add these lines to your workspace's
      <code>.bazelrc</code> and keep Zeal open while running Bazel:
    </p>
    <div class="query-command">
      <pre class="query-command-pre"><code>{BAZELRC}</code></pre>
      <CopyButton
        text={BAZELRC}
        label="Copy .bazelrc settings"
        onerror={(message) => (copyError = message)}
      />
    </div>
    {#if copyError}<p role="status">{copyError}</p>{/if}
  </div>
</form>
