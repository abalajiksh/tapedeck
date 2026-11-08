<script lang="ts">
	/**
	 * Every listen of this artist added up over time, with the day your rate
	 * of listening to them stepped up marked on it.
	 *
	 * The step is the server's single change point (`dossier::step_up`): the
	 * day where "one rate before, a higher one after" fits the listening best.
	 * The label states both rates, so the mark explains itself.
	 */
	import { LayerCake, Svg } from 'layercake';
	import { scaleLinear, scaleUtc } from 'd3-scale';
	import { area, line, curveStepAfter } from 'd3-shape';
	import ChartTip from './ChartTip.svelte';
	import { fmtDate, nf, parseDay, pointerX, valueTicks } from './graphs';

	/** The days they were played, oldest first. */
	export let daily: { date: string; plays: number }[] = [];
	/** The last day of the history, so the line runs on to it. */
	export let end: string | null = null;
	export let step: { date: string; before_per_week: number; after_per_week: number } | undefined = undefined;
	export let height = 220;

	const pad = { top: 26, right: 12, bottom: 24, left: 40 };

	$: points = (() => {
		let run = 0;
		const out = daily.map((d) => ({ t: parseDay(d.date), v: (run += d.plays) }));
		if (end && out.length && parseDay(end) > out[out.length - 1].t) out.push({ t: parseDay(end), v: run });
		return out;
	})();
	$: total = points.length ? points[points.length - 1].v : 0;
	$: x0 = points[0]?.t ?? new Date();
	$: x1 = points[points.length - 1]?.t ?? new Date();
	$: yTicks = valueTicks(total);
	$: stepAt = step ? parseDay(step.date) : null;
	/** Listens up to a date — the last point at or before it. */
	function at(t: Date): number {
		let v = 0;
		for (const p of points) {
			if (p.t > t) break;
			v = p.v;
		}
		return v;
	}

	let hover: Date | null = null;
	let tipX = 0;
	let wrapWidth = 0;
	function move(e: PointerEvent, invert: (px: number) => Date) {
		const x = pointerX(e, pad.left);
		const t = invert(x);
		hover = t >= x0 && t <= x1 ? t : null;
		tipX = x + pad.left;
	}
	const fmt = (n: number) => n.toLocaleString(undefined, { maximumFractionDigits: 1 });
</script>

{#if points.length > 1}
	<div class="wrap" style="height:{height}px;" bind:clientWidth={wrapWidth}>
		<LayerCake padding={pad} xScale={scaleUtc()} yScale={scaleLinear()}
			xDomain={[x0.getTime(), x1.getTime()]} yDomain={[0, Math.max(1, total)]} data={[]}
			let:xScale let:yScale let:width let:height={h}>
			{@const ln = line<{ t: Date; v: number }>().x((p) => xScale(p.t)).y((p) => yScale(p.v)).curve(curveStepAfter)}
			{@const ar = area<{ t: Date; v: number }>().x((p) => xScale(p.t)).y0(h).y1((p) => yScale(p.v)).curve(curveStepAfter)}
			<Svg>
				{#each yTicks as t}
					<line x1="0" x2={width} y1={yScale(t)} y2={yScale(t)} class="grid" />
					<text x="-6" y={yScale(t)} dy="0.32em" text-anchor="end" class="tick">{nf(t)}</text>
				{/each}
				<path d={ar(points)} fill="var(--td-accent)" opacity="0.14" />
				<path d={ln(points)} fill="none" stroke="var(--td-accent)" stroke-width="2" />
				{#if step && stepAt}
					{@const sx = xScale(stepAt)}
					<line x1={sx} x2={sx} y1={-14} y2={h} class="step" />
					<text x={sx > width * 0.6 ? sx - 6 : sx + 6} y={-4} text-anchor={sx > width * 0.6 ? 'end' : 'start'} class="note">
						Stepped up {fmtDate(stepAt)} · {fmt(step.before_per_week)} → {fmt(step.after_per_week)} a week
					</text>
				{/if}
				{#each xScale.ticks(Math.max(2, Math.floor(width / 110))) as t}
					<text x={xScale(t)} y={h + 16} text-anchor="middle" class="tick">
						{t.toLocaleDateString(undefined, { timeZone: 'UTC', month: 'short', year: '2-digit' })}
					</text>
				{/each}
				{#if hover}
					<line x1={xScale(hover)} x2={xScale(hover)} y1="0" y2={h} class="cross" />
					<circle cx={xScale(hover)} cy={yScale(at(hover))} r="4" fill="var(--td-accent)" stroke="var(--td-surface)" stroke-width="2" />
				{/if}
				<rect x="0" y="0" {width} height={h} fill="transparent" role="presentation"
					on:pointermove={(e) => move(e, xScale.invert)} on:pointerleave={() => (hover = null)} />
			</Svg>
		</LayerCake>
		<ChartTip x={tipX} y={8} width={wrapWidth} show={hover !== null}>
			{#if hover}
				<strong>{fmtDate(hover)}</strong><br />
				{nf(at(hover))} listens so far
			{/if}
		</ChartTip>
	</div>
	<p class="caption">
		{#if step}
			The marked day is where your listening to them stepped up most: the split where one rate before
			and a higher one after fits every day best.
		{:else}
			No single day stands out as a step up — the listening grew steadily, or came early and eased off.
		{/if}
	</p>
{/if}

<style>
	.wrap { position: relative; width: 100%; }
	.grid { stroke: var(--td-border); stroke-width: 1; }
	.cross { stroke: var(--td-muted-2); stroke-width: 1; }
	.step { stroke: var(--td-text); stroke-width: 1; stroke-dasharray: 4 3; opacity: 0.6; }
	.tick { font-size: 11px; fill: var(--td-muted-3); font-family: inherit; }
	.note { font-size: 11.5px; fill: var(--td-text); font-weight: 600; font-family: inherit; }
	.caption { font-size: 12px; color: var(--td-muted-3); margin: 8px 0 0; }
</style>
