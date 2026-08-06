<script lang="ts">
	/**
	 * Twenty-four hours as a dial.
	 *
	 * A clock, not a bar chart, because the data is genuinely cyclic: 23:00 and
	 * 00:00 are adjacent, and a bar chart puts them at opposite ends of the
	 * page. That is the one thing the Statistics heatmap cannot show — it reads
	 * across days, this reads around one.
	 *
	 * Hand-built arcs rather than LayerCake: there is no scale to compute beyond
	 * "hour to angle", which is a constant, and no axis to lay out.
	 */
	export let hours: number[] = [];
	export let size = 190;
	/** A comparison ring, drawn faintly inside. */
	export let baseline: number[] | null = null;
	export let baselineLabel = 'Previous';

	const R_OUT = 46;
	const R_IN = 19;

	$: max = Math.max(1, ...hours, ...(baseline ?? []));
	$: peak = hours.length ? hours.indexOf(Math.max(...hours)) : 0;

	/** An hour's wedge, as an SVG path. Radius carries the value. */
	function wedge(i: number, v: number, cap: number, rIn: number, rOutMax: number) {
		const a0 = ((i - 0.5) / 24) * Math.PI * 2 - Math.PI / 2;
		const a1 = ((i + 0.5) / 24) * Math.PI * 2 - Math.PI / 2;
		// sqrt so *area* carries the count — the same reasoning the genre map's
		// radii use. Raw radius exaggerates a busy hour by its square.
		const rOut = rIn + (rOutMax - rIn) * Math.sqrt(v / cap);
		const p = (r: number, a: number) => `${(50 + r * Math.cos(a)).toFixed(2)},${(50 + r * Math.sin(a)).toFixed(2)}`;
		return `M ${p(rIn, a0)} L ${p(rOut, a0)} A ${rOut} ${rOut} 0 0 1 ${p(rOut, a1)} L ${p(rIn, a1)} A ${rIn} ${rIn} 0 0 0 ${p(rIn, a0)} Z`;
	}
</script>

<div class="clock" style="width:{size}px;">
	<svg viewBox="0 0 100 100" role="img" aria-label="Listening by hour of day">
		<circle cx="50" cy="50" r={R_IN} fill="none" stroke="var(--td-border)" stroke-width="0.6" />
		{#if baseline}
			{#each baseline as v, i}
				<path d={wedge(i, v, max, R_IN, R_OUT)} fill="var(--td-track)" opacity="0.7" />
			{/each}
		{/if}
		{#each hours as v, i}
			<path d={wedge(i, v, max, R_IN, R_OUT)} fill="var(--td-accent)" opacity={i === peak ? 1 : 0.78}>
				<title>{String(i).padStart(2, '0')}:00 — {v.toLocaleString()} plays</title>
			</path>
		{/each}
		{#each [0, 6, 12, 18] as h}
			{@const a = (h / 24) * Math.PI * 2 - Math.PI / 2}
			<text
				x={50 + (R_OUT + 6) * Math.cos(a)}
				y={50 + (R_OUT + 6) * Math.sin(a)}
				font-size="5"
				fill="var(--td-muted-3)"
				text-anchor="middle"
				dominant-baseline="middle">{String(h).padStart(2, '0')}</text
			>
		{/each}
	</svg>
	{#if baseline}
		<div class="legend">
			<span><i style="background:var(--td-accent);"></i>This period</span>
			<span><i style="background:var(--td-track);"></i>{baselineLabel}</span>
		</div>
	{/if}
</div>

<style>
	.clock svg {
		width: 100%;
		height: auto;
		display: block;
		overflow: visible;
	}
	.legend {
		display: flex;
		gap: 12px;
		margin-top: 10px;
		font-size: 11.5px;
		color: var(--td-muted-2);
		justify-content: center;
		flex-wrap: wrap;
	}
	.legend span {
		display: inline-flex;
		align-items: center;
		gap: 6px;
	}
	.legend i {
		width: 10px;
		height: 10px;
		border-radius: 3px;
		display: inline-block;
	}
</style>
