<script lang="ts">
	/**
	 * The long tail: artists ranked by plays, and the running share of
	 * everything they add up to. A steep start means a few artists carry the
	 * listening; a long flat run is everyone you heard once or twice.
	 *
	 * A share of artist **credits**, not listens — a listen with two artists
	 * counts for both — because artists are counted over everyone credited,
	 * the way the charts count them. The caption says so.
	 */
	import { LayerCake, Svg } from 'layercake';
	import { scaleLinear } from 'd3-scale';
	import { area, line } from 'd3-shape';
	import ChartTip from './ChartTip.svelte';
	import { nf, pointerX } from './graphs';

	export let counts: number[] = [];
	export let credits = 0;
	export let half = 0;
	export let eighty = 0;
	export let height = 300;

	const pad = { top: 12, right: 10, bottom: 26, left: 38 };

	/** Cumulative share at each rank, 1-based: `cum[r - 1]` is the top r. */
	$: cum = (() => {
		let run = 0;
		return counts.map((c) => (run += c) / Math.max(1, credits));
	})();
	$: n = counts.length;
	/** Thousands of ranks draw as a few hundred points; the curve is smooth. */
	$: stride = Math.max(1, Math.floor(n / 600));
	$: pts = cum
		.map((v, i) => [i + 1, v] as [number, number])
		.filter((_, i) => i % stride === 0 || i === n - 1);
	$: xTicks = (() => {
		const step = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000].find((s) => n / s <= 6) ?? 10000;
		const out = [1];
		for (let v = step; v <= n; v += step) out.push(v);
		return out;
	})();

	let hover: number | null = null;
	let tipX = 0;
	let wrapWidth = 0;
	function move(e: PointerEvent, invert: (px: number) => number) {
		const x = pointerX(e, pad.left);
		const r = Math.round(invert(x));
		hover = r >= 1 && r <= n ? r : null;
		tipX = x + pad.left;
	}
</script>

{#if n}
	<div class="wrap" style="height:{height}px;" bind:clientWidth={wrapWidth}>
		<LayerCake padding={pad} xScale={scaleLinear()} yScale={scaleLinear()}
			xDomain={[1, Math.max(2, n)]} yDomain={[0, 1]} data={[]}
			let:xScale let:yScale let:width let:height={h}>
			{@const ln = line<[number, number]>().x((d) => xScale(d[0])).y((d) => yScale(d[1]))}
			{@const ar = area<[number, number]>().x((d) => xScale(d[0])).y0(h).y1((d) => yScale(d[1]))}
			<Svg>
				{#each [0, 0.25, 0.5, 0.75, 1] as t}
					<line x1="0" x2={width} y1={yScale(t)} y2={yScale(t)} class="grid" />
					<text x="-6" y={yScale(t)} dy="0.32em" text-anchor="end" class="tick">{t * 100}%</text>
				{/each}
				<path d={ar(pts)} fill="var(--td-accent)" opacity="0.14" />
				<path d={ln(pts)} fill="none" stroke="var(--td-accent)" stroke-width="2" />
				{#each [[half, 0.5], [eighty, 0.8]] as [rank, share]}
					{#if rank > 0}
						{@const x = xScale(rank)}
						<line x1={x} x2={x} y1={yScale(share)} y2={h} class="mark" />
						<line x1="0" x2={x} y1={yScale(share)} y2={yScale(share)} class="mark" />
						<circle cx={x} cy={yScale(share)} r="4" fill="var(--td-accent)" stroke="var(--td-surface)" stroke-width="2" />
						<text x={x + 8} y={yScale(share) + 14} class="note">
							top {nf(rank)} = {share * 100}%
						</text>
					{/if}
				{/each}
				{#each xTicks as t}
					<text x={xScale(t)} y={h + 16} text-anchor="middle" class="tick">{nf(t)}</text>
				{/each}
				{#if hover !== null}
					<line x1={xScale(hover)} x2={xScale(hover)} y1="0" y2={h} class="cross" />
					<circle cx={xScale(hover)} cy={yScale(cum[hover - 1])} r="4" fill="var(--td-accent)" stroke="var(--td-surface)" stroke-width="2" />
				{/if}
				<rect x="0" y="0" {width} height={h} fill="transparent" role="presentation"
					on:pointermove={(e) => move(e, xScale.invert)} on:pointerleave={() => (hover = null)} />
			</Svg>
		</LayerCake>
		<ChartTip x={tipX} y={8} width={wrapWidth} show={hover !== null}>
			{#if hover !== null}
				Your top <strong>{nf(hover)}</strong> artist{hover === 1 ? '' : 's'}:
				<strong>{(cum[hover - 1] * 100).toFixed(1)}%</strong> of artist credits<br />
				<span class="sub">#{nf(hover)} was played {nf(counts[hover - 1])} time{counts[hover - 1] === 1 ? '' : 's'}</span>
			{/if}
		</ChartTip>
	</div>
	<p class="caption">Artists by rank, most-played first. A listen counts once for each artist credited on it.</p>
{/if}

<style>
	.wrap { position: relative; width: 100%; }
	.grid { stroke: var(--td-border); stroke-width: 1; }
	.mark { stroke: var(--td-muted-2); stroke-width: 1; stroke-dasharray: 3 3; }
	.cross { stroke: var(--td-muted-2); stroke-width: 1; }
	.tick { font-size: 11px; fill: var(--td-muted-3); font-family: inherit; }
	.note { font-size: 11.5px; fill: var(--td-text); font-weight: 600; font-family: inherit; }
	.sub { color: var(--td-muted-3); }
	.caption { font-size: 12px; color: var(--td-muted-3); margin: 8px 0 0; }
</style>
