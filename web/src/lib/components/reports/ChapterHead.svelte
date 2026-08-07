<script lang="ts">
	/**
	 * The head of a week or month chapter: colour block, title, blurb, Share,
	 * then the headline band — the big number, its delta, and the rank badge.
	 *
	 * Shared because the week and month layouts differ only in scale. The year
	 * chapter is a different shape entirely (a bento grid, not a card) and does
	 * not use this.
	 *
	 * The badge is where the design puts "You out-listened 88% of Tapedeck
	 * listeners". That needs other people's listening, and on a private
	 * household instance it quietly reports how much everyone else plays — so
	 * the slot keeps its shape and says something true about *your own* history
	 * instead. See the note at the top of `reports.rs`.
	 */
	import type { ReportPeriod } from '$lib/api';
	import { nf, delta, ordinal } from './bits';

	export let p: ReportPeriod;
	export let unit: 'week' | 'month';
	export let onShare: (() => void) | null = null;

	$: big = unit === 'week' ? 60 : 72;
	$: swatch = unit === 'week' ? 48 : 54;
	$: title = unit === 'week' ? 23 : 26;

	$: d = delta(p.plays, p.prev?.plays);

	$: rankText =
		p.rank === 1
			? `Your busiest ${unit} on record`
			: p.rank > 0
				? `Your ${ordinal(p.rank)} busiest ${unit} of ${p.rank_of}`
				: '';
</script>

<header class="head">
	<div class="swatch" style="width:{swatch}px;height:{swatch}px;background:{p.colour ?? 'var(--td-track)'};"
		title={p.colour ? `This ${unit} sounded like ${p.colour}` : 'Colour fills in as listens are enriched'}></div>
	<div style="flex:1;min-width:220px;">
		<div class="h-display" style="font-size:{title}px;line-height:1.1;">{p.label}</div>
		<slot name="blurb" />
	</div>
	{#if onShare}
		<button class="btn btn-secondary share" on:click={onShare}>
			<svg viewBox="0 0 24 24" width="15" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="18" cy="5" r="3"/><circle cx="6" cy="12" r="3"/><circle cx="18" cy="19" r="3"/><path d="M8.6 13.5l6.8 4M15.4 6.5l-6.8 4"/></svg>Share
		</button>
	{/if}
</header>

<div class="band">
	<div style="flex:1;min-width:220px;">
		<div class="h-display" style="font-size:{big}px;line-height:.9;">{nf(p.plays)}</div>
		<div class="unit">scrobbles{unit === 'week' ? ' this week' : ''}</div>
		{#if d != null}
			<div class="delta" class:up={d > 0} class:down={d < 0}>
				{d > 0 ? '▲' : d < 0 ? '▼' : '—'} {Math.abs(d)}% vs {p.prev?.label}
				<span class="was">· {nf(p.prev?.plays ?? 0)} then</span>
			</div>
		{/if}
		{#if rankText}
			<div class="rank">
				<svg viewBox="0 0 24 24" width="17" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M5 13l7-7 7 7"/><path d="M5 19l7-7 7 7"/></svg>
				<span>{rankText}</span>
			</div>
		{/if}
	</div>
	<div style="flex:1;min-width:300px;">
		<slot name="chart" />
	</div>
</div>

<style>
	.head {
		display: flex; align-items: flex-start; gap: 16px;
		flex-wrap: wrap; margin-bottom: 24px;
	}
	.swatch { border-radius: 15px; flex: none; box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.12); }
	.share { font-size: 13px; padding: 9px 16px; gap: 7px; }
	.band {
		display: flex; gap: 26px; flex-wrap: wrap;
		align-items: center; margin-bottom: 24px;
	}
	.unit { font-size: 14.5px; color: var(--td-muted); margin-top: 6px; }
	.delta { margin-top: 12px; font-size: 14px; font-weight: 600; color: var(--td-muted-2); }
	.delta.up { color: var(--td-good-fg); }
	.delta.down { color: #b2622d; }
	.was { color: var(--td-muted-3); font-weight: 400; }
	.rank {
		margin-top: 14px; display: inline-flex; align-items: center; gap: 10px;
		background: var(--td-good-bg); color: var(--td-good-fg);
		border-radius: 14px; padding: 9px 13px;
		font-size: 12.5px; font-weight: 600;
	}
</style>
