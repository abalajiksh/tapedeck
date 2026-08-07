<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { api } from '$lib/api';
	import type { AlbumView } from '$lib/api';
	import { formatDuration, qualityLabel, tierTagClass } from '$lib/utils';
	import { openArtist } from '$lib/entities';
	import { lovedKeys } from '$lib/loves';
	import CoverArt from '$lib/components/CoverArt.svelte';
	import StatStrip from '$lib/components/StatStrip.svelte';
	import BioCard from '$lib/components/BioCard.svelte';
	import AlbumCard from '$lib/components/AlbumCard.svelte';

	let album: AlbumView | null = null;
	let loading = true;
	let error = '';
	let loveBusy = false;

	// Same override the artist page offers for a portrait. An album otherwise
	// wears whatever artwork the newest listen of it carries — right most of the
	// time, and unfixable when it's the compilation's cover or the wrong edition.
	let editingCover = false;
	let coverUrl = '';
	let coverMsg = '';

	async function saveCover(url: string) {
		if (!album) return;
		coverMsg = '';
		try {
			const r = await api.setAlbumCover(album.id, url);
			album = { ...album, artwork_url: r.artwork_url, cover_override: !!r.artwork_url };
			coverUrl = '';
			editingCover = false;
			coverMsg = url ? 'Cover updated.' : 'Cover reset.';
			// Clearing puts the derived cover back, which the server picks — so
			// the page has to ask again rather than guess at it here.
			if (!url) await load();
			setTimeout(() => (coverMsg = ''), 2500);
		} catch (e) {
			coverMsg = String(e).replace(/^API \d+: /, '');
		}
	}

	$: id = Number($page.params.id);

	// The tracklist bars are relative to the album's own most-played track, not
	// to anything global — the shape of *this* record is what's being shown.
	$: topPlays = album?.tracks.reduce((m, t) => Math.max(m, t.plays), 0) ?? 0;

	// Two ways to read a tracklist: what you play most, and the record's own
	// running order. The server sorts by plays; sequence is a client-side
	// re-sort. Tracks with no track number sink to the end rather than
	// pretending to be track zero.
	let sort: 'plays' | 'sequence' = 'plays';
	$: tracks = (() => {
		const list = [...(album?.tracks ?? [])];
		if (sort === 'plays') return list;
		return list.sort((a, b) => {
			const an = a.track_number ?? Number.MAX_SAFE_INTEGER;
			const bn = b.track_number ?? Number.MAX_SAFE_INTEGER;
			return an === bn ? a.title.localeCompare(b.title) : an - bn;
		});
	})();

	// The album's own runtime, summed over its distinct tracks — `listening_seconds`
	// from the API is how long *you* have spent on it, which the Hours stat says.
	$: runtime = album ? formatRuntime(album.tracks.reduce((n, t) => n + (t.duration ?? 0), 0)) : '';
	function formatRuntime(seconds: number): string {
		if (!seconds) return '';
		const m = Math.round(seconds / 60);
		return m >= 60 ? `${Math.floor(m / 60)}h ${m % 60}m` : `${m} min`;
	}

	// The love state lives in the shared store so a heart toggled here matches
	// the one on the history rows without either refetching.
	$: albumKey = album ? `release|${album.artist.trim().toLowerCase()}|${album.title.trim().toLowerCase()}` : '';
	$: loved = albumKey ? $lovedKeys.has(albumKey) : false;

	async function toggleLove() {
		if (!album || loveBusy) return;
		loveBusy = true;
		const next = !loved;
		lovedKeys.update((s) => {
			const c = new Set(s);
			if (next) c.add(albumKey); else c.delete(albumKey);
			return c;
		});
		try {
			await api.setLove({ entity_id: album.id }, next);
		} catch {
			lovedKeys.update((s) => {
				const c = new Set(s);
				if (next) c.delete(albumKey); else c.add(albumKey);
				return c;
			});
		}
		loveBusy = false;
	}

	async function load() {
		loading = true;
		error = '';
		try {
			album = await api.getAlbum(id);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not load this album';
		}
		loading = false;
	}

	// Re-fetch when navigating album → "more from artist" → album without an
	// unmount in between.
	let lastId: number | null = null;
	$: if (id && id !== lastId) {
		lastId = id;
		load();
	}

	onMount(() => {
		// Populate the loved set if this page was the entry point.
		import('$lib/loves').then((m) => m.loadLoves());
	});
</script>

<svelte:head><title>{album ? `${album.title} — Tapedeck` : 'Album — Tapedeck'}</title></svelte:head>

