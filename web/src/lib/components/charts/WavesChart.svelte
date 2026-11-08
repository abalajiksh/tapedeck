<script lang="ts">
	/**
	 * Obsession waves: the range's six most-played artists, each as a smoothed
	 * daily line, so the fortnight you could not stop playing someone shows as
	 * a wave and a steady favourite as a long low swell.
	 *
	 * Smoothed with a trailing 7-day mean once the range is long enough for
	 * that to mean something; a week is drawn day by day. Colour is assigned in
	 * the fixed series order, ranked by plays, and the legend names every line —
	 * colour is never the only way to tell them apart.
	 */
	import { LayerCake, Svg } from 'layercake';
	import { scaleLinear } from 'd3-scale';
	import { area, line, curveMonotoneX } from 'd3-shape';
	import ChartTip from './ChartTip.svelte';
	import { SERIES, dayAt, dayTicks, fmtDate, nf, pointerX, rolling, valueTicks } from './graphs';
	import { openArtist } from '$lib/entities';

	export let start: string;
	export let waves: { name: string; plays: number; daily: number[] }[] = [];
	export let height = 260;

	const pad = { top: 10, right: 8, bottom: 24, left: 36 };

	$: n = waves[0]?.daily.length ?? 0;
	$: smoothing = n > 60 ? 7 : 1;
	$: smooth = waves.map((w, s) => ({ ...w, color: SERIES[s], values: rolling(w.daily, smoothing) }));
	$: max = Math.max(1, ...smooth.flatMap((w) => w.values));
	$: ticks = dayTicks(start, n);
	$: yTicks = valueTicks(max);

	/** One artist picked out from the legend; the others fade. */
	let focus: number | null = null;
	let hover: number | null = null;
	let tipX = 0;
	let wrapWidth = 0;

	function move(e: PointerEvent, invert: (px: number) => number) {
		const x = pointerX(e, pad.left);
		const i = Math.round(invert(x));
		hover = i >= 0 && i < n ? i : null;
		tipX = x + pad.left;
	}

	const fmt = (v: number) => (smoothing > 1 ? v.toFixed(1) : nf(v));
</script>

{#if n && waves.length}
	<div class="legend">
		{#each smooth as w, s}
			<button class="item" class:dim={focus !== null && focus !== s}
				on:mouseenter={() => (focus = s)} on:mouseleave={() => (focus = null)}
				on:focus={() => (focus = s)} on:blur={() => (focus = null)}
				on:click={() => openArtist(w.name)} title="Open {w.name}">
				<span class="dot" style="background:{w.color};"></span>{w.name}
				<span class="n">{nf(w.plays)}</span>
			</button>
		{/each}
	</div>
	<div class="wrap" style="height:{height}px;" bind:clientWidth={wrapWidth}>
		<LayerCake padding={pad} xScale={scaleLinear()} yScale={scaleLinear()}
			xDomain={[0, Math.max(1, n - 1)]} yDomain={[0, max]} data={[]}
			let:xScale let:yScale let:width let:height={h}>
			{@const ln = line<number>().x((_, i) => xScale(i)).y((v) => yScale(v)).curve(curveMonotoneX)}
			{@const ar = area<number>().x((_, i) => xScale(i)).y0(h).y1((v) => yScale(v)).curve(curveMonotoneX)}
			<Svg>
				{#each yTicks as t}
					<line x1="0" x2={width} y1={yScale(t)} y2={yScale(t)} class="grid" />
					<text x="-6" y={yScale(t)} dy="0.32em" text-anchor="end" class="tick">{nf(t)}</text>
				{/each}
				{#each smooth as w, s}
					{@const faded = focus !== null && focus !== s}
					<path d={ar(w.values)} fill={w.color} opacity={faded ? 0.03 : 0.14} />
					<path d={ln(w.values)} fill="none" stroke={w.color} stroke-width="2"
						stroke-linejoin="round" opacity={faded ? 0.18 : 1} />
				{/each}
				{#each ticks as t}
					<text x={xScale(t.i)} y={h + 16} text-anchor="middle" class="tick">{t.label}</text>
				{/each}
				{#if hover !== null}
					<line x1={xScale(hover)} x2={xScale(hover)} y1="0" y2={h} class="cross" />
					{#each smooth as w}
						<circle cx={xScale(hover)} cy={yScale(w.values[hover])} r="4" fill={w.color}
							stroke="var(--td-surface)" stroke-width="2" />
					{/each}
				{/if}
				<rect x="0" y="0" {width} height={h} fill="transparent" role="presentation"
					on:pointermove={(e) => move(e, xScale.invert)} on:pointerleave={() => (hover = null)} />
			</Svg>
		</LayerCake>
		<ChartTip x={tipX} y={8} width={wrapWidth} show={hover !== null}>
			{#if hover !== null}
				{@const h = hover}
				<strong>{fmtDate(dayAt(start, h))}</strong>
				{#if smoothing > 1}<span class="sub"> · 7-day average</span>{/if}
				{#each [...smooth].sort((a, b) => b.values[h] - a.values[h]) as w}
					<div class="row"><span class="dot" style="background:{w.color};"></span>
						<span class="nm">{w.name}</span><span class="v">{fmt(w.values[h])}</span></div>
				{/each}
			{/if}
		</ChartTip>
	</div>
	<p class="caption">
		Listens a day{smoothing > 1 ? ', averaged over the previous seven days' : ''}, for the
		{waves.length} artists you played most in this range. Click a name to open them.
	</p>
{/if}

<style>
	.wrap { position: relative; width: 100%; }
	.legend { display: flex; flex-wrap: wrap; gap: 4px 14px; margin-bottom: 10px; }
	.item {
		display: inline-flex; align-items: center; gap: 6px; font-size: 12.5px;
		color: var(--td-text); background: none; border: 0; padding: 2px 0; cursor: pointer;
		font-family: inherit; transition: opacity 0.15s;
	}
	.item.dim { opacity: 0.4; }
	.item .n { color: var(--td-muted-3); font-size: 11.5px; }
	.dot { width: 9px; height: 9px; border-radius: 50%; flex: none; display: inline-block; }
	.row { display: flex; align-items: center; gap: 6px; }
	.row .nm { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.row .v { font-variant-numeric: tabular-nums; color: var(--td-muted); }
	.sub { color: var(--td-muted-3); }
	.grid { stroke: var(--td-border); stroke-width: 1; }
	.cross { stroke: var(--td-muted-2); stroke-width: 1; }
	.tick { font-size: 11px; fill: var(--td-muted-3); font-family: inherit; }
	.caption { font-size: 12px; color: var(--td-muted-3); margin: 8px 0 0; }
</style>
