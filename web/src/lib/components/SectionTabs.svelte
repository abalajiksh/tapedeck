<script lang="ts">
	import { afterUpdate } from 'svelte';

	// The tab strip shown at the top of a grouped page. The sidebar has one
	// entry per group; this is how you move between the pages inside it.
	// The pages keep their own URLs, so every existing link and bookmark to
	// /charts or /map still lands where it did — the grouping is navigation,
	// not a route change.
	export let label: string;
	export let tabs: { href: string; label: string }[];
	export let active: string;

	// On a phone the strip scrolls, and landing on its last tab would leave the
	// one you are on out of sight. Only the strip moves; the page stays put.
	let strip: HTMLDivElement;
	afterUpdate(() => {
		const on = strip?.querySelector<HTMLElement>('.tab.on');
		if (!on) return;
		const s = strip.getBoundingClientRect(), t = on.getBoundingClientRect();
		if (t.left < s.left) strip.scrollLeft += t.left - s.left - 8;
		else if (t.right > s.right) strip.scrollLeft += t.right - s.right + 8;
	});
</script>

<nav class="section-tabs" aria-label={label}>
	<span class="eyebrow">{label}</span>
	<div class="strip" role="tablist" bind:this={strip}>
		{#each tabs as t}
			<a
				href={t.href}
				role="tab"
				aria-selected={active === t.href}
				class="tab"
				class:on={active === t.href}
			>{t.label}</a>
		{/each}
	</div>
</nav>

<style>
	.section-tabs {
		display: flex;
		align-items: center;
		gap: 16px;
		margin: -6px 0 26px;
		padding-bottom: 12px;
		border-bottom: 1px solid var(--td-border);
	}
	.eyebrow {
		flex: none;
		font-size: 11px;
		letter-spacing: 0.16em;
		text-transform: uppercase;
		color: var(--td-accent);
		font-weight: 600;
	}
	/* Scrolls sideways rather than wrapping: four tabs on a phone would
	   otherwise stack into a block that pushes the page below the fold. */
	.strip {
		display: flex;
		gap: 4px;
		overflow-x: auto;
		scrollbar-width: none;
		min-width: 0;
	}
	.strip::-webkit-scrollbar {
		display: none;
	}
	.tab {
		flex: none;
		padding: 7px 15px;
		border-radius: 999px;
		font-size: 13.5px;
		font-weight: 600;
		color: var(--td-nav-fg);
		text-decoration: none;
		white-space: nowrap;
	}
	.tab:hover {
		background: var(--td-nav-hover);
		color: var(--td-text);
	}
	.tab.on {
		background: var(--td-panel);
		color: #f4e7cf;
	}
	@media (max-width: 600px) {
		.eyebrow {
			display: none;
		}
	}
</style>
