<script lang="ts">
	/**
	 * Listens per day across the range, with the median active day drawn
	 * across it and the busiest day marked.
	 *
	 * One bar per calendar day, including the empty ones — a gap in the
	 * listening is part of its shape. Across a whole history that is well over
	 * a thousand bars a pixel or less wide, which is fine: the hover reads the
	 * day under the pointer from the scale, so the target is the column, not
	 * the bar.
	 */
	import { LayerCake, Svg } from 'layercake';
	import { scaleLinear } from 'd3-scale';
	import ChartTip from './ChartTip.svelte';
	import { dayAt, dayTicks, fmtDate, nf, pointerX, valueTicks } from './graphs';

	export let start: string;
	export let daily: number[] = [];
	export let median = 0;
	export let peak: { day: string; plays: number } | undefined = undefined;
	export let height = 220;

	const pad = { top: 18, right: 8, bottom: 24, left: 36 };

	$: n = daily.length;
	$: max = Math.max(1, ...daily);
	$: ticks = dayTicks(start, n);
	$: yTicks = valueTicks(max);
	$: peakIndex = peak ? daily.indexOf(peak.plays) : -1;

	let hover: number | null = null;
	let tipX = 0;
	let tipY = 0;
	let wrapWidth = 0;

	function move(e: PointerEvent, invert: (px: number) => number) {
		const x = pointerX(e, pad.left);
		const i = Math.floor(invert(x));
		hover = i >= 0 && i < n ? i : null;
		tipX = x + pad.left;
		tipY = 8;
	}
</script>

{#if n}
	<div class="wrap" style="height:{height}px;" bind:clientWidth={wrapWidth}>
		<LayerCake
			padding={pad}
			x="i"
			y="v"
			xScale={scaleLinear()}
			yScale={scaleLinear()}
			xDomain={[0, n]}
			yDomain={[0, max]}
			data={[]}
			let:xScale
			let:yScale
			let:width
			let:height={h}
		>
			<Svg>
				{#each yTicks as t}
					<line x1="0" x2={width} y1={yScale(t)} y2={yScale(t)} class="grid" />
					<text x="-6" y={yScale(t)} dy="0.32em" text-anchor="end" class="tick">{nf(t)}</text>
				{/each}
				{#each daily as v, i}
					{#if v > 0}
						{@const x0 = xScale(i)}
						{@const w = Math.max(1, xScale(i + 1) - x0 - (xScale(1) - xScale(0) > 3 ? 1 : 0))}
						<rect x={x0} y={yScale(v)} width={w} height={h - yScale(v)}
							fill={i === peakIndex ? 'var(--td-accent-hi)' : 'var(--td-accent)'}
							opacity={hover === null || hover === i ? 0.9 : 0.55} />
					{/if}
				{/each}
				{#if median > 0}
					<!-- Labelled in the caption rather than here: on a busy history the
					     bars cover any label placed on the line. -->
					<line x1="0" x2={width} y1={yScale(median)} y2={yScale(median)} class="median" />
				{/if}
				{#if peak && peakIndex >= 0}
					{@const px = xScale(peakIndex + 0.5)}
					<text x={px} y={yScale(peak.plays) - 6} text-anchor={px > width * 0.7 ? 'end' : 'start'} class="note strong">
						peak · {nf(peak.plays)} on {fmtDate(dayAt(start, peakIndex))}
					</text>
				{/if}
				{#each ticks as t}
					<text x={xScale(t.i)} y={h + 16} text-anchor="middle" class="tick">{t.label}</text>
				{/each}
				{#if hover !== null}
					<line x1={xScale(hover + 0.5)} x2={xScale(hover + 0.5)} y1="0" y2={h} class="cross" />
				{/if}
				<rect x="0" y="0" {width} height={h} fill="transparent" role="presentation"
					on:pointermove={(e) => move(e, xScale.invert)}
					on:pointerleave={() => (hover = null)} />
			</Svg>
		</LayerCake>
		<ChartTip x={tipX} y={tipY} width={wrapWidth} show={hover !== null}>
			{#if hover !== null}
				<strong>{fmtDate(dayAt(start, hover), true)}</strong><br />
				{nf(daily[hover])} listen{daily[hover] === 1 ? '' : 's'}
			{/if}
		</ChartTip>
	</div>
	{#if median > 0}
		<p class="caption">
			The dashed line is your median day with any listening: {nf(median)} listens. Days with none
			are left out of it.
		</p>
	{/if}
{/if}

<style>
	.wrap { position: relative; width: 100%; }
	.grid { stroke: var(--td-border); stroke-width: 1; }
	.median { stroke: var(--td-muted-2); stroke-width: 1; stroke-dasharray: 3 3; }
	.cross { stroke: var(--td-muted-2); stroke-width: 1; }
	.tick { font-size: 11px; fill: var(--td-muted-3); font-family: inherit; }
	.note { font-size: 11px; fill: var(--td-muted-2); font-family: inherit; }
	.caption { font-size: 12px; color: var(--td-muted-3); margin: 8px 0 0; }
	.strong { fill: var(--td-text); font-weight: 600; }
</style>
