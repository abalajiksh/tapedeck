<script lang="ts">
	/**
	 * The glyph on a Rediscovery card.
	 *
	 * Every card used to carry the same circular-arrow mark, which reads as
	 * *reload* — someone clicked one expecting a picture to re-fetch. Nothing on
	 * this page reloads anything, so the arrow is gone entirely and each
	 * collection gets a glyph for what it actually is: a clock for what you
	 * haven't touched, a moon for the small hours, a record for the shelf.
	 *
	 * Drawn in `currentColor` like `SourceIcon` and `ChainIcon`, so the card's
	 * accent tints it and neither theme needs a second copy. An unknown key
	 * falls back to a note rather than erroring — same rule the chain icons
	 * follow, since the server is free to add a collection the UI hasn't met.
	 */
	export let name: string;
	export let size = 22;
</script>

<svg viewBox="0 0 24 24" width={size} height={size} fill="none" stroke="currentColor"
	stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
	{#if name === 'forgotten'}
		<!-- A clock: these are tracks measured by how long it's been. -->
		<circle cx="12" cy="12" r="9" /><path d="M12 7v5l3.5 2" />
	{:else if name === 'highrated'}
		<path d="M20.8 6.6a5 5 0 0 0-7.3-.5L12 7.6l-1.5-1.5a5 5 0 0 0-7.3 6.8L12 21l8.8-8.1a5 5 0 0 0 0-6.3z" />
	{:else if name === 'onelisten'}
		<!-- A star for the masterpiece half, not a counter — the "once" is in
		     the title and the stat line. -->
		<path d="M12 3.5l2.7 5.5 6 .9-4.3 4.2 1 6-5.4-2.8-5.4 2.8 1-6L3.3 9.9l6-.9z" />
	{:else if name === 'seasonal'}
		<rect x="3.5" y="5" width="17" height="15.5" rx="3" />
		<path d="M8 2.8v4.4M16 2.8v4.4M3.5 10.5h17" />
	{:else if name === 'latenight'}
		<path d="M20.5 14.3A8.6 8.6 0 0 1 9.7 3.5a8.6 8.6 0 1 0 10.8 10.8z" />
	{:else if name === 'deepcuts'}
		<!-- A tracklist with a later line picked out: deep cuts are the ones
		     further down the record. -->
		<path d="M4 6.5h16M4 11h16M4 15.5h9" />
		<circle cx="18" cy="18.4" r="2.6" />
	{:else if name === 'dusty'}
		<circle cx="12" cy="12" r="9" /><circle cx="12" cy="12" r="3" />
	{:else}
		<path d="M9 18V6l10-2v12" /><circle cx="6.5" cy="18" r="2.6" /><circle cx="16.5" cy="16" r="2.6" />
	{/if}
</svg>
