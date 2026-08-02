<script lang="ts">
	// Back to top: a long list is thousands of rows deep, and the browser's own
	// Home key doesn't work once focus is inside the page.
	export let after = 900;

	let scrolled = false;
	function onScroll() {
		scrolled = window.scrollY > after;
	}
	function toTop() {
		const still = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
		window.scrollTo({ top: 0, behavior: still ? 'auto' : 'smooth' });
	}
</script>

<svelte:window on:scroll={onScroll} />

{#if scrolled}
	<button class="to-top" on:click={toTop} title="Back to top" aria-label="Back to top">
		<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
			<path d="M12 19V5"/><path d="M5 12l7-7 7 7"/>
		</svg>
	</button>
{/if}

<style>
	.to-top {
		position: fixed;
		right: 26px;
		bottom: 26px;
		z-index: 40;
		width: 44px;
		height: 44px;
		border-radius: 50%;
		border: 1px solid var(--td-border);
		background: var(--td-surface);
		color: var(--td-text);
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		box-shadow: 0 8px 24px rgba(46, 43, 37, .18);
		animation: to-top-in .18s ease-out;
	}
	.to-top:hover { background: var(--td-nav-hover); }
	@keyframes to-top-in {
		from { opacity: 0; transform: translateY(8px); }
		to { opacity: 1; transform: none; }
	}
	/* Someone who asked for less motion gets the button, not the entrance. */
	@media (prefers-reduced-motion: reduce) {
		.to-top { animation: none; }
	}
</style>
