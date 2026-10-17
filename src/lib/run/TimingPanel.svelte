<script lang="ts">
  import type { RunDetail } from "../types";
  import { sectionData, sectionMessage } from "../run";
  import ProfileView from "../ProfileView.svelte";

  let { detail, running, loading }: { detail: RunDetail | null; running: boolean; loading: boolean } = $props();
  const profile = $derived(sectionData(detail?.profile));
</script>

{#if running}<p class="panel-empty">Timing is available after the run finishes.</p>
{:else if detail}
  {#if detail.profile.status !== "ok"}<p class="run-notice">{sectionMessage(detail.profile)}</p>{/if}
  {#if profile}{#key detail}<ProfileView {profile} />{/key}{/if}
{:else if loading}<p class="panel-empty">Loading profile…</p>{/if}
