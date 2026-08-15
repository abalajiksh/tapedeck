<script lang="ts">
	// Playlists that live in Tapedeck.
	//
	// This is the review step the MCP design turns on: a list an assistant built
	// is saved here first and reaches a media server only when the person whose
	// server it is says so. It is also just a useful place to keep a list —
	// Playlist Lab next door generates them and had nowhere to put one.
	//
	// The track rows are deliberately the *same* rows Rediscovery draws — cover,
	// title, artist — album, right-aligned play count, hairline separators. A
	// list of bare text beside a panel full of sleeves reads as two different
	// applications, which is what it looked like before v0.64.2.
	import { onMount, tick } from 'svelte';
	import { api } from '$lib/api';
	import type { SavedPlaylist, PlaylistTrackView, PlaylistTarget } from '$lib/api';
	import { formatDateTime } from '$lib/utils';
	import CoverArt from '$lib/components/CoverArt.svelte';

	let playlists: SavedPlaylist[] = [];
	let targets: PlaylistTarget[] = [];
	let openId: number | null = null;
	let tracks: PlaylistTrackView[] = [];
	let loading = true;
	let tracksLoading = false;
	let busy = false;
	let result = '';

	onMount(async () => {
		await load();
		try {
			// An empty list is a normal answer — plenty of instances have no
			// writable source at all — and the page says so rather than showing
			// a button that cannot work.
			targets = (await api.playlistTargets()).targets;
		} catch {
			/* no targets is a normal state */
		}
		loading = false;
	});

	async function load() {
		try {
			playlists = (await api.getSavedPlaylists()).playlists;
		} catch (e) {
			console.error(e);
		}
	}

	async function open(p: SavedPlaylist) {
		if (openId === p.id) {
			openId = null;
			return;
		}
		openId = p.id;
		tracks = [];
		result = '';
		tracksLoading = true;
		try {
			tracks = (await api.getSavedPlaylist(p.id)).tracks;
		} catch (e) {
			console.error(e);
		}
		tracksLoading = false;
		// The list renders below the card, so with several playlists it can open
		// off-screen and the click looks like it did nothing — the complaint
		// Rediscovery's "See all" had, and the same fix.
		await tick();
		document
			.querySelector(`[data-playlist="${p.id}"]`)
			?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
	}

	async function push(p: SavedPlaylist, sourceId: number) {
		busy = true;
		result = '';
		try {
			const r = await api.pushSavedPlaylist(p.id, sourceId);
			// A partial match is the normal outcome, so it is reported as one.
			// Saying "sent" when three tracks did not land is the failure this
			// codebase keeps finding in other forms.
			result =
				r.matched === r.requested
					? `Sent all ${r.matched} tracks to ${r.protocol}.`
					: `Sent ${r.matched} of ${r.requested} to ${r.protocol}. Not found: ` +
						r.missing.map((m) => `${m.artist} — ${m.title}`).join(', ');
			await load();
		} catch (e) {
			result = e instanceof Error ? e.message : 'Could not send the playlist.';
		}
		busy = false;
	}

	async function remove(p: SavedPlaylist) {
		if (!confirm(`Delete “${p.name}”?`)) return;
		try {
			await api.deleteSavedPlaylist(p.id);
			if (openId === p.id) openId = null;
			await load();
		} catch (e) {
			console.error(e);
		}
	}
</script>

<svelte:head><title>Playlists · Tapedeck</title></svelte:head>

<div class="head">
	<div>
		<h1 class="h-display">Playlists</h1>
		<p class="sub">Lists kept in Tapedeck — yours, and any an assistant has handed back.</p>
	</div>
	<a class="btn btn-secondary" href="/playlists">Playlist Lab →</a>
</div>

