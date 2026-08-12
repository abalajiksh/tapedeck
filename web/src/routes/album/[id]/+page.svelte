<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { api } from '$lib/api';
	import type { AlbumView, AlbumTracklist, TracklistTrack } from '$lib/api';
	import { formatDuration, qualityLabel, tierTagClass } from '$lib/utils';
	import { openArtist } from '$lib/entities';
	import { lovedKeys, loveKeyOf, toggleLoveByName } from '$lib/loves';
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

	// ── The record's own tracklist ──
	// Everything else on this page is built from listens, which means a record
	// you have played twice renders as a two-track record. Pulling the real
	// tracklist is what turns the page from a picture of your listening into a
	// picture of the object — so it is opt-in, costs a rate-limited call to
	// MusicBrainz, and is never stored: writing these into `scrobbles` would be
	// inventing listens that never happened.
	let full: AlbumTracklist | null = null;
	let fullLoading = false;
	let fullError = '';
	let showFull = false;

	async function loadTracklist() {
		if (!album || fullLoading) return;
		if (full) { showFull = !showFull; return; }
		fullLoading = true;
		fullError = '';
		try {
			full = await api.getAlbumTracklist(album.id);
			showFull = true;
		} catch (e) {
			fullError = String(e).replace(/^API \d+: /, '');
		}
		fullLoading = false;
	}

	/** Light fold for matching a played title against the record's own. Not
	 *  `normalise_artist`'s aggression — that answers "same performer?", where
	 *  over-folding invents a merge. Here a miss only costs a row appearing as
	 *  unplayed, so punctuation and case are folded and nothing else. */
	function titleKey(t: string): string {
		return t.trim().toLowerCase().replace(/[^\p{L}\p{N}]+/gu, '');
	}

	interface Row {
		title: string;
		artist: string;
		position: number | null;
		side: string | null;
		duration: number | null;
		plays: number;
		/** Present only for tracks you have actually played — it carries the
		 *  quality columns the badge is built from. */
		played: AlbumView['tracks'][number] | null;
	}

	/** The record's running order, with your plays attached to the tracks that
	 *  have any. A played track the lookup does not list is kept and appended:
	 *  it is evidence of something you actually heard, and MusicBrainz being
	 *  short a bonus track is not a reason to hide it. */
	function mergeTracklist(listed: TracklistTrack[], played: AlbumView['tracks']): Row[] {
		const byKey = new Map(played.map((t) => [titleKey(t.title), t]));
		const claimed = new Set<string>();

		const rows: Row[] = listed.map((t) => {
			const key = titleKey(t.title);
			const hit = byKey.get(key);
			if (hit) claimed.add(key);
			return {
				title: t.title,
				artist: t.artist ?? album?.artist ?? '',
				position: t.position,
				side: t.side,
				duration: hit?.duration ?? t.duration_secs ?? null,
				plays: hit?.plays ?? 0,
				played: hit ?? null
			};
		});

		for (const t of played) {
			if (claimed.has(titleKey(t.title))) continue;
			rows.push({
				title: t.title,
				artist: album?.artist ?? '',
				position: t.track_number ?? null,
				side: null,
				duration: t.duration ?? null,
				plays: t.plays,
				played: t
			});
		}
		return rows;
	}

	$: fullRows = full && showFull ? mergeTracklist(full.tracks, album?.tracks ?? []) : [];
	$: unplayedCount = fullRows.filter((r) => !r.plays).length;
	/** Sides only mean something on a record pressed with them — a CD comes back
	 *  as one side and labelling every row "A" would be noise. */
	$: showSides = new Set(fullRows.map((r) => r.side).filter(Boolean)).size > 1;

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

	<div class="sec-head">
		<h3 class="h-display" style="font-size:20px;margin:0;">Tracks</h3>
		<div style="display:flex;gap:8px;align-items:center;flex-wrap:wrap;">
			{#if !showFull && album.tracks.length}
				<div style="display:inline-flex;background:var(--td-sidebar);border-radius:999px;padding:3px;gap:2px;">
					<button class="seg-btn" class:on={sort === 'plays'} on:click={() => (sort = 'plays')}>By plays</button>
					<button class="seg-btn" class:on={sort === 'sequence'} on:click={() => (sort = 'sequence')}>Track order</button>
				</div>
			{/if}
			<!-- Opt-in, because it is a call to somebody else's rate-limited
			     server. Once pulled it is held for the life of the page, so the
			     button becomes a plain toggle. -->
			<button class="btn btn-secondary" style="font-size:12.5px;" on:click={loadTracklist} disabled={fullLoading}>
				{#if fullLoading}Looking it up…
				{:else if !full}Whole record
				{:else if showFull}Only what I've played
				{:else}Whole record{/if}
			</button>
		</div>
	</div>

	{#if fullError}
		<p style="font-size:13px;color:var(--td-muted);margin:0 0 12px;">{fullError}</p>
	{/if}

	{#if showFull && full}
		<p style="font-size:12.5px;color:var(--td-muted);margin:0 0 10px;">
			The record as MusicBrainz has it{full.year ? `, ${full.year}` : ''}{full.format ? ` · ${full.format}` : ''}
			{#if unplayedCount} — {unplayedCount} track{unplayedCount === 1 ? '' : 's'} you haven't played{/if}.
			Nothing here is stored as a listen.
		</p>
		<div class="panel" style="border-radius:22px;overflow:hidden;">
			{#each fullRows as r}
				{@const q = r.played ? qualityLabel(r.played) : null}
				{@const key = loveKeyOf('track', r.artist, r.title)}
				{@const isLoved = key ? $lovedKeys.has(key) : false}
				<div class="track" class:unplayed={!r.plays}>
					<span class="n">{showSides && r.side ? `${r.side}${r.position ?? ''}` : (r.position ?? '—')}</span>
					<div style="flex:1;min-width:0;">
						<div class="t-title">{r.title}</div>
						<div class="bar"><div class="fill" style="width:{topPlays ? (r.plays / topPlays) * 100 : 0}%"></div></div>
					</div>
					<button class="t-love" class:on={isLoved}
						on:click={() => toggleLoveByName('track', r.artist, r.title)}
						aria-pressed={isLoved} aria-label={isLoved ? 'Remove love' : 'Love this track'}
						title={isLoved ? 'Loved — click to remove' : 'Love this track'}>
						<svg viewBox="0 0 24 24" width="15" fill={isLoved ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1-1.1a5.5 5.5 0 0 0-7.8 7.8l1.1 1L12 21l7.7-7.6 1.1-1a5.5 5.5 0 0 0 0-7.8z"/></svg>
					</button>
					{#if q?.text}<span class={tierTagClass(q.tier)} style="flex:none;">{q.text}</span>{/if}
					<span class="plays">{r.plays || '—'}</span>
					<span class="dur">{formatDuration(r.duration)}</span>
				</div>
			{/each}
		</div>
	{:else if album.tracks.length}
		<div class="panel" style="border-radius:22px;overflow:hidden;">
			{#each tracks as t, i}
				{@const q = qualityLabel(t)}
				{@const key = loveKeyOf('track', album.artist, t.title)}
				{@const isLoved = key ? $lovedKeys.has(key) : false}
				<div class="track">
					<span class="n">{sort === 'sequence' ? (t.track_number ?? '—') : i + 1}</span>
					<div style="flex:1;min-width:0;">
						<div class="t-title">{t.title}</div>
						<div class="bar"><div class="fill" style="width:{topPlays ? (t.plays / topPlays) * 100 : 0}%"></div></div>
					</div>
					<button class="t-love" class:on={isLoved}
						on:click={() => album && toggleLoveByName('track', album.artist, t.title)}
						aria-pressed={isLoved} aria-label={isLoved ? 'Remove love' : 'Love this track'}
						title={isLoved ? 'Loved — click to remove' : 'Love this track'}>
						<svg viewBox="0 0 24 24" width="15" fill={isLoved ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1-1.1a5.5 5.5 0 0 0-7.8 7.8l1.1 1L12 21l7.7-7.6 1.1-1a5.5 5.5 0 0 0 0-7.8z"/></svg>
					</button>
					{#if q.text}<span class={tierTagClass(q.tier)} style="flex:none;">{q.text}</span>{/if}
					<span class="plays">{t.plays}</span>
					<span class="dur">{formatDuration(t.duration)}</span>
				</div>
			{/each}
		</div>
	{:else}
		<div class="panel" style="border-radius:22px;padding:36px;text-align:center;color:var(--td-muted);">
			No listens of this record yet — pull the whole record to see what's on it.
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
	/* A track on the record you have never played. Dimmed rather than hidden or
	   badged — it is part of the object, and the whole reason for pulling the
	   tracklist is to see the shape of what you have been skipping. */
	.track.unplayed { opacity: .55; }
	.track.unplayed .t-title { font-weight: 500; }

	/* Revealed on hover like the history row's, not always-on like the deck's:
	   there are a dozen of these on a page and a column of filled outlines would
	   read as decoration. Always visible once loved, and always visible on touch,
	   where there is no hover to reveal it. */
	.t-love {
		flex: none; border: none; background: transparent; padding: 4px; cursor: pointer;
		color: var(--td-muted-3); border-radius: 8px; display: flex;
		opacity: 0; transition: opacity .15s, color .15s, transform .15s;
	}
	.track:hover .t-love, .t-love:focus-visible, .t-love.on { opacity: 1; }
	.t-love.on { color: #c67139; }
	.t-love:hover { color: #b2622d; transform: scale(1.1); }
	@media (hover: none) { .t-love { opacity: 1; } }
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
