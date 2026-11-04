<script lang="ts">
	import { openArtist } from '$lib/entities';
	import { creditedNames } from '$lib/credits';

	/** The primary, or the only artist. */
	export let artist = '';
	/** The server's `artist_credit` (`"; "`-joined, primary first). */
	export let credit: string | null | undefined = null;
	/** A list already resolved by the server (an album's `credited`); wins when set. */
	export let names: string[] | null = null;
	/** Show at most this many and summarise the rest as "+N"; 0 shows everyone. */
	export let max = 0;
	/** Inside a row that is itself a button, where a button may not nest: the
	 *  names are still clickable, but stay out of the tab order — the row is
	 *  what the keyboard opens. */
	export let nested = false;

	$: all = names?.length ? names : creditedNames(artist, credit);
	$: shown = max > 0 ? all.slice(0, max) : all;
</script>

<!-- One link per credited artist. Buttons rather than anchors: a name has to be
     resolved to an entity id (and possibly created) before there is a URL. -->
{#each shown as name, i}{#if i}<span aria-hidden="true">{', '}</span>{/if}{#if nested}<span
			class="xref"
			role="link"
			tabindex="-1"
			on:click|stopPropagation={() => openArtist(name)}
			on:keydown|stopPropagation={(e) => e.key === 'Enter' && openArtist(name)}>{name}</span>{:else}<button
			type="button"
			class="xref"
			on:click|stopPropagation={() => openArtist(name)}>{name}</button>{/if}{/each}{#if shown.length < all.length}<span
		class="more"
		title={all.slice(shown.length).join(', ')}>{` +${all.length - shown.length}`}</span>{/if}

<style>
	/* Reads as the secondary line it sits in until you point at it. */
	.xref {
		border: none;
		background: transparent;
		padding: 0;
		cursor: pointer;
		font: inherit;
		color: inherit;
	}
	.xref:hover { color: var(--td-accent); text-decoration: underline; }
	.more { white-space: nowrap; }
</style>
