<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { api } from '$lib/api';
	import type { ArtistView } from '$lib/api';
	import { formatDuration, qualityLabel, tierTagClass } from '$lib/utils';
	import { openAlbum } from '$lib/entities';
	import { lovedKeys } from '$lib/loves';
	import StatStrip from '$lib/components/StatStrip.svelte';
	import AlbumCard from '$lib/components/AlbumCard.svelte';

	let artist: ArtistView | null = null;
	let loading = true;
	let error = '';
	let loveBusy = false;

	$: id = Number($page.params.id);
	$: topPlays = artist?.top_tracks.reduce((m, t) => Math.max(m, t.plays), 0) ?? 0;
	$: peakYear = artist?.trend.reduce((m, t) => Math.max(m, t.value), 0) ?? 0;

	// An artist entity's key carries an empty artist half — the name *is* the
	// artist. Matches `Database::entity_key`.
	$: artistKey = artist ? `artist||${artist.name.trim().toLowerCase()}` : '';
	$: loved = artistKey ? $lovedKeys.has(artistKey) : false;

	async function toggleLove() {
		if (!artist || loveBusy) return;
		loveBusy = true;
		const next = !loved;
		lovedKeys.update((s) => {
			const c = new Set(s);
			if (next) c.add(artistKey); else c.delete(artistKey);
			return c;
		});
		try {
			await api.setLove({ entity_id: artist.id }, next);
		} catch {
			lovedKeys.update((s) => {
				const c = new Set(s);
				if (next) c.delete(artistKey); else c.add(artistKey);
				return c;
			});
		}
		loveBusy = false;
	}

	async function load() {
		loading = true;
		error = '';
		try {
			artist = await api.getArtist(id);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not load this artist';
		}
		loading = false;
	}

	let lastId: number | null = null;
	$: if (id && id !== lastId) {
		lastId = id;
		load();
	}

	onMount(() => {
		import('$lib/loves').then((m) => m.loadLoves());
	});
</script>

<svelte:head><title>{artist ? `${artist.name} — Tapedeck` : 'Artist — Tapedeck'}</title></svelte:head>

