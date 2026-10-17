<script lang="ts">
  import { tick } from "svelte";
  import type { SessionView } from "./session";
  import { outcome, sectionData, sectionMessage } from "./run";
  import ProfileView from "./ProfileView.svelte";

  let { view, reload }: { view: SessionView; reload: () => void } = $props();
  const record = $derived(view.detail?.record ?? view.runs.find((run) => run.id === view.selectedRun));
  const running = $derived(view.state?.running === view.selectedRun);
  const profile = $derived(sectionData(view.detail?.profile));
  const bep = $derived(sectionData(view.detail?.bep));
  const log = $derived(running ? view.liveLog : view.detail ? sectionData(view.detail.log) : view.liveLog || null);
  let logElement = $state<HTMLTextAreaElement>();
  let follow = $state(true);
  $effect(() => {
    void log;
    if (follow) void tick().then(() => { if (logElement) logElement.scrollTop = logElement.scrollHeight; });
  });
</script>

{#if record}
  <div class="run-head"><strong>bazel {record.cmd} {record.target}</strong><span class="badge">{outcome(record, view.state?.running ?? null)}</span></div>
  <p class="label">{new Date(record.startedAt).toLocaleString()}{#if record.finishedAt !== undefined} → {new Date(record.finishedAt).toLocaleTimeString()} · {((record.finishedAt - record.startedAt) / 1000).toFixed(3)}s (wall clock){/if}</p>
{:else}<p class="dim">run {view.selectedRun}</p>{/if}
{#if view.detailLoading}<p class="dim" role="status">記録を読み込み中…</p>{/if}
{#if view.detailError}<p class="err">詳細を読めません: {view.detailError}</p><button class="btn" onclick={reload}>retry</button>{/if}

<h2>timing</h2>
{#if running}<p class="dim">not available — 完了後に profile を表示します</p>
{:else if view.detail}
  <p class:warn={view.detail.profile.status !== "ok"} class:label={view.detail.profile.status === "ok"}>{sectionMessage(view.detail.profile)}</p>
  {#if profile}{#key view.detail}<ProfileView {profile} />{/key}{/if}
{/if}

<h2>log</h2>
{#if running && !view.liveFromStart}<p class="warn">接続後の出力を表示中です。完了後に保存済みの全 log を読み込みます。</p>{/if}
{#if !running && view.detail}<p class:warn={view.detail.log.status !== "ok"} class:label={view.detail.log.status === "ok"}>{sectionMessage(view.detail.log)}</p>{/if}
{#if log !== null}
  <label class="label"><input type="checkbox" bind:checked={follow} /> 新しい出力を追う</label>
  <textarea class="run-log" aria-label="Build stdout and stderr" readonly bind:this={logElement} value={log || (running ? "出力待ち" : "(empty log)")} onscroll={() => { if (logElement) follow = logElement.scrollHeight - logElement.scrollTop - logElement.clientHeight < 24; }}></textarea>
{/if}

<h2>BEP</h2>
{#if running}<p class="dim">not available — 完了後に記録を読み込みます</p>
{:else if view.detail}
  <p class:warn={view.detail.bep.status !== "ok"} class:label={view.detail.bep.status === "ok"}>{sectionMessage(view.detail.bep)}</p>
  {#if bep}
    {#if bep.targets.length === 0 && bep.tests.length === 0}<p class="dim">target / test の結果は記録されていません</p>{/if}
    <ul class="list">
      {#each bep.targets as target}<li><span>{target.label}</span><span>{target.ok ? "✓ target completed" : "✗ target failed"}</span></li>{/each}
      {#each bep.tests as test}<li><span>{test.label}</span><span>{test.status}</span></li>{/each}
    </ul>
  {/if}
{/if}