{#if loading}
	<div class="spin"></div>
{:else if !playlists.length}
	<div class="panel empty">
		<p>No playlists yet.</p>
		<p class="dim">
			Build one in <a href="/playlists">Playlist Lab</a>, or connect an assistant in
			<a href="/settings">Settings → AI Connections</a> and ask it for one.
		</p>
	</div>
{:else}
	<div class="list">
		{#each playlists as p (p.id)}
			<div class="panel card" class:on={openId === p.id}>
				<div class="cardhead">
					<button class="cardmain" on:click={() => open(p)} aria-label="Open {p.name}">
						<div class="title">
							<span class="h-display name">{p.name}</span>
							{#if p.origin !== 'user'}
								<!-- Provenance, the same rule notes follow: a list a chatbot
								     wrote stays distinguishable from one you built. -->
								<span class="badge">by {p.origin}</span>
							{/if}
						</div>
						{#if p.description}<p class="desc">{p.description}</p>{/if}
						<div class="meta">
							{p.tracks} track{p.tracks === 1 ? '' : 's'} · {formatDateTime(p.updated_at)}
							{#if p.pushed_at}· sent to {p.pushed_to} {formatDateTime(p.pushed_at)}{/if}
						</div>
					</button>
					<div class="cardactions">
						{#if openId === p.id && targets.length}
							{#each targets as t}
								<button class="btn btn-primary send" disabled={busy} on:click={() => push(p, t.source_id)}>
									Send to {t.label}
								</button>
							{/each}
						{/if}
						<button class="del" on:click={() => remove(p)}>Delete</button>
					</div>
				</div>

				{#if openId === p.id}
					{#if result}<p class="result">{result}</p>{/if}
					{#if !targets.length}
						<p class="dim note">
							No media server is connected, so there is nowhere to send this yet.
							Add one in <a href="/settings">Settings → Sources</a>.
						</p>
					{/if}
				{/if}
			</div>

			{#if openId === p.id}
				<div class="panel tracks on" data-playlist={p.id}>
					{#if tracksLoading}
						<div class="spin small"></div>
					{:else}
						{#each tracks as t, i}
							<div class="row">
								<CoverArt
									url={t.artwork_url}
									caaId={t.caa_id}
									releaseMbid={t.caa_release_mbid}
									seed={`${t.artist} ${t.title}`}
									size={38}
									radius={8}
								/>
								<span class="rowtext">
									<span class="nm">{t.title}</span>
									<span class="sb">{t.artist}{t.album ? ` — ${t.album}` : ''}</span>
									{#if t.note}
										<!-- Why the track is here, in the words of whatever
										     chose it. Unique to a playlist; Rediscovery rows
										     have no equivalent. -->
										<span class="why">{t.note}</span>
									{/if}
								</span>
								{#if t.plays}
									<span class="plays">{t.plays} play{t.plays === 1 ? '' : 's'}</span>
								{/if}
							</div>
							{#if i < tracks.length - 1}<div class="sep"></div>{/if}
						{/each}
					{/if}
				</div>
			{/if}
		{/each}
	</div>
{/if}

<style>
	.head {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 16px;
		flex-wrap: wrap;
		margin-bottom: 20px;
	}
	h1 { font-size: 28px; margin: 0 0 4px; }
	.sub { font-size: 14px; color: var(--td-muted); margin: 0; }
	.list { display: flex; flex-direction: column; gap: 10px; }

	.card { border-radius: 20px; padding: 18px 20px; border: 1px solid transparent; }
	.card.on { border-color: var(--td-accent); }
	.cardhead { display: flex; align-items: flex-start; justify-content: space-between; gap: 14px; flex-wrap: wrap; }
	.cardmain {
		flex: 1; min-width: 0; text-align: left;
		background: none; border: 0; padding: 0; cursor: pointer; color: inherit;
	}
	.name { font-size: 19px; }
	.title { display: flex; align-items: baseline; gap: 10px; flex-wrap: wrap; }
	.badge {
		font-size: 10.5px; letter-spacing: 0.08em; text-transform: uppercase;
		color: var(--td-muted-3);
	}
	.desc { font-size: 13.5px; color: var(--td-muted); margin: 6px 0 0; }
	.meta { font-size: 12.5px; color: var(--td-muted-2); margin-top: 8px; }
	.cardactions { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
	.send { font-size: 12.5px; }
	.del { background: none; border: 0; color: #b2622d; font-size: 12.5px; cursor: pointer; }
	.result { font-size: 12.5px; color: var(--td-muted); margin: 14px 0 0; }
	.note { margin: 14px 0 0; }

	/* The Rediscovery row, exactly — cover, two lines, right-aligned count,
	   hairline separators inset from the panel edge. */
	/* The list wears the card's ring while it is up, so the two read as one
	   object rather than as a panel that happened to appear underneath. */
	.tracks { border-radius: 20px; overflow: hidden; margin: -2px 0 4px; border: 1px solid transparent; }
	.tracks.on { border-color: var(--td-accent); }
	.row { display: flex; align-items: center; gap: 12px; width: 100%; padding: 11px 16px; }
	.rowtext { flex: 1; min-width: 0; }
	.nm { display: block; font-size: 14px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.sb { display: block; font-size: 12.5px; color: var(--td-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.why { display: block; font-size: 12px; color: var(--td-muted-2); font-style: italic; margin-top: 3px; }
	.plays { font-size: 12.5px; color: var(--td-muted-2); flex: none; font-variant-numeric: tabular-nums; }
	.sep { height: 1px; background: var(--td-border); margin: 0 16px; }
	@media (max-width: 520px) { .plays { display: none; } }

	.empty { border-radius: 18px; padding: 28px; text-align: center; }
	.dim { font-size: 13px; color: var(--td-muted-2); }
	.spin {
		width: 22px; height: 22px; border: 2px solid var(--td-track);
		border-top-color: var(--td-accent); border-radius: 50%;
		margin: 40px auto; animation: spin 0.8s linear infinite;
	}
	.spin.small { margin: 18px auto; width: 18px; height: 18px; }
	@keyframes spin { to { transform: rotate(360deg); } }
	@media (prefers-reduced-motion: reduce) { .spin { animation: none; } }
</style>
