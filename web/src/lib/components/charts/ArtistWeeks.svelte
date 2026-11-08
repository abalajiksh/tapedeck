<script lang="ts">
	/**
	 * Every week of the history, this artist's listens stacked under everyone
	 * else's: how big a part of the listening they were, and when.
	 *
	 * Both parts come from one pair the server counted together (`plays` and
	 * `all`, skip-free, same week), so the stack always adds up to the week.
	 */
	import { LayerCake, Svg } from 'layercake';
	import { scaleLinear } from 'd3-scale';
	import ChartTip from './ChartTip.svelte';
	import { dayTicks, fmtDate, nf, parseDay, pointerX, valueTicks } from './graphs';

	export let weeks: { week: string; plays: number; all: number }[] = [];
	export let name: string;
	export let height = 220;

	const pad = { top: 10, right: 8, bottom: 24, left: 40 };

	$: n = weeks.length;
	$: max = Math.max(1, ...weeks.map((w) => w.all));
	$: ticks = n ? dayTicks(weeks[0].week, n * 7).map((t) => ({ ...t, i: t.i / 7 })) : [];
	$: yTicks = valueTicks(max);

	let hover: number | null = null;
	let tipX = 0;
	let wrapWidth = 0;
	function move(e: PointerEvent, invert: (px: number) => number) {
		const x = pointerX(e, pad.left);
		const i = Math.floor(invert(x));
		hover = i >= 0 && i < n ? i : null;
		tipX = x + pad.left;
	}
	const share = (a: number, b: number) => (b ? `${Math.round((a / b) * 100)}%` : '—');
</script>

{#if n > 1}
	<div class="legend">
		<span><i style="background:var(--td-accent);"></i>{name}</span>
		<span><i style="background:var(--td-muted-3);opacity:0.5;"></i>everyone else</span>
	</div>
	<div class="wrap" style="height:{height}px;" bind:clientWidth={wrapWidth}>
		<LayerCake padding={pad} xScale={scaleLinear()} yScale={scaleLinear()}
			xDomain={[0, n]} yDomain={[0, max]} data={[]}
			let:xScale let:yScale let:width let:height={h}>
			<Svg>
				{#each yTicks as t}
					<line x1="0" x2={width} y1={yScale(t)} y2={yScale(t)} class="grid" />
					<text x="-6" y={yScale(t)} dy="0.32em" text-anchor="end" class="tick">{nf(t)}</text>
				{/each}
				{#each weeks as w, i}
					{@const x0 = xScale(i)}
					{@const bw = Math.max(1, xScale(i + 1) - x0 - (xScale(1) - xScale(0) > 3 ? 1 : 0))}
					{#if w.all > 0}
						<rect x={x0} y={yScale(w.all)} width={bw} height={yScale(w.plays) - yScale(w.all)}
							fill="var(--td-muted-3)" opacity={hover === null || hover === i ? 0.45 : 0.3} />
					{/if}
					{#if w.plays > 0}
						<rect x={x0} y={yScale(w.plays)} width={bw} height={h - yScale(w.plays)}
							fill="var(--td-accent)" />
					{/if}
				{/each}
				{#each ticks as t}
					<text x={xScale(t.i)} y={h + 16} text-anchor="middle" class="tick">{t.label}</text>
				{/each}
				<rect x="0" y="0" {width} height={h} fill="transparent" role="presentation"
					on:pointermove={(e) => move(e, xScale.invert)} on:pointerleave={() => (hover = null)} />
			</Svg>
		</LayerCake>
		<ChartTip x={tipX} y={8} width={wrapWidth} show={hover !== null}>
			{#if hover !== null}
				{@const w = weeks[hover]}
				<strong>Week of {fmtDate(parseDay(w.week))}</strong><br />
				{name}: {nf(w.plays)} of {nf(w.all)} listens ({share(w.plays, w.all)})
			{/if}
		</ChartTip>
	</div>
{/if}

<style>
	.wrap { position: relative; width: 100%; }
	.legend { display: flex; gap: 16px; font-size: 12.5px; color: var(--td-muted); margin-bottom: 8px; }
	.legend i { display: inline-block; width: 10px; height: 10px; border-radius: 3px; margin-right: 6px; vertical-align: -1px; }
	.grid { stroke: var(--td-border); stroke-width: 1; }
	.tick { font-size: 11px; fill: var(--td-muted-3); font-family: inherit; }
</style>
