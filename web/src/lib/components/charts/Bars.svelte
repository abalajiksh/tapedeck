<script lang="ts">
	/**
	 * The marks for `PeriodBars`. Split out because this is how LayerCake works:
	 * the parent computes the scales and puts them in context, and a mark
	 * component reads them. Keeping the marks separate is what lets them stay
	 * ordinary SVG styled with `--td-*` variables rather than chart config.
	 */
	import { getContext } from 'svelte';

	/** Which field to draw, so one component serves both series. */
	export let key: 'value' | 'prev' = 'value';
	export let fill = 'var(--td-accent)';
	/** 0–1 of the band. The previous series is drawn wider and behind. */
	export let widthFactor = 0.5;

	const { data, xGet, yGet, xScale, yScale, height } = getContext<any>('LayerCake');

	$: band = $xScale.bandwidth ? $xScale.bandwidth() : 10;
	$: w = band * widthFactor;
	$: inset = (band - w) / 2;
</script>

{#each $data as d}
	{@const x = $xGet(d) + inset}
	{@const y = $yScale(d[key] ?? 0)}
	<rect {x} y={Math.min(y, $height)} width={Math.max(w, 1)} height={Math.max($height - y, 0)} {fill} rx="3">
		<title>{d.label}: {(d[key] ?? 0).toLocaleString()} plays</title>
	</rect>
{/each}