{#if loading}
	<div style="display:flex;justify-content:center;padding:80px;"><div class="spin"></div></div>
{:else if error}
	<div class="panel" style="border-radius:22px;padding:44px;text-align:center;">
		<div class="h-display" style="font-size:20px;margin-bottom:6px;">Nothing on this side</div>
		<div style="color:var(--td-muted);font-size:14px;margin-bottom:18px;">{error}</div>
		<a class="btn btn-secondary" href="/history" style="text-decoration:none;">Back to history</a>
	</div>
{:else if album}
	<button class="back" on:click={() => history.back()}>
		<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.75" stroke-linecap="round" stroke-linejoin="round"><path d="M15 5l-7 7 7 7"/></svg>Back
	</button>

	<div class="hero">
		<div class="cover">
			<CoverArt
				url={album.artwork_url}
				caaId={album.caa_id}
				releaseMbid={album.caa_release_mbid}
				seed={album.title}
				size={260}
				radius={22}
			/>
			<!-- Quiet until you go looking for it — a correction, not a call to
			     action. Same treatment as the artist portrait's. -->
			<button class="pic-edit" on:click={() => (editingCover = !editingCover)}>
				{album.cover_override ? 'Change cover' : 'Set the cover'}
			</button>
		</div>
		<div style="flex:1;min-width:260px;">
			<div class="eyebrow">Album</div>
			<h1 class="h-display" style="font-size:40px;margin:0 0 6px;line-height:1.04;">{album.title}</h1>
			<p style="margin:0 0 16px;color:var(--td-muted);font-size:16px;">
				<button class="linkish" on:click={() => album && openArtist(album.artist)}>{album.artist}</button>
				{album.year ? ` · ${album.year}` : ''}
				{album.track_count ? ` · ${album.track_count} track${album.track_count === 1 ? '' : 's'}` : ''}
			</p>
			<div style="display:flex;gap:8px;flex-wrap:wrap;align-items:center;">
				{#if runtime}<span class="tag tag-neutral">{runtime}</span>{/if}
				<button class="love" class:on={loved} on:click={toggleLove} disabled={loveBusy}
					aria-pressed={loved} aria-label={loved ? 'Remove love' : 'Love this album'}>
					<svg viewBox="0 0 24 24" width="16" fill={loved ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"><path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1-1.1a5.5 5.5 0 0 0-7.8 7.8l1.1 1L12 21l7.7-7.6 1.1-1a5.5 5.5 0 0 0 0-7.8z"/></svg>
					{loved ? 'Loved' : 'Love'}
				</button>
				<a class="btn btn-secondary" style="font-size:12.5px;text-decoration:none;"
					href={`/notes?entity=${album.id}`}>Liner notes</a>
			</div>
		</div>
	</div>

	{#if editingCover}
		<div class="panel" style="border-radius:18px;padding:16px;margin-bottom:14px;">
			<div style="display:flex;gap:10px;flex-wrap:wrap;align-items:center;">
				<input class="input" style="flex:1;min-width:240px;" bind:value={coverUrl}
					placeholder="https://… or /api/v1/art/<listen id>">
				<button class="btn btn-primary" on:click={() => saveCover(coverUrl)}>Save</button>
				{#if album.cover_override}
					<button class="btn btn-secondary" on:click={() => saveCover('')}>Reset</button>
				{/if}
				<button class="btn btn-secondary" on:click={() => (editingCover = false)}>Cancel</button>
			</div>
			<p style="font-size:12.5px;color:var(--td-muted);margin:10px 0 0;">
				Paste any image URL. Without one, the cover is whichever artwork the
				newest listen of this record carries — Reset goes back to that.
			</p>
		</div>
	{/if}
	{#if coverMsg}
		<p style="font-size:13px;color:var(--td-good-fg);margin:0 0 12px;">{coverMsg}</p>
	{/if}

	<StatStrip stats={album.stats} />

	<BioCard bio={album.bio} heading="About this release" />

	{#if album.tracks.length}
		<div class="sec-head">
			<h3 class="h-display" style="font-size:20px;margin:0;">Tracks</h3>
			<div style="display:inline-flex;background:var(--td-sidebar);border-radius:999px;padding:3px;gap:2px;">
				<button class="seg-btn" class:on={sort === 'plays'} on:click={() => (sort = 'plays')}>By plays</button>
				<button class="seg-btn" class:on={sort === 'sequence'} on:click={() => (sort = 'sequence')}>Track order</button>
			</div>
		</div>
		<div class="panel" style="border-radius:22px;overflow:hidden;">
			{#each tracks as t, i}
				{@const q = qualityLabel(t)}
				<div class="track">
					<span class="n">{sort === 'sequence' ? (t.track_number ?? '—') : i + 1}</span>
					<div style="flex:1;min-width:0;">
						<div class="t-title">{t.title}</div>
						<div class="bar"><div class="fill" style="width:{topPlays ? (t.plays / topPlays) * 100 : 0}%"></div></div>
					</div>
					{#if q.text}<span class={tierTagClass(q.tier)} style="flex:none;">{q.text}</span>{/if}
					<span class="plays">{t.plays}</span>
					<span class="dur">{formatDuration(t.duration)}</span>
				</div>
			{/each}
		</div>
	{:else}
		<div class="panel" style="border-radius:22px;padding:36px;text-align:center;color:var(--td-muted);">
			No listens of this record yet.
		</div>
	{/if}

	{#if album.more.length}
		<div class="sec-head" style="margin-top:30px;">
			<h3 class="h-display" style="font-size:20px;margin:0;">More from {album.artist}</h3>
			<button class="linkish" on:click={() => album && openArtist(album.artist)}>Artist page →</button>
		</div>
		<div class="grid">
			{#each album.more as m}
				<AlbumCard album={m} artist={album.artist} />
			{/each}
		</div>
	{/if}
{/if}

<style>
	.spin {
		width: 22px; height: 22px; border: 2px solid var(--td-track);
		border-top-color: var(--td-accent); border-radius: 50%;
		animation: td-spin 0.7s linear infinite;
	}
	.back {
		display: inline-flex; align-items: center; gap: 7px;
		border: none; background: transparent; cursor: pointer;
		font-size: 13.5px; font-weight: 600; color: var(--td-nav-fg);
		padding: 0; margin-bottom: 18px;
	}
	.back:hover { color: var(--td-text); }
	.hero {
		display: flex; gap: 30px; flex-wrap: wrap;
		align-items: center; margin-bottom: 30px;
	}
	.cover {
		width: 260px; max-width: 100%; flex: none;
		display: flex; flex-direction: column; gap: 8px; align-items: center;
	}
	.pic-edit {
		border: none; background: none; padding: 0; cursor: pointer;
		font: inherit; font-size: 12px; color: var(--td-muted-2);
		text-decoration: underline; text-underline-offset: 3px;
	}
	.pic-edit:hover { color: var(--td-accent); }
	.eyebrow {
		font-size: 11px; letter-spacing: .16em; text-transform: uppercase;
		color: #c67139; font-weight: 600; margin-bottom: 8px;
	}
	/* A button that reads as a link — these navigate via a resolve round-trip,
	   so they can't be anchors. */
	.linkish {
		border: none; background: transparent; padding: 0; cursor: pointer;
		font: inherit; font-weight: 600; color: #c67139;
	}
	.linkish:hover { color: #e07a55; text-decoration: underline; }
	.love {
		display: inline-flex; align-items: center; gap: 7px;
		border: 1px solid var(--td-border); background: var(--td-surface);
		border-radius: 999px; padding: 6px 14px; cursor: pointer;
		font-size: 12.5px; font-weight: 600; color: var(--td-nav-fg);
	}
	.love:hover { color: #b2622d; }
	.love.on { color: #b2622d; }
	.sec-head {
		display: flex; align-items: center; justify-content: space-between;
		gap: 12px; margin-bottom: 12px;
	}
	/* Same segmented control as the Statistics range picker. Styles are scoped
	   per page in this codebase, so it's repeated rather than shared. */
	.seg-btn {
		border: none; background: transparent; cursor: pointer; color: var(--td-nav-fg);
		font-size: 12.5px; padding: 6px 14px; border-radius: 999px; font-family: inherit;
	}
	.seg-btn.on { background: var(--td-accent); color: #fbf5ea; }
	.track {
		display: flex; align-items: center; gap: 14px;
		padding: 12px 18px; border-bottom: 1px solid var(--td-border);
	}
	.track:last-child { border-bottom: none; }
	.n {
		width: 26px; flex: none; text-align: center;
		font-variant-numeric: tabular-nums; font-size: 13px; color: var(--td-muted-3);
	}
	.t-title {
		font-size: 14.5px; font-weight: 600;
		white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
	}
	.bar {
		height: 5px; border-radius: 999px; background: var(--td-track);
		overflow: hidden; margin-top: 6px; max-width: 220px;
	}
	.fill {
		height: 100%; border-radius: 999px;
		background: linear-gradient(90deg, var(--td-accent-2), var(--td-accent-hi));
	}
	.plays {
		font-size: 12.5px; color: var(--td-muted-2); width: 54px;
		text-align: right; flex: none; font-variant-numeric: tabular-nums;
	}
	.dur {
		font-size: 12px; color: var(--td-muted-3); width: 46px;
		text-align: right; flex: none; font-variant-numeric: tabular-nums;
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
		gap: 16px;
	}
</style>
