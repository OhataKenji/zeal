<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import AppView from "./AppView.svelte";
  import { createSession, emptyView } from "./lib/session";

  const requestedPage = typeof location === "undefined"
    ? null
    : new URLSearchParams(location.search).get("view");
  const initialPage = requestedPage === "analytics" || requestedPage === "query"
    ? requestedPage
    : "targets";
  let view = $state.raw(emptyView(initialPage));
  const session = createSession(
    {
      invoke,
      listen: <T>(event: string, callback: (payload: T) => void) =>
        listen<T>(event, (e) => callback(e.payload)),
    },
    (next) => {
      view = next;
    },
    initialPage,
  );
  onMount(() => {
    void session.init();
    return () => session.dispose();
  });
</script>

<AppView {view} {session} />
