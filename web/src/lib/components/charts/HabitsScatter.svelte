<script lang="ts">
	/**
	 * Daily habit or burst. Across: how often an artist comes back — the days
	 * you played them over the days from first to last. Up: how much of their
	 * listening landed on one single day. A habit sits low and to the right; a
	 * binge sits high on the left.
	 *
	 * Both axes are scaled to the data rather than fixed at 0–1. The script
	 * drew the whole unit square and shaded its top half "burst", and on a real
	 * history every artist sat below the line — half the chart was an empty
	 * pink field. The corner notes carry the reading instead.
	 */
	import { LayerCake, Svg } from 'layercake';
	import { scaleLinear, scaleSqrt } from 'd3-scale';
	import ChartTip from './ChartTip.svelte';
	import { fmtDate, nf, parseDay, pct } from './graphs';
	import { openArtist } from '$lib/entities';

	export let habits: {
		name: string;
		plays: number;
		active_days: number;
		span_days: number;
		return_rate: number;
		top_day_share: number;
		top_day: string;
	}[] = [];
	export let height = 340;

	const pad = { top: 14, right: 16, bottom: 34, left: 42 };

	$: xMax = Math.min(1, Math.max(0.05, ...habits.map((h) => h.return_rate)) * 1.08);
	$: yMax = Math.min(1, Math.max(0.05, ...habits.map((h) => h.top_day_share)) * 1.12);
	$: r = scaleSqrt().domain([0, Math.max(1, ...habits.map((h) => h.plays))]).range([3, 16]);
	/** Who gets a name on the plot, in priority order: the ten biggest, then
	 *  the three burstiest. The rest are named on hover. */
	$: candidates = [
		...habits.slice(0, 10),
		...[...habits].sort((a, b) => b.top_day_share - a.top_day_share).slice(0, 3)
	].filter((h, i, all) => all.indexOf(h) === i);

	/**
	 * Place labels greedily, skipping any that would overlap one already
	 * placed or a bigger dot. A dense cluster then shows its biggest names
	 * cleanly instead of all of them on top of each other.
	 */
	function placeLabels(list: typeof candidates, xs: (v: number) => number, ys: (v: number) => number, width: number) {
		const boxes: { x0: number; x1: number; y0: number; y1: number }[] = [];
		const out: { name: string; x: number; y: number; end: boolean }[] = [];
		for (const d of list) {
			const cx = xs(d.return_rate);
			const cy = ys(d.top_day_share);
			const rr = r(d.plays);
			const w = d.name.length * 6.4;
			const end = cx + rr + 4 + w > width;
			const x = end ? cx - rr - 4 : cx + rr + 4;
			const box = { x0: end ? x - w : x, x1: end ? x : x + w, y0: cy - 7, y1: cy + 7 };
			if (boxes.some((b) => b.x0 < box.x1 && box.x0 < b.x1 && b.y0 < box.y1 && box.y0 < b.y1)) continue;
			boxes.push(box);
			out.push({ name: d.name, x, y: cy, end });
		}
		return out;
	}
	/** Big dots underneath, so a small one is never hidden behind a large one. */
	$: drawn = [...habits].sort((a, b) => b.plays - a.plays);

	let hover: (typeof habits)[number] | null = null;
	let tipX = 0;
	let tipY = 0;
	let wrapWidth = 0;
</script>

{#if habits.length}
	<div class="reading">
		<span><strong>Bursts</strong> sit high on the left — much of it on one day.</span>
		<span><strong>Habits</strong> sit low on the right — you keep coming back.</span>
	</div>
	<div class="wrap" style="height:{height}px;" bind:clientWidth={wrapWidth}>
		<LayerCake padding={pad} xScale={scaleLinear()} yScale={scaleLinear()} xNice yNice
			xDomain={[0, xMax]} yDomain={[0, yMax]} data={[]}
			let:xScale let:yScale let:width let:height={h}>
			<Svg>
				{#each xScale.ticks(5) as t}
					<line x1={xScale(t)} x2={xScale(t)} y1="0" y2={h} class="grid" />
					<text x={xScale(t)} y={h + 16} text-anchor="middle" class="tick">{pct(t)}</text>
				{/each}
				{#each yScale.ticks(5) as t}
					<line x1="0" x2={width} y1={yScale(t)} y2={yScale(t)} class="grid" />
					<text x="-6" y={yScale(t)} dy="0.32em" text-anchor="end" class="tick">{pct(t)}</text>
				{/each}
				{#each drawn as d}
					{@const cx = xScale(d.return_rate)}
					{@const cy = yScale(d.top_day_share)}
					<circle {cx} {cy} r={r(d.plays)} class="dot" class:on={hover === d}
						role="button" tabindex="0" aria-label={d.name}
						on:pointerenter={() => { hover = d; tipX = cx + pad.left; tipY = cy + pad.top; }}
						on:pointerleave={() => (hover = null)}
						on:focus={() => { hover = d; tipX = cx + pad.left; tipY = cy + pad.top; }}
						on:blur={() => (hover = null)}
						on:click={() => openArtist(d.name)}
						on:keydown={(e) => e.key === 'Enter' && openArtist(d.name)} />
				{/each}
				{#each placeLabels(candidates, xScale, yScale, width) as l}
					<text x={l.x} y={l.y} dy="0.32em" text-anchor={l.end ? 'end' : 'start'} class="label">{l.name}</text>
				{/each}
				<text x={width / 2} y={h + 30} text-anchor="middle" class="axis">comes back on this share of days →</text>
			</Svg>
		</LayerCake>
		<ChartTip x={tipX} y={tipY} width={wrapWidth} show={hover !== null}>
			{#if hover}
				<strong>{hover.name}</strong> · {nf(hover.plays)} plays<br />
				Played on {nf(hover.active_days)} of {nf(hover.span_days)} days ({pct(hover.return_rate)})<br />
				Biggest day {fmtDate(parseDay(hover.top_day))}: {pct(hover.top_day_share)} of their plays
			{/if}
		</ChartTip>
	</div>
	<p class="caption">
		Up: the share of an artist's plays on their single biggest day. Across: of the days from the
		first to the last time you played them in this range, how many you did. Dot size is plays. Artists
		with fewer than five plays, or heard on only one day, are left out.
	</p>
{/if}

<style>
	.wrap { position: relative; width: 100%; }
	.grid { stroke: var(--td-border); stroke-width: 1; }
	.tick { font-size: 11px; fill: var(--td-muted-3); font-family: inherit; }
	.axis { font-size: 11.5px; fill: var(--td-muted-2); font-family: inherit; }
	.reading { display: flex; flex-wrap: wrap; gap: 4px 22px; font-size: 12.5px; color: var(--td-muted); margin-bottom: 10px; }
	.reading strong { color: var(--td-text); }
	.label { font-size: 11.5px; fill: var(--td-text); font-family: inherit; pointer-events: none; }
	.dot { fill: var(--td-accent); fill-opacity: 0.72; stroke: var(--td-surface); stroke-width: 2; cursor: pointer; outline: none; }
	.dot.on { fill-opacity: 1; stroke: var(--td-text); }
	.caption { font-size: 12px; color: var(--td-muted-3); margin: 8px 0 0; }
</style>
