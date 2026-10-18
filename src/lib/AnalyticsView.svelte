<script lang="ts">
  import RunStatus from "./RunStatus.svelte";
  import { invoke } from "@tauri-apps/api/core";
  import {
    createAnalytics,
    emptyAnalytics,
    recordedOutcome,
    type AnalyticsFilter,
  } from "./analytics";
  import { app, session } from "./state.svelte";
  import RunView from "./RunView.svelte";
  import type { RunRecord } from "./types";

  let { workspace, runs }: { workspace: string; runs: RunRecord[] } = $props();
  let view = $state.raw(emptyAnalytics());
  let controller = $state<ReturnType<typeof createAnalytics> | null>(null);
  $effect(() => {
    // A workspace change discards both the filters and pending responses.
    void workspace;
    const next = createAnalytics({ invoke }, (value) => (view = value));
    controller = next;
    return next.dispose;
  });
  $effect(() => {
    void runs;
    if (controller) void controller.load();
  });
  const data = $derived(view.data);
  const max = $derived(Math.max(1, ...(data?.days.map((d) => d.count) ?? [])));
  const width = $derived(Math.max(640, (data?.days.length ?? 0) * 24 + 60));
  function filter(patch: Partial<AnalyticsFilter>) {
    session.selectRun(null);
    void controller?.filter({ ...view.filter, ...patch });
  }
</script>

