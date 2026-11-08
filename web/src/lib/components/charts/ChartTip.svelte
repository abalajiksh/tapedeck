<script lang="ts">
	/**
	 * The hover card every Statistics graph shares.
	 *
	 * Positioned in the chart wrapper's own pixels; flips to the left of the
	 * pointer in the right-hand part of the chart so it never runs off the
	 * panel. `pointer-events: none` so it can't steal the hover it's showing.
	 */
	export let x = 0;
	export let y = 0;
	/** The wrapper's width, to decide which side to open on. */
	export let width = 0;
	export let show = false;

	$: flip = width > 0 && x > width * 0.6;
</script>

{#if show}
	<div class="tip" style="top:{Math.max(0, y)}px;{flip ? `right:${width - x + 12}px` : `left:${x + 12}px`};">
		<slot />
	</div>
{/if}

<style>
	.tip {
		position: absolute;
		z-index: 5;
		pointer-events: none;
		background: var(--td-surface);
		color: var(--td-text);
		border: 1px solid var(--td-border);
		border-radius: 10px;
		padding: 8px 10px;
		font-size: 12.5px;
		line-height: 1.45;
		box-shadow: 0 6px 20px rgba(0, 0, 0, 0.14);
		max-width: 280px;
		white-space: normal;
	}
</style>
