<script lang="ts">
	/**
	 * The top artist as a picture-sized tile, with their initial set enormous
	 * and faint behind the name.
	 *
	 * The design fills this with artwork. Tapedeck does have artist portraits,
	 * but only for artists it found in the user's own library — most rows have
	 * none — so the initial is the *design*, not a placeholder waiting for an
	 * image. Same reasoning as the lettered cover tile everywhere else.
	 */
	import { openArtist } from '$lib/entities';
	import { nf } from './bits';

	export let artist: { name: string; plays: number; hours: number };
	export let label = 'Top artist';
	export let ratio = '16/11';
	export let initialSize = 150;

	$: initial = (artist.name || '?').trim().charAt(0).toUpperCase();
</script>

<button class="hero" style="aspect-ratio:{ratio};" on:click={() => openArtist(artist.name)}>
	<span class="ini h-display" style="font-size:{initialSize}px;" aria-hidden="true">{initial}</span>
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
