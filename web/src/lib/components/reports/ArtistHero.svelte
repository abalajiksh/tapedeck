<script lang="ts">
	/**
	 * The top artist as a picture-sized tile, with their initial set enormous
	 * and faint behind the name.
	 *
	 * The design fills this with artwork, and it can now: an artist may carry a
	 * **landscape** image set by hand. Deliberately not the square portrait —
	 * this tile is 16/11, so a portrait either pillarboxes or crops the head
	 * off, and the two are different photographs rather than two crops.
	 *
	 * Nothing Tapedeck can reach supplies a landscape, so most artists have
	 * none, and the enormous faint initial stays the design for those — not a
	 * placeholder waiting for an image that will never arrive. Same reasoning as
	 * the lettered cover tile everywhere else.
	 */
	import { openArtist } from '$lib/entities';
	import { nf } from './bits';

	export let artist: {
		name: string;
		plays: number;
		hours: number;
		landscape_url?: string | null;
	};
	export let label = 'Top artist';
	export let ratio = '16/11';
	export let initialSize = 150;

	$: initial = (artist.name || '?').trim().charAt(0).toUpperCase();
</script>

<button class="hero" class:photo={!!artist.landscape_url} style="aspect-ratio:{ratio};"
	on:click={() => openArtist(artist.name)}>
	{#if artist.landscape_url}
		<img class="shot" src={artist.landscape_url} alt="" loading="lazy" />
		<!-- The name and the plays sit on top of it, so the bottom of the image
		     has to darken or the text lands on whatever happens to be there. -->
		<span class="scrim" aria-hidden="true"></span>
	{:else}
		<span class="ini h-display" style="font-size:{initialSize}px;" aria-hidden="true">{initial}</span>
	{/if}
	<span class="pill">
		<svg viewBox="0 0 24 24" width="13" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"><path d="M12 15a3 3 0 0 0 3-3V6a3 3 0 0 0-6 0v6a3 3 0 0 0 3 3z"/><path d="M6 11v1a6 6 0 0 0 12 0v-1M12 18v3"/></svg>
		{label}
	</span>
	<span class="foot">
		<span class="h-display name">{artist.name}</span>
		<span class="sub">{nf(artist.plays)} plays · {artist.hours}h</span>
	</span>
</button>

<style>
	.hero {
		position: relative; width: 100%; overflow: hidden; border: none;
		border-radius: 18px; cursor: pointer; padding: 0; font: inherit;
		background: linear-gradient(150deg, var(--td-panel), var(--td-panel2));
		display: flex; align-items: center; justify-content: center;
	}
	.hero:hover .name { color: #ffd7b0; }
	.shot {
		position: absolute; inset: 0; width: 100%; height: 100%;
		object-fit: cover; display: block;
	}
	.scrim {
		position: absolute; inset: 0;
		background: linear-gradient(to top, rgba(15, 11, 7, 0.85) 0%, rgba(15, 11, 7, 0.15) 45%, transparent 70%);
	}
	/* The gradient background is only visible behind the initial; with a photo
	   it would tint the image at the edges. */
	.hero.photo { background: #0f0b07; }
	.ini { color: rgba(244, 231, 207, 0.09); line-height: 1; user-select: none; }
	.pill {
		position: absolute; top: 14px; left: 14px;
		display: inline-flex; align-items: center; gap: 7px;
		background: rgba(15, 11, 7, 0.55); border-radius: 999px;
		padding: 5px 12px; color: #f4e7cf; font-size: 11px; font-weight: 600;
	}
	.foot {
		position: absolute; left: 16px; right: 16px; bottom: 14px;
		color: #f4e7cf; text-align: left; min-width: 0;
	}
	.name {
		display: block; font-size: 24px; line-height: 1.1;
		overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
	}
	.sub { display: block; font-size: 12.5px; color: #c9a678; margin-top: 2px; }
</style>
