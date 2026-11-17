<script lang="ts">
	/**
	 * A segmented control or a row of chips — the two controls every Insights
	 * tab is built from, styled once. `chips` draws outlined chips (what to
	 * rank), the default draws the pill group (how and when).
	 */
	import { createEventDispatcher } from 'svelte';

	export let options: { k: string; label: string; disabled?: boolean; hint?: string }[] = [];
	export let value: string;
	export let chips = false;
	export let small = false;

	const dispatch = createEventDispatcher<{ change: string }>();
	function pick(k: string) {
		if (k === value) return;
		value = k;
		dispatch('change', k);
	}
</script>

{#if chips}
	<div class="chips" role="group">
		{#each options as o}
			<button class="chip" class:on={value === o.k} class:small disabled={o.disabled} title={o.hint ?? ''}
				on:click={() => pick(o.k)}>{o.label}</button>
		{/each}
	</div>
{:else}
	<div class="seg" role="group">
		{#each options as o}
			<button class="seg-btn" class:on={value === o.k} disabled={o.disabled} title={o.hint ?? ''}
				on:click={() => pick(o.k)}>{o.label}</button>
		{/each}
	</div>
{/if}

<style>
	.seg { display: inline-flex; flex-wrap: wrap; background: var(--td-sidebar); border-radius: 999px; padding: 3px; gap: 2px; }
	.seg-btn {
		border: none; background: transparent; cursor: pointer; color: var(--td-nav-fg);
		font-size: 12.5px; padding: 6px 14px; border-radius: 999px; font-family: inherit;
	}
	.seg-btn.on { background: var(--td-accent); color: var(--td-on-accent); }
	.seg-btn:disabled { opacity: 0.4; cursor: default; }
	.chips { display: flex; gap: 8px; flex-wrap: wrap; }
	.chip {
		border: 1px solid var(--td-border); background: var(--td-surface); color: var(--td-nav-fg);
		font: inherit; font-size: 13px; font-weight: 600; padding: 8px 18px; border-radius: 999px; cursor: pointer;
	}
	.chip.small { font-size: 12px; padding: 5px 12px; font-weight: 500; }
	.chip:hover { border-color: var(--td-accent); }
	.chip.on { background: var(--td-panel); color: var(--td-on-accent); border-color: transparent; }
	.chip:disabled { opacity: 0.4; cursor: default; }
</style>
