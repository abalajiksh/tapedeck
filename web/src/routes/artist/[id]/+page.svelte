<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { api } from '$lib/api';
	import DubDialog from '$lib/components/DubDialog.svelte';
	import type { ArtistDeck } from '$lib/api';
	let dubOpen = false;
	import type { ArtistView } from '$lib/api';
	import { formatDuration, qualityLabel, tierTagClass } from '$lib/utils';
	import { openAlbum } from '$lib/entities';
	import { lovedKeys, isLovedNamed } from '$lib/loves';
	import StatStrip from '$lib/components/StatStrip.svelte';
	import BioCard from '$lib/components/BioCard.svelte';
	import BarChart from '$lib/components/BarChart.svelte';
	import AlbumCard from '$lib/components/AlbumCard.svelte';

	let artist: ArtistView | null = null;
	let editingPic = false;
	let picUrl = '';
	let picMsg = '';
	/** A stored URL can rot — a library server goes off, a CDN link expires.
	 *  Tracked rather than removing the node, so the edit control survives. */
	let imgFailed = false;

	let wideUrl = '';

	async function saveWide(url: string) {
		if (!artist?.artist_row_id) return;
		picMsg = '';
		try {
			const r = await api.setArtistLandscape(artist.artist_row_id, url);
			artist = { ...artist, landscape_url: r.landscape_url };
			wideUrl = '';
			picMsg = url ? 'Wide picture updated.' : 'Wide picture cleared.';
			setTimeout(() => (picMsg = ''), 2500);
		} catch (e) {
			picMsg = String(e).replace(/^API \d+: /, '');
		}
	}

	/** Upload rather than paste. Same override, different way in — a picture on
	 *  your own disk had no route before this. */
	let picBusy = false;
	async function uploadPicture(e: Event, wide: boolean) {
		const input = e.target as HTMLInputElement;
		const file = input.files?.[0];
		if (!file || !artist?.artist_row_id) return;
		picBusy = true;
		picMsg = '';
		try {
			if (wide) {
				const r = await api.uploadArtistLandscape(artist.artist_row_id, file);
				artist = { ...artist, landscape_url: r.landscape_url };
				picMsg = 'Wide picture uploaded.';
			} else {
				const r = await api.uploadArtistImage(artist.artist_row_id, file);
				artist = { ...artist, image_url: r.image_url };
				imgFailed = false;
				picMsg = 'Picture uploaded.';
			}
			setTimeout(() => (picMsg = ''), 2500);
		} catch (err) {
			picMsg = String(err).replace(/^API \d+: /, '');
		}
		picBusy = false;
		// Cleared so re-picking the same file fires `change` again.
		input.value = '';
	}

	async function savePicture(url: string) {
		if (!artist?.artist_row_id) return;
		picMsg = '';
		try {
			const r = await api.setArtistImage(artist.artist_row_id, url);
			artist = { ...artist, image_url: r.image_url };
			imgFailed = false;
			picUrl = '';
			editingPic = false;
			picMsg = url ? 'Picture updated.' : 'Picture cleared.';
			setTimeout(() => (picMsg = ''), 2500);
		} catch (e) {
			picMsg = String(e).replace(/^API \d+: /, '');
		}
	}
	let loading = true;
	let error = '';
	let loveBusy = false;

	$: id = Number($page.params.id);
	$: topPlays = artist?.top_tracks.reduce((m, t) => Math.max(m, t.plays), 0) ?? 0;

	// An artist entity's key carries an empty artist half — the name *is* the
	// artist. Matches `Database::entity_key`.
	$: artistKey = artist ? `artist||${artist.name.trim().toLowerCase()}` : '';
	$: loved = artist ? isLovedNamed($lovedKeys, 'artist', artist.name, artist.name) : false;

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

	// Who else on this instance plays them, and which of your own chains you
	// hear them on. Loaded after the artist rather than beside it: the name is
	// what it keys on, and a failure here must not take the page down — an
	// instance with nobody else on it simply has no panel.
	let otherDecks: ArtistDeck[] = [];
	let myChains: { name: string; plays: number }[] = [];

	async function loadPatchContext(name: string) {
		otherDecks = [];
		myChains = [];
		asked = new Set();
		try {
			const r = await api.getPatchArtistContext(name);
			otherDecks = r.decks;
			myChains = r.chains;
		} catch {
			/* the panels simply do not render */
		}
	}

	// Tracked beside the rows rather than folded onto them: `patched` is the
	// server's answer and stays the server's, and a patch is *pending* until
	// they say yes — writing `patched: true` here would claim it was accepted.
	let asked = new Set<string>();
	async function askPatch(username: string) {
		try {
			await api.requestPatch(username);
			asked = new Set([...asked, username]);
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not ask');
		}
	}

	async function load() {
		loading = true;
		error = '';
		try {
			artist = await api.getArtist(id);
			void loadPatchContext(artist.name);
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
				<!-- Passing a record to somebody. The endpoint has existed since
				     v0.80.0 with nothing calling it, so dubs could be received and
				     never sent. -->
				<button class="btn btn-secondary" style="font-size:12.5px;" on:click={() => (dubOpen = true)}>
					Dub to a deck
				</button>
			</div>
		</div>

		<!-- Portrait sits on the right like a CV photo, and only when the
		     metadata chain actually found one — the cassette stack is the
		     identity when it hasn't, not a placeholder to fill. -->
		<div class="portrait-slot">
			{#if artist.image_url && !imgFailed}
				<img class="portrait" src={artist.image_url} alt="" loading="lazy"
					on:error={() => (imgFailed = true)}>
			{/if}
			<!-- Tapedeck is a listening journal, so the last word on what an
			     artist looks like belongs to whoever keeps it — not to whichever
			     provider answered first. -->
			{#if artist.artist_row_id}
				<button class="pic-edit" on:click={() => (editingPic = !editingPic)}>
					{artist.image_url ? 'Change picture' : 'Add a picture'}
				</button>
			{/if}
		</div>
	</div>

	{#if editingPic}
		<div class="panel" style="border-radius:18px;padding:16px;margin-bottom:14px;">
			<div style="display:flex;gap:10px;flex-wrap:wrap;align-items:center;">
				<input class="input" style="flex:1;min-width:240px;" bind:value={picUrl}
					placeholder="https://… or /api/v1/artists/img/{artist.artist_row_id}">
				<button class="btn btn-primary" on:click={() => savePicture(picUrl)}>Save</button>
				<label class="btn btn-secondary" style="cursor:pointer;">
					{picBusy ? 'Uploading…' : 'Upload…'}
					<input type="file" accept="image/*" hidden disabled={picBusy}
						on:change={(e) => uploadPicture(e, false)} />
				</label>
				<button class="btn btn-secondary" on:click={() => savePicture('')}>Clear</button>
				<button class="btn btn-secondary" on:click={() => (editingPic = false)}>Cancel</button>
			</div>
			<p style="font-size:12.5px;color:var(--td-muted);margin:10px 0 0;">
				Paste any image URL, or <strong>Upload…</strong> a file from this device
				(up to 8&nbsp;MB). To use the picture from your own Plex or Jellyfin,
				use <code>/api/v1/artists/img/{artist.artist_row_id}</code> — Tapedeck
				fetches it server-side, since the browser can't reach your media server.
			</p>
			<!-- Said plainly rather than left to be discovered: `artists` has no
			     user column, so this is not a per-account preference. -->
			<p style="font-size:12.5px;color:var(--td-muted-2);margin:8px 0 0;">
				Artist pictures are shared by everyone on this instance.
			</p>

			<!-- The wide one. Its own field rather than a crop of the portrait,
			     because a 16/11 tile and a square photo are different pictures —
			     cropping one to the other either pillarboxes or loses the head. -->
			<div style="margin-top:16px;padding-top:14px;border-top:1px solid var(--td-border);">
				<label for="wide" style="display:block;font-size:13px;margin-bottom:6px;">
					Wide picture
				</label>
				<div style="display:flex;gap:10px;flex-wrap:wrap;align-items:center;">
					<input id="wide" class="input" style="flex:1;min-width:240px;" bind:value={wideUrl}
						placeholder="https://… a landscape photo">
					<button class="btn btn-primary" on:click={() => saveWide(wideUrl)}>Save</button>
					<label class="btn btn-secondary" style="cursor:pointer;">
						{picBusy ? 'Uploading…' : 'Upload…'}
						<input type="file" accept="image/*" hidden disabled={picBusy}
							on:change={(e) => uploadPicture(e, true)} />
					</label>
					<button class="btn btn-secondary" on:click={() => saveWide('')}>Clear</button>
				</div>
				<p style="font-size:12.5px;color:var(--td-muted);margin:10px 0 0;">
					Used where an artist gets a whole tile — the top-artist card in a
					Reports chapter. Nothing fetches these automatically, so a chapter
					without one keeps the big initial.
				</p>
			</div>
		</div>
	{/if}
	{#if picMsg}
		<p style="font-size:13px;color:var(--td-good-fg);margin:0 0 12px;">{picMsg}</p>
	{/if}

	<StatStrip stats={artist.stats} />

	<BioCard bio={artist.bio} heading="About this artist" />

	{#if artist.trend.length > 1}
		<h3 class="h-display" style="font-size:20px;margin:0 0 12px;">Plays by year</h3>
		<div style="margin-bottom:14px;"><BarChart bars={artist.trend} /></div>
	{/if}

	<!--
		The Patch half of the design's artist screen. Not a separate page: its
		stats are the ones this page already computes, and a second copy would
		drift — so only the part that is genuinely about *other decks* is new.
	-->
	{#if otherDecks.length || myChains.length}
		<div class="pcols">
			{#if otherDecks.length}
				<div class="panel pcard">
					<h3 class="h-display" style="font-size:19px;margin:0 0 4px;">Who else here plays them</h3>
					<p class="psub">From decks on this instance with shared chains.</p>
					{#each otherDecks as d (d.user_id)}
						<div class="prow2">
							<span class="pav">
								{#if d.avatar_url}<img src={d.avatar_url} alt="" />{:else}{(d.display_name || d.username).slice(0, 2).toUpperCase()}{/if}
							</span>
							<div style="min-width:0;flex:1;">
								<div class="pname2">{d.display_name || d.username}</div>
								<div class="pmeta2">
									{d.plays} play{d.plays === 1 ? '' : 's'}
									{#if d.patched}· patched{/if}
								</div>
							</div>
							<!--
								A patch is what makes a deck readable, so the button
								says which of the two situations this is rather than
								offering a link that would 404.
							-->
							{#if d.patched}
								<a class="btn btn-secondary pbtn" href="/users/{d.username}">Open deck</a>
							{:else if asked.has(d.username)}
								<span class="tag">Asked</span>
							{:else}
								<button class="btn btn-secondary pbtn" on:click={() => askPatch(d.username)}>
									Ask to patch
								</button>
							{/if}
						</div>
					{/each}
				</div>
			{/if}

			{#if myChains.length}
				<div class="panel pcard">
					<h3 class="h-display" style="font-size:19px;margin:0 0 4px;">Chains you hear them on</h3>
					<p class="psub">Your own listening — every chain, shared or not.</p>
					{#each myChains as c (c.name)}
						<div class="crow">
							<div style="flex:1;min-width:0;">
								<div class="pname2">{c.name}</div>
								<div class="cbar">
									<div style="width:{Math.min(100, (c.plays / (myChains[0]?.plays || 1)) * 100)}%"></div>
								</div>
							</div>
							<span class="pmeta2">{c.plays}</span>
						</div>
					{/each}
				</div>
			{/if}
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

{#if artist}
	<DubDialog bind:open={dubOpen} kind="artist" artist={artist.name} name={null} />
{/if}

<style>
	.pcols {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
		gap: 18px;
		margin-bottom: 30px;
	}
	.pcard { border-radius: 22px; padding: 20px; }
	.psub { margin: 0 0 14px; font-size: 12.5px; color: var(--td-muted-2); }
	.prow2, .crow { display: flex; align-items: center; gap: 11px; padding: 9px 0; min-width: 0; }
	.prow2 + .prow2, .crow + .crow { border-top: 1px solid var(--td-border); }
	.pav {
		width: 34px;
		height: 34px;
		border-radius: 50%;
		background: var(--td-surface2);
		color: var(--td-muted-2);
		display: flex;
		align-items: center;
		justify-content: center;
		font: 700 11px ui-monospace, SFMono-Regular, Menlo, monospace;
		flex: none;
		overflow: hidden;
	}
	.pav img { width: 100%; height: 100%; object-fit: cover; }
	.pname2 { font-size: 13.5px; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.pmeta2 { font-size: 12px; color: var(--td-muted-2); font-variant-numeric: tabular-nums; }
	.pbtn { font-size: 12.5px; padding: 7px 14px; white-space: nowrap; flex: none; text-decoration: none; }
	.cbar { height: 5px; border-radius: 999px; background: var(--td-track); overflow: hidden; margin-top: 5px; }
	.cbar > div { height: 100%; border-radius: 999px; background: linear-gradient(90deg, var(--td-accent-2), var(--td-accent)); }

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
	.portrait {
		width: 168px; height: 168px; flex: none; object-fit: cover;
		border-radius: 20px; background: var(--td-track);
		border: 1px solid var(--td-border);
	}
	.portrait-slot { display: flex; flex-direction: column; gap: 8px; align-items: center; flex: none; }
	/* Quiet until you go looking for it — this is a correction, not a
	   call to action. */
	.pic-edit {
		border: none; background: none; padding: 0; cursor: pointer;
		font: inherit; font-size: 12px; color: var(--td-muted-2);
		text-decoration: underline; text-underline-offset: 3px;
	}
	.pic-edit:hover { color: var(--td-accent); }
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
