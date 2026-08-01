<script lang="ts">
	/**
	 * The vertical bar chart used for "by decade" on Statistics and "plays by
	 * year" on an artist page.
	 *
	 * Shared rather than written twice on purpose: the two were built separately
	 * and drifted into different bar widths, colours and label placement, which
	 * made the same kind of fact look like two different kinds of chart.
	 */
	export let bars: { label: string; value: number }[] = [];
	/** Minimum width per bar before the panel starts scrolling sideways. */
	export let minBarWidth = 60;

	$: max = Math.max(1, ...bars.map((b) => b.value));
</script>

<div class="panel wrap">
	<div class="rail" style="min-width:{bars.length * minBarWidth}px;">
		{#each bars as b}
			<div class="col">
				<span class="val">{b.value}</span>
				<div class="track">
					<div class="fill" style="height:{Math.max(4, (b.value / max) * 100)}%"></div>
				</div>
				<span class="label">{b.label}</span>
			</div>
		{/each}
	</div>
</div>

<style>
	.wrap {
		border-radius: 22px;
		padding: 22px;
		margin-bottom: 16px;
		overflow-x: auto;
	}
	/* Right-aligned, so the newest period sits at the right edge and history
	   grows leftwards. Three years of listening should read as three bars at the
	   end of a timeline, not three slabs stretched across the panel — hence the
	   cap on column width as well. */
	.rail {
		display: flex;
		align-items: flex-end;
		justify-content: flex-end;
		gap: 16px;
		height: 150px;
	}
	.col {
		flex: 1;
		max-width: 92px;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: flex-end;
		height: 100%;
		gap: 8px;
	}
	.val { font-size: 12px; color: var(--td-muted-2); }
	.track {
		width: 100%;
		background: var(--td-track);
		border-radius: 12px;
		overflow: hidden;
		display: flex;
		align-items: flex-end;
		height: 100%;
	}
	.fill {
		width: 100%;
		background: linear-gradient(180deg, #e07a55, #c67139);
		border-radius: 12px;
	}
	.label { font-size: 12.5px; font-weight: 600; }
</style>