<section class="analytics" class:detail={!!app.view.selectedRun}>
  <h1>Local Run Analytics <span class="badge">日次の実行件数</span></h1>
  {#if app.view.selectedRun}
    <button class="btn back" onclick={() => session.selectRun(null)}
      >← 日次の履歴へ戻る</button
    >
    <RunView view={app.view} reload={session.loadDetail} />
  {:else}
    <div class="filters">
      <label
        >開始日 <input
          type="date"
          aria-label="開始日"
          value={view.filter.start ?? ""}
          onchange={(e) => filter({ start: e.currentTarget.value })}
        /></label
      >
      <label
        >終了日 <input
          type="date"
          aria-label="終了日"
          value={view.filter.end ?? ""}
          onchange={(e) => filter({ end: e.currentTarget.value })}
        /></label
      >
      <label
        >cmd <select
          aria-label="集計するcmd"
          value={view.filter.cmd ?? ""}
          onchange={(e) =>
            filter({
              cmd: (e.currentTarget.value as AnalyticsFilter["cmd"]) || null,
            })}
        >
          <option value="">すべて</option><option>build</option><option
            >test</option
          >
        </select></label
      >
      <label
        >結果 <select
          aria-label="集計する結果"
          value={view.filter.outcome ?? ""}
          onchange={(e) =>
            filter({
              outcome:
                (e.currentTarget.value as AnalyticsFilter["outcome"]) || null,
            })}
        >
          <option value="">すべて</option><option>ok</option><option
            >failed</option
          ><option>interrupted</option>
        </select></label
      >
      <button class="btn" onclick={() => controller?.load()}>再集計</button>
    </div>
    <details class="analytics-notes">
      <summary>集計について</summary>
      <p>
        期間は1〜366日。OSのローカル日付で表示します。タイムゾーン変更後はworkspaceを開き直してください。
      </p>
      <p>
        終了コードを保存したGUI実行だけを数えます。全Bazel起動回数ではありません。interruptedはexit
        8です。
      </p>
    </details>
    {#if view.loading}<p role="status">集計中…</p>{/if}
    {#if view.error}<p class="err" role="alert">
        集計できません: {view.error}
      </p>{/if}
    {#if data}
      <section class="analytics-card">
        <div class="chart-heading">
          <h2 class="eyebrow">Daily recorded runs</h2>
          <strong>Zealで記録した実行 {data.total}件</strong>
        </div>
        <p class="dim">{data.filter.start} 〜 {data.filter.end}</p>
        <div class="chart-scroll">
          <svg
            viewBox="0 0 {width} 240"
            style:width="{width}px"
            role="img"
            aria-label="日付別の実行件数。各日を選ぶには下の日付一覧を使います。"
          >
            <title>日次の実行件数</title>
            <line x1="50" y1="20" x2="50" y2="200" />
            <line x1="50" y1="200" x2={width - 10} y2="200" />
            <text x="4" y="16">件数</text><text x="12" y="34">{max}</text><text
              x="30"
              y="200">0</text
            >
            <text x={width - 45} y="236">日付</text>
            {#each data.days as day, index}
              {@const step = (width - 70) / data.days.length}
              <rect
                rx="5"
                x={55 + index * step}
                y={200 - (day.count / max) * 160}
                width={Math.max(1, step - 3)}
                height={(day.count / max) * 160}
              >
                <title
                  >{day.day}: {day.count}件{day.day === data.today
                    ? "（集計途中）"
                    : ""}</title
                >
              </rect>
              {#if index === 0 || index === data.days.length - 1}
                <text
                  x={55 + index * step}
                  y="218"
                  text-anchor={index === 0 ? "start" : "end"}>{day.day}</text
                >
              {/if}
            {/each}
          </svg>
        </div>
      </section>
      <section class="analytics-card">
        <h2 class="eyebrow">Browse by day</h2>
        <div class="day-list" aria-label="日付を選択">
          {#each data.days as day}
            <button
              class="btn"
              class:chosen={view.filter.day === day.day}
              aria-pressed={view.filter.day === day.day}
              onclick={() => controller?.selectDay(day.day)}
            >
              {day.day} · {day.count}件{day.day === data.today
                ? "（集計途中）"
                : ""}
            </button>
          {/each}
        </div>
        {#if data.filter.day}
          <h2>{data.filter.day} の履歴 · {data.runs.length}件</h2>
          <button class="btn" onclick={() => controller?.selectDay(null)}
            >日付の選択を解除</button
          >
          {#if data.runs.length === 0}<p>この条件の記録はありません。</p>{/if}
          <ul class="list">
            {#each data.runs as run (run.id)}
              {@const result = recordedOutcome(run)}
              <li>
                <button
                  class="run-row"
                  title={result}
                  aria-label={`${run.cmd} ${run.target}: ${result}`}
                  onclick={() => session.selectRun(run.id)}
                >
                  <span class="run-label"
                    ><RunStatus
                      kind={result === "ok"
                        ? "succeeded"
                        : result === "unknown"
                          ? "unknown"
                          : "failed"}
                      label={result}
                      compact
                    />{run.cmd}
                    {run.target}</span
                  ><small>{run.id}</small>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/if}
  {/if}
</section>

<style>
  .analytics {
    min-width: 0;
    padding: 28px;
    overflow: auto;
  }
  .analytics h1 {
    margin: 0 0 24px;
    font-size: 22px;
    letter-spacing: -0.7px;
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  .analytics-card {
    background: var(--surface);
    border-radius: 16px;
    padding: 22px;
    margin-top: 20px;
  }
  .chart-heading {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    align-items: center;
  }
  .chart-heading strong {
    font: 600 12px var(--mono);
    color: var(--green);
  }
  .analytics-notes {
    color: var(--note);
    margin-top: 12px;
    font-size: 11px;
  }
  .filters input,
  .filters select {
    background: var(--surface);
    border: 1px solid var(--line);
    padding: 8px;
    border-radius: 8px;
    color: var(--ink);
    font: inherit;
  }
  .analytics.detail {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .detail > h1 {
    flex: 0 0 auto;
    margin: 14px;
  }
  .back {
    flex: 0 0 auto;
    align-self: flex-start;
    margin: 0 14px;
  }
  .filters,
  .day-list {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .filters label {
    display: flex;
    gap: 4px;
    align-items: center;
  }
  .day-list {
    margin: 12px 0;
    max-height: 180px;
    overflow: auto;
  }
  .chart-scroll {
    overflow-x: auto;
  }
  svg {
    height: 240px;
    display: block;
  }
  svg line {
    stroke: var(--line);
  }
  svg text {
    fill: var(--ink);
    font-size: 11px;
  }
  rect {
    fill: #86c68c;
  }
  .chosen {
    background: var(--selection);
    outline: 2px solid var(--green);
  }
</style>
