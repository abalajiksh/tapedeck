<script lang="ts">
	/**
	 * A ranked list for the Classical, Scores and lens tabs — Top Charts' row,
	 * with two things it does not need: a row can **open** in place (an
	 * opera's arias, a song's covers), and a row can carry small actions
	 * ("Not Christmas", "Not classical") that correct the classifier.
	 */
	import { createEventDispatcher } from 'svelte';
	import CoverArt from '$lib/components/CoverArt.svelte';
	import type { InsightRow } from '$lib/api';

	export let rows: InsightRow[] = [];
	export let metric: 'plays' | 'hours' | 'hearings' = 'plays';
	/** Rounded tiles for people, square for records. */
	export let round = false;
	/** Whether a row can be opened in place; the parent fills `expanded`. */
	export let expandable: (r: InsightRow) => boolean = () => false;
	export let expanded: Record<string, InsightRow[] | 'loading'> = {};
	/** Actions offered on every row, e.g. `[{ id: 'not', label: 'Not Christmas' }]`. */
	export let actions: { id: string; label: string }[] = [];
	/** The trailing words after the value: "3 recordings". */
	export let extra: (r: InsightRow) => string = () => '';

	const dispatch = createEventDispatcher<{
		open: InsightRow;
		toggle: InsightRow;
		action: { id: string; row: InsightRow };
	}>();

	const value = (r: InsightRow) =>
		metric === 'hours'
			? Math.round((r.seconds / 3600) * 10) / 10
			: metric === 'hearings'
				? (r.hearings ?? 0)
				: r.plays;
	$: unit = metric === 'hours' ? 'h' : '';
	$: max = Math.max(1, ...rows.map(value));
</script>

<div class="panel list">
	{#each rows as r, i (r.key)}
		{@const kids = expanded[r.key]}
		<div class="item">
			<div class="row">
				<span class="rank h-display">{i + 1}</span>
				<button class="main" on:click={() => dispatch('open', r)}>
					<CoverArt url={r.artwork_url} caaId={r.caa_id ?? null} releaseMbid={r.caa_release_mbid ?? null}
						seed={r.name} size={38} radius={round ? 999 : 9} />
					<div class="text">
						<div class="name">
							{r.name}
							{#if r.inferred}<span class="dot" title="Role inferred: MusicBrainz does not describe these recordings, so Tapedeck guessed from the name or from their other recordings.">•</span>{/if}
						</div>
						{#if r.sub || extra(r)}
							<div class="sub">{[r.sub, extra(r)].filter(Boolean).join(' · ')}</div>
						{/if}
						<div class="bar"><div class="fill" style="width:{(value(r) / max) * 100}%"></div></div>
					</div>
				</button>
				{#if expandable(r)}
					<button class="more" aria-expanded={!!expanded[r.key]} title="Open" on:click={() => dispatch('toggle', r)}>
						<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.4" aria-hidden="true"
							style="transform:rotate({expanded[r.key] ? 180 : 0}deg)"><path d="M6 9l6 6 6-6" /></svg>
					</button>
				{/if}
				{#each actions as a}
					<button class="act" on:click={() => dispatch('action', { id: a.id, row: r })}>{a.label}</button>
				{/each}
				<span class="val">{value(r)}{unit}</span>
			</div>
			{#if kids === 'loading'}
				<div class="inner muted">Opening…</div>
			{:else if Array.isArray(kids)}
				<div class="inner">
					{#each kids as c}
						<div class="child">
							<span class="cname">{c.name}</span>
							{#if c.sub}<span class="csub">{c.sub}</span>{/if}
							<span class="cval">{c.plays} {c.plays === 1 ? 'play' : 'plays'}</span>
						</div>
					{/each}
				</div>
			{/if}
		</div>
	{/each}
</div>

<style>
	.list { border-radius: 22px; overflow: hidden; }
	.item { border-bottom: 1px solid var(--td-border); }
	.item:last-child { border-bottom: none; }
	.row { display: flex; align-items: center; gap: 10px; padding: 10px 16px; }
	.row:hover { background: var(--td-nav-hover); }
	.rank { width: 28px; flex: none; text-align: center; font-size: 16px; color: var(--td-muted-3); font-variant-numeric: tabular-nums; }
	.main {
		flex: 1; min-width: 0; display: flex; align-items: center; gap: 14px;
		border: none; background: transparent; color: inherit; font: inherit; text-align: left; cursor: pointer; padding: 0;
	}
	.text { flex: 1; min-width: 0; }
	.name { font-size: 14.5px; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.dot { color: var(--td-accent); margin-left: 4px; cursor: help; }
	.sub { font-size: 12.5px; color: var(--td-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.bar { height: 6px; border-radius: 999px; background: var(--td-track); overflow: hidden; max-width: 260px; margin-top: 6px; }
	.fill { height: 100%; border-radius: 999px; background: linear-gradient(90deg, var(--td-accent-2), var(--td-accent)); }
	.val { font-size: 13px; font-weight: 600; width: 64px; text-align: right; flex: none; font-variant-numeric: tabular-nums; }
	.more, .act {
		flex: none; border: 1px solid var(--td-border); background: var(--td-surface); color: var(--td-muted);
		border-radius: 999px; cursor: pointer; font: inherit; font-size: 11.5px; padding: 4px 10px;
		display: inline-flex; align-items: center;
	}
	.more { padding: 4px 6px; }
	.more:hover, .act:hover { border-color: var(--td-accent); color: var(--td-text); }
	.more svg { transition: transform 0.15s; }
	.inner { padding: 4px 16px 12px 70px; display: flex; flex-direction: column; gap: 4px; }
	.muted { color: var(--td-muted); font-size: 12.5px; }
	.child { display: flex; gap: 10px; align-items: baseline; font-size: 13px; min-width: 0; }
	.cname { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
	.csub { color: var(--td-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; flex: 1; }
	.cval { color: var(--td-muted-2); font-variant-numeric: tabular-nums; white-space: nowrap; margin-left: auto; }
	@media (max-width: 640px) {
		.act { display: none; }
		.inner { padding-left: 16px; }
	}
</style>
