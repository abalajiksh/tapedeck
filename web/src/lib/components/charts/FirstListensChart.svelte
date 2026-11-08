<script lang="ts">
	/**
	 * When each of the range's top artists entered the library: a dot on the
	 * day you first heard them, sized by how much you played them in the range,
	 * and a line running on to the last time you did.
	 *
	 * First and last listens are **all-time** facts even on a ranged page —
	 * "when did they arrive" is not a question about the last 30 days — so the
	 * axis always runs from the earliest arrival to today.
	 */
	import { scaleSqrt, scaleUtc } from 'd3-scale';
	import ChartTip from './ChartTip.svelte';
	import { fmtTs, nf } from './graphs';
	import { openArtist } from '$lib/entities';

	export let artists: { name: string; plays: number; first: number; last: number; loyalty: number }[] = [];

	const ROW = 28;
	const LABEL = 170;
	const RIGHT = 16;
	const AXIS = 24;

	let width = 0;
	$: plotW = Math.max(40, width - LABEL - RIGHT);
	$: now = Date.now();
	$: x = scaleUtc()
		.domain([new Date(Math.min(...artists.map((a) => a.first)) * 1000), new Date(now)])
		.range([0, plotW]);
	$: r = scaleSqrt().domain([0, Math.max(1, ...artists.map((a) => a.plays))]).range([3, 13]);
	$: ticks = x.ticks(Math.max(2, Math.floor(plotW / 110)));
	$: spanYears = (now / 1000 - Math.min(...artists.map((a) => a.first))) / (365.25 * 86400);
	$: tickFmt = (d: Date) =>
		d.toLocaleDateString(undefined, spanYears > 3 ? { year: 'numeric' } : { month: 'short', year: '2-digit' });
	$: h = artists.length * ROW + AXIS;

	let hover: number | null = null;
	let tipY = 0;
	let tipX = 0;
	const trunc = (s: string, n = 22) => (s.length > n ? s.slice(0, n - 1) + '…' : s);
</script>

{#if artists.length}
	<div class="wrap" bind:clientWidth={width}>
		<svg {width} height={h} role="img" aria-label="When your top artists first appeared">
			{#each ticks as t}
				<line x1={LABEL + x(t)} x2={LABEL + x(t)} y1="0" y2={h - AXIS} class="grid" />
				<text x={LABEL + x(t)} y={h - 8} text-anchor="middle" class="tick">{tickFmt(t)}</text>
			{/each}
			{#each artists as a, i}
				{@const cy = i * ROW + ROW / 2}
				{@const x0 = LABEL + x(new Date(a.first * 1000))}
				{@const x1 = LABEL + x(new Date(a.last * 1000))}
				<g class="row" class:on={hover === i} role="button" tabindex="0"
					on:click={() => openArtist(a.name)}
					on:keydown={(e) => e.key === 'Enter' && openArtist(a.name)}
					on:pointerenter={() => { hover = i; tipY = cy + 10; tipX = x0; }}
					on:pointerleave={() => (hover = null)}
					on:focus={() => { hover = i; tipY = cy + 10; tipX = x0; }}
					on:blur={() => (hover = null)}>
					<rect x="0" y={i * ROW} {width} height={ROW} class="hit" />
					<text x={LABEL - 12} y={cy} dy="0.32em" text-anchor="end" class="name">{trunc(a.name)}</text>
					<line x1={x0} x2={Math.max(x0, x1)} y1={cy} y2={cy} class="span" />
					<circle cx={x0} cy={cy} r={r(a.plays)} class="dot" />
				</g>
			{/each}
		</svg>
		<ChartTip x={tipX} y={tipY} width={width} show={hover !== null}>
			{#if hover !== null}
				{@const a = artists[hover]}
				<strong>{a.name}</strong><br />
				First heard {fmtTs(a.first)}<br />
				Last heard {fmtTs(a.last)}<br />
				<span class="sub">{nf(a.plays)} plays in this range</span>
			{/if}
		</ChartTip>
	</div>
	<p class="caption">
		Your most-played artists in this range, in the order they arrived. The dot is your first listen
		ever, sized by plays in the range; the line runs to your most recent one.
	</p>
{/if}

<style>
	.wrap { position: relative; width: 100%; }
	svg { display: block; overflow: visible; }
	.grid { stroke: var(--td-border); stroke-width: 1; }
	.tick { font-size: 11px; fill: var(--td-muted-3); font-family: inherit; }
	.name { font-size: 12.5px; fill: var(--td-text); font-family: inherit; }
	.hit { fill: transparent; }
	.row { cursor: pointer; outline: none; }
	.row.on .hit { fill: var(--td-track); opacity: 0.45; }
	.span { stroke: var(--td-accent); stroke-width: 2; opacity: 0.35; stroke-linecap: round; }
	.dot { fill: var(--td-accent); stroke: var(--td-surface); stroke-width: 2; }
	.sub { color: var(--td-muted-3); }
	.caption { font-size: 12px; color: var(--td-muted-3); margin: 8px 0 0; }
</style>
