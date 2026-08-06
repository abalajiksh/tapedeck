<script lang="ts">
	/**
	 * Plays per sub-bucket for one period, with the previous period behind it.
	 *
	 * The first chart on the LayerCake seam. LayerCake is a layout engine rather
	 * than a charting library — it computes the scales and hands them to your
	 * own markup — which is exactly why it was chosen here: every colour stays a
	 * `--td-*` custom property, so charts theme with the rest of the app instead
	 * of carrying a second palette that has to be kept in sync.
	 */
	import { LayerCake, Svg } from 'layercake';
	import { scaleBand, scaleLinear } from 'd3-scale';
	import Bars from './Bars.svelte';

	export let values: number[] = [];
	export let labels: string[] = [];
	/** The same buckets a period earlier, drawn behind. Omit for one series. */
	export let previous: number[] | null = null;
	export let height = 160;
	export let currentLabel = 'This period';
	export let previousLabel = 'Previous';

	$: data = labels.map((label, i) => ({
		label,
		value: values[i] ?? 0,
		prev: previous?.[i] ?? 0
	}));
	/** Both series share one domain, or the comparison is a lie. */
	$: max = Math.max(1, ...data.map((d) => Math.max(d.value, d.prev)));
</script>

{#if data.length}
	<div class="wrap" style="height:{height}px;">
		<LayerCake
			padding={{ top: 6, right: 0, bottom: 22, left: 0 }}
			x="label"
			y="value"
			xScale={scaleBand().paddingInner(0.3).round(true)}
			yScale={scaleLinear()}
			xDomain={labels}
			yDomain={[0, max]}
			{data}
		>
			<Svg>
				<!-- Previous sits *behind* rather than beside: two full bars per
				     bucket doubles the ink and halves the width, and the question
				     is "more or less than last time", not "rank these two". -->
				{#if previous}
					<Bars key="prev" fill="var(--td-track)" widthFactor={0.92} />
				{/if}
				<Bars key="value" fill="var(--td-accent)" widthFactor={0.52} />
			</Svg>
		</LayerCake>
		<!-- Labels share the plot's column geometry so they cannot drift apart as
		     the panel widens — the mistake the listening heatmap made once. -->
		<div class="axis">
			{#each data as d}<span>{d.label}</span>{/each}
		</div>
	</div>
	{#if previous}
		<div class="legend">
			<span><i style="background:var(--td-accent);"></i>{currentLabel}</span>
			<span><i style="background:var(--td-track);"></i>{previousLabel}</span>
		</div>
	{/if}
{/if}

<style>
	.wrap {
		position: relative;
	}
	.axis {
		position: absolute;
		bottom: 0;
		left: 0;
		right: 0;
		display: flex;
		font-size: 10.5px;
		color: var(--td-muted-3);
	}
	.axis span {
		flex: 1;
		text-align: center;
		overflow: hidden;
		white-space: nowrap;
	}
	.legend {
		display: flex;
		gap: 14px;
		margin-top: 8px;
		font-size: 11.5px;
		color: var(--td-muted-2);
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