{#if loading}
	<div style="display:flex;justify-content:center;padding:80px;"><div class="spin"></div></div>
{:else if error}
	<div class="panel" style="border-radius:22px;padding:44px;text-align:center;">
		<div class="h-display" style="font-size:20px;margin-bottom:6px;">Nothing on this side</div>
		<div style="color:var(--td-muted);font-size:14px;margin-bottom:18px;">{error}</div>
		<a class="btn btn-secondary" href="/history" style="text-decoration:none;">Back to history</a>
	</div>
{:else if artist}
	<button class="back" on:click={() => history.back()}>
		<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.75" stroke-linecap="round" stroke-linejoin="round"><path d="M15 5l-7 7 7 7"/></svg>Back
	</button>

	<div class="hero">
		<!-- A stack of cassettes rather than a photo: Tapedeck has no artist
		     imagery of its own and inventing one from a provider is UX 4's next
		     step, not this one. -->
		<div class="stack">
			<div class="sheet a"></div>
			<div class="sheet b"></div>
			<div class="face">
				<svg viewBox="0 0 198 130" style="width:100%;height:auto;display:block;">
					<rect x="14" y="10" width="170" height="30" rx="7" fill="#f4e7cf"/>
					<rect x="30" y="52" width="138" height="62" rx="9" fill="#1b1712"/>
					<rect x="70" y="72" width="58" height="20" rx="2" fill="#3a3128"/>
					{#each [70, 128] as cx}
						<g>
							<circle {cx} cy="83" r="21" fill="none" stroke="#e9dcc4" stroke-width="2.4"/>
							<circle {cx} cy="83" r="6" fill="#e9dcc4"/>
							<line x1={cx - 19} y1="83" x2={cx + 19} y2="83" stroke="#e9dcc4" stroke-width="2"/>
							<line x1={cx - 9} y1="66" x2={cx + 9} y2="100" stroke="#e9dcc4" stroke-width="2"/>
							<line x1={cx - 9} y1="100" x2={cx + 9} y2="66" stroke="#e9dcc4" stroke-width="2"/>
						</g>
					{/each}
				</svg>
			</div>
		</div>

		<div style="flex:1;min-width:260px;">
			<div class="eyebrow">Artist</div>
			<h1 class="h-display" style="font-size:44px;margin:0 0 6px;line-height:1.02;">{artist.name}</h1>
			<p style="margin:0 0 16px;color:var(--td-muted);font-size:16px;">
				{artist.album_count} album{artist.album_count === 1 ? '' : 's'} ·
				{artist.track_count} track{artist.track_count === 1 ? '' : 's'} in your history
			</p>
			<div style="display:flex;gap:10px;flex-wrap:wrap;align-items:center;">
				{#if artist.rank > 0}
					<span class="rank">
						<svg viewBox="0 0 24 24" width="14" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12l4 4L19 6"/></svg>
						#{artist.rank} of {artist.artist_total} most played
					</span>
				{/if}
				<button class="love" class:on={loved} on:click={toggleLove} disabled={loveBusy}
					aria-pressed={loved} aria-label={loved ? 'Remove love' : 'Love this artist'}>
					<svg viewBox="0 0 24 24" width="16" fill={loved ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"><path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1-1.1a5.5 5.5 0 0 0-7.8 7.8l1.1 1L12 21l7.7-7.6 1.1-1a5.5 5.5 0 0 0 0-7.8z"/></svg>
					{loved ? 'Loved' : 'Love'}
				</button>
				<a class="btn btn-secondary" style="font-size:12.5px;text-decoration:none;"
					href={`/notes?entity=${artist.id}`}>Liner notes</a>
			</div>
		</div>
	</div>

	<StatStrip stats={artist.stats} />

	{#if artist.trend.length > 1}
		<h3 class="h-display" style="font-size:20px;margin:0 0 12px;">Plays by year</h3>
		<div class="panel trend">
			{#each artist.trend as t}
				<div class="tcol">
					<div class="tbar" style="height:{peakYear ? Math.max(4, (t.value / peakYear) * 100) : 4}%" title={`${t.value} plays`}></div>
					<div class="tlabel">{t.label}</div>
				</div>
			{/each}
		</div>
	{/if}

	{#if artist.albums.length}
		<h3 class="h-display" style="font-size:20px;margin:0 0 12px;">Top albums</h3>
		<div class="grid" style="margin-bottom:30px;">
			{#each artist.albums as a}
				<AlbumCard album={a} artist={artist.name} />
			{/each}
		</div>
	{/if}

	{#if artist.top_tracks.length}
		<div class="sec-head">
			<h3 class="h-display" style="font-size:20px;margin:0;">Top tracks</h3>
			<span style="font-size:12.5px;color:var(--td-muted-3);">by plays</span>
		</div>
		<div class="panel" style="border-radius:22px;overflow:hidden;">
			{#each artist.top_tracks as t, i}
				{@const q = qualityLabel(t)}
				<div class="track">
					<span class="n">{i + 1}</span>
					<div style="flex:1;min-width:0;">
						<div class="t-title">{t.title}</div>
						{#if t.album}
							<button class="t-album" on:click={() => artist && t.album && openAlbum(artist.name, t.album)}>{t.album}</button>
						{/if}
						<div class="bar"><div class="fill" style="width:{topPlays ? (t.plays / topPlays) * 100 : 0}%"></div></div>
					</div>
					{#if q.text}<span class={tierTagClass(q.tier)} style="flex:none;">{q.text}</span>{/if}
					<span class="plays">{t.plays}</span>
					<span class="dur">{formatDuration(t.duration)}</span>
				</div>
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
	.stack { width: 230px; max-width: 100%; flex: none; position: relative; }
	.sheet { position: absolute; inset: 0; border-radius: 24px; }
	.sheet.a { transform: rotate(-7deg) translateY(6px); background: var(--td-track); opacity: .5; }
	.sheet.b { transform: rotate(4deg) translateY(2px); background: var(--td-surface2); border: 1px solid var(--td-border); }
	.face { position: relative; border-radius: 24px; background: #2b2721; padding: 16px; }
	.eyebrow {
		font-size: 11px; letter-spacing: .16em; text-transform: uppercase;
		color: #c67139; font-weight: 600; margin-bottom: 8px;
	}
	.rank {
		display: inline-flex; align-items: center; gap: 8px;
		font-size: 12.5px; color: var(--td-good-fg); background: var(--td-good-bg);
		padding: 7px 14px; border-radius: 999px;
	}
	.love {
		display: inline-flex; align-items: center; gap: 7px;
		border: 1px solid var(--td-border); background: var(--td-surface);
		border-radius: 999px; padding: 6px 14px; cursor: pointer;
		font-size: 12.5px; font-weight: 600; color: var(--td-nav-fg);
	}
	.love:hover, .love.on { color: #b2622d; }
	.trend {
		border-radius: 22px; padding: 18px 20px; margin-bottom: 30px;
		display: flex; align-items: flex-end; gap: 10px; height: 150px;
	}
	.tcol { flex: 1; display: flex; flex-direction: column; align-items: center; justify-content: flex-end; height: 100%; }
	.tbar {
		width: 100%; max-width: 46px; border-radius: 7px 7px 3px 3px;
		background: linear-gradient(var(--td-accent-2), var(--td-accent));
	}
	.tlabel { font-size: 11.5px; color: var(--td-muted-3); margin-top: 7px; }
	.sec-head {
		display: flex; align-items: center; justify-content: space-between;
		gap: 12px; margin-bottom: 12px;
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
		gap: 16px;
	}
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
	.t-album {
		border: none; background: transparent; padding: 0; cursor: pointer;
		font: inherit; font-size: 12.5px; color: var(--td-muted);
		display: block; margin-top: 1px;
	}
	.t-album:hover { color: var(--td-accent); text-decoration: underline; }
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
</style>
