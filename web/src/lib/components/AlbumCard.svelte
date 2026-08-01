<script lang="ts">
	import type { AlbumAggregate } from '$lib/api';
	import { openAlbum } from '$lib/entities';
	import CoverArt from './CoverArt.svelte';

	export let album: AlbumAggregate;
	/** Needed to resolve the album — the same title exists under many artists. */
	export let artist: string;
</script>

<button class="card" on:click={() => openAlbum(artist, album.name)}>
	<div class="art">
		<CoverArt
			url={album.artwork_url}
			caaId={album.caa_id}
			releaseMbid={album.caa_release_mbid}
			seed={album.name}
			size={150}
			radius={16}
		/>
	</div>
	<div class="title">{album.name}</div>
	<div class="meta">{album.year ? `${album.year} · ` : ''}{album.plays} plays</div>
</button>

<style>
	.card {
		border: none;
		background: transparent;
		padding: 0;
		cursor: pointer;
		text-align: left;
		color: inherit;
		font: inherit;
		display: block;
		width: 100%;
	}
	/* The cover is square and fills the column; CoverArt takes a pixel size, so
	   the wrapper does the responsive part and the image stretches to it. */
	.art {
		aspect-ratio: 1;
		margin-bottom: 9px;
		border-radius: 16px;
		overflow: hidden;
		background: var(--td-track);
	}
	.art :global(img),
	.art :global(div) {
		width: 100% !important;
		height: 100% !important;
		border-radius: 16px !important;
	}
	.card:hover .title { color: var(--td-accent); }
	.title {
		font-size: 14px;
		font-weight: 600;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		transition: color .15s;
	}
	.meta {
		font-size: 12px;
		color: var(--td-muted-2);
		margin-top: 2px;
	}
</style>
