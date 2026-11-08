<script lang="ts">
	/**
	 * How each top artist's plays split across their records: a catalogue
	 * explorer is a bar of many slices, a one-album fan is one long one.
	 *
	 * Slices are shaded by rank within the artist — most-played darkest — since
	 * the records differ from row to row and only their order is comparable.
	 * Every other record folds into one grey slice, and listens whose source
	 * named no record at all are hatched rather than hidden, so each bar adds
	 * up to everything that artist was played.
	 */
	import { nf } from './graphs';
	import { openAlbum, openArtist } from '$lib/entities';

	export let artists: {
		name: string;
		plays: number;
		album_count: number;
		albums: { name: string; album_artist: string; plays: number }[];
		rest_plays: number;
		no_album: number;
	}[] = [];

	const SHADES = [100, 78, 60, 44, 30];
	const share = (n: number, of: number) => (of > 0 ? (n / of) * 100 : 0);
</script>

{#if artists.length}
	<div class="rows">
		{#each artists as a}
			<div class="row">
				<button class="who" on:click={() => openArtist(a.name)} title="Open {a.name}">
					<span class="nm">{a.name}</span>
					<span class="sub">{nf(a.plays)} plays · {nf(a.album_count)} record{a.album_count === 1 ? '' : 's'}</span>
				</button>
				<div class="bar">
					{#each a.albums as al, i}
						{@const p = share(al.plays, a.plays)}
						<button class="seg" style="flex:{al.plays};background:color-mix(in srgb, var(--td-accent) {SHADES[i]}%, var(--td-surface));color:{i === 0 ? 'var(--td-on-accent)' : 'var(--td-text)'};"
							title="{al.name}: {nf(al.plays)} plays ({Math.round(p)}%)"
							on:click={() => openAlbum(al.album_artist, al.name)}>
							{#if p >= 11}<span class="t">{al.name}</span><span class="p">{Math.round(p)}%</span>{/if}
						</button>
					{/each}
					{#if a.rest_plays > 0}
						{@const p = share(a.rest_plays, a.plays)}
						<div class="seg rest" style="flex:{a.rest_plays};"
							title="{nf(a.album_count - a.albums.length)} more records: {nf(a.rest_plays)} plays ({Math.round(p)}%)">
							{#if p >= 11}<span class="t">+{nf(a.album_count - a.albums.length)} more</span><span class="p">{Math.round(p)}%</span>{/if}
						</div>
					{/if}
					{#if a.no_album > 0}
						{@const p = share(a.no_album, a.plays)}
						<div class="seg none" style="flex:{a.no_album};"
							title="No record named: {nf(a.no_album)} plays ({Math.round(p)}%)">
							{#if p >= 11}<span class="t">no record</span><span class="p">{Math.round(p)}%</span>{/if}
						</div>
					{/if}
				</div>
			</div>
		{/each}
	</div>
	<div class="legend">
		<span><i class="sw" style="background:var(--td-accent);"></i>most-played record, then lighter</span>
		<span><i class="sw rest"></i>every other record</span>
		<span><i class="sw none"></i>no record named</span>
	</div>
{/if}

<style>
	.rows { display: flex; flex-direction: column; gap: 10px; }
	.row { display: grid; grid-template-columns: 190px minmax(0, 1fr); gap: 14px; align-items: center; }
	@media (max-width: 640px) { .row { grid-template-columns: 1fr; gap: 4px; } }
	.who { display: flex; flex-direction: column; align-items: flex-end; text-align: right; background: none; border: 0; padding: 0; cursor: pointer; font-family: inherit; color: var(--td-text); min-width: 0; }
	@media (max-width: 640px) { .who { align-items: flex-start; text-align: left; } }
	.nm { font-size: 13px; font-weight: 600; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.sub { font-size: 11.5px; color: var(--td-muted-3); }
	.bar { display: flex; gap: 2px; height: 34px; border-radius: 8px; overflow: hidden; }
	.seg {
		min-width: 2px; border: 0; padding: 0 6px; display: flex; flex-direction: column; justify-content: center;
		align-items: center; overflow: hidden; font-family: inherit; cursor: pointer; line-height: 1.15;
	}
	div.seg { cursor: default; }
	.seg .t { font-size: 11px; font-weight: 600; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.seg .p { font-size: 10.5px; opacity: 0.85; }
	.rest { background: var(--td-track); color: var(--td-muted); }
	.none {
		background: repeating-linear-gradient(135deg, var(--td-track) 0 4px, var(--td-surface) 4px 8px);
		color: var(--td-muted);
	}
	.legend { display: flex; flex-wrap: wrap; gap: 6px 18px; margin-top: 14px; font-size: 12px; color: var(--td-muted-3); }
	.sw { display: inline-block; width: 12px; height: 12px; border-radius: 3px; vertical-align: -2px; margin-right: 6px; }
	.sw.rest { background: var(--td-track); }
	.sw.none { background: repeating-linear-gradient(135deg, var(--td-track) 0 3px, var(--td-surface) 3px 6px); border: 1px solid var(--td-border); }
</style>
