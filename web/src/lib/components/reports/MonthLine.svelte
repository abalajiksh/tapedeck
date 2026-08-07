<script lang="ts">
	/**
	 * Scrobbles by month across a year, with the prior year dashed behind it.
	 *
	 * Hand-built rather than run through LayerCake, for the same reason
	 * `ListeningClock` is: there is one scale here and it is linear in both
	 * directions. LayerCake earns its place where the scales are the hard part
	 * (see `PeriodBars`); here it would be a dependency wrapping two `map`s.
	 *
	 * Both series share one domain — drawing them against separate maxima would
	 * make a worse year look identical to a better one.
	 */
	export let values: number[] = [];
	export let previous: number[] | null = null;
	export let labels: string[] = [];

	const W = 640;
	const H = 210;
	const PAD_X = 22;
	const TOP = 14;
	const BASE = 182;

	$: max = Math.max(1, ...values, ...(previous ?? []));
	$: x = (i: number) =>
		values.length > 1 ? PAD_X + (i / (values.length - 1)) * (W - PAD_X * 2) : W / 2;
	$: y = (v: number) => BASE - (v / max) * (BASE - TOP);

	const pts = (vals: number[], fx: (i: number) => number, fy: (v: number) => number) =>
		vals.map((v, i) => `${fx(i).toFixed(1)},${fy(v).toFixed(1)}`).join(' ');

	$: linePts = pts(values, x, y);
	$: prevPts = previous?.length ? pts(previous, x, y) : '';
	$: areaPath = values.length
		? `M${x(0).toFixed(1)},${BASE} L${linePts.split(' ').join(' L')} L${x(values.length - 1).toFixed(1)},${BASE} Z`
		: '';
</script>

<svg viewBox="0 0 {W} {H}" style="width:100%;height:auto;display:block;" role="img"
	aria-label="Scrobbles by month">
	<defs>
		<linearGradient id="rp-area" x1="0" y1="0" x2="0" y2="1">
			<stop offset="0" stop-color="var(--td-accent)" stop-opacity="0.26" />
			<stop offset="1" stop-color="var(--td-accent)" stop-opacity="0" />
		</linearGradient>
	</defs>
	<line x1={PAD_X} y1={BASE} x2={W - PAD_X} y2={BASE} stroke="var(--td-border)" stroke-width="1" />
	{#if areaPath}<path d={areaPath} fill="url(#rp-area)" />{/if}
	{#if prevPts}
		<polyline points={prevPts} fill="none" stroke="var(--td-muted-3)" stroke-width="2"
			stroke-dasharray="5 5" opacity="0.7" />
	{/if}
	<polyline points={linePts} fill="none" stroke="var(--td-accent)" stroke-width="3"
		stroke-linejoin="round" stroke-linecap="round" />
	{#each values as v, i}
		<circle cx={x(i)} cy={y(v)} r="3.5" fill="var(--td-surface)"
			stroke="var(--td-accent)" stroke-width="2" />
	{/each}
	{#each labels as l, i}
		<text x={x(i)} y={H - 4} text-anchor="middle" font-size="11"
			fill="var(--td-muted-3)" font-family="inherit">{l}</text>
	{/each}
</svg>
