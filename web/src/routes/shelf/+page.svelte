<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type PhysicalRelease, type PhysicalTrack, type ReleaseCandidate } from '$lib/api';
	import { formatDuration, formatDateTime } from '$lib/utils';
	import CoverArt from '$lib/components/CoverArt.svelte';

	// Records and tapes are one screen, not two: the workflow is identical and
	// most shelves hold both. The filter is a view, not a mode.
	type Filter = 'all' | 'vinyl' | 'cassette';
	let filter: Filter = 'all';

	let shelf: PhysicalRelease[] = [];
	let loading = true;
	let error = '';

	// ── Adding something ──
	let adding = false;
	let lookupKind: 'vinyl' | 'cassette' = 'vinyl';
	let barcode = '', catno = '', qArtist = '', qRelease = '';
	let searching = false;
	let candidates: ReleaseCandidate[] = [];
	let searchNotes: string[] = [];
	let discogsEnabled = false;
	let searched = false;

	// ── Playing a side ──
	let open: PhysicalRelease | null = null;
	let openTracks: PhysicalTrack[] = [];
	let playing = false;
	let flipPrompt = '';
	let playMsg = '';

	$: sides = [...new Set(openTracks.map((t) => t.side))].sort();
	$: shown = shelf.filter((r) => filter === 'all' || r.kind === filter);

	async function load() {
		loading = true;
		try {
			shelf = (await api.listShelf()).releases;
			error = '';
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		loading = false;
	}

	async function search() {
		searching = true;
		candidates = [];
		searchNotes = [];
		searched = false;
		try {
			const r = await api.lookupPressing({
				barcode, catalog_number: catno, artist: qArtist, release: qRelease, kind: lookupKind
			});
			candidates = r.candidates;
			searchNotes = r.notes;
			discogsEnabled = r.discogs_enabled;
			searched = true;
		} catch (e) {
			searchNotes = [String(e).replace(/^API \d+: /, '')];
			searched = true;
		}
		searching = false;
	}

	/** A Discogs search hit carries no tracklist — fetch it before shelving. */
	async function shelve(c: ReleaseCandidate) {
		let full = c;
		if (c.source === 'discogs' && !c.tracks.length && c.discogs_id) {
			try {
				full = (await api.discogsRelease(c.discogs_id)).candidate;
			} catch {
				// Shelve it anyway: a record with no tracklist is still a record
				// you own, and the sides can be typed in later.
			}
		}
		try {
			await api.addPressing({ ...full, kind: lookupKind, tracks: full.tracks ?? [] });
			adding = false;
			candidates = [];
			barcode = catno = qArtist = qRelease = '';
			await load();
		} catch (e) {
			searchNotes = [String(e).replace(/^API \d+: /, '')];
		}
	}

	function openRelease(r: PhysicalRelease) {
		open = r;
		playMsg = '';
		flipPrompt = '';
		try {
			openTracks = JSON.parse(r.tracks) as PhysicalTrack[];
		} catch {
			openTracks = [];
		}
	}

	async function play(side: string) {
		if (!open) return;
		playing = true;
		playMsg = '';
		try {
			const r = await api.playSide(open.id, { side });
			playMsg = r.listens_added
				? `Side ${side} — ${r.listens_added} listen${r.listens_added === 1 ? '' : 's'} recorded.`
				: `Side ${side} was already recorded at that time.`;
			// The flip reminder is the whole point of tracking sides: you are
			// away from the screen while it plays, and the next thing you do is
			// turn it over.
			flipPrompt = r.next_side ?? '';
			await load();
			const fresh = shelf.find((x) => x.id === open?.id);
			if (fresh) open = fresh;
		} catch (e) {
			playMsg = String(e).replace(/^API \d+: /, '');
		}
		playing = false;
	}

	async function remove(r: PhysicalRelease) {
		if (!confirm(`Remove “${r.title}” from the shelf? Listens already recorded are kept.`)) return;
		try {
			await api.deletePressing(r.id);
			if (open?.id === r.id) open = null;
			await load();
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
	}

	function sideSecs(side: string): number {
		return openTracks
			.filter((t) => t.side === side)
			.reduce((n, t) => n + (t.duration_secs ?? 180), 0);
	}

	onMount(load);
</script>

<svelte:head><title>Vinyl & Tape — Tapedeck</title></svelte:head>

<div style="display:flex;align-items:flex-end;justify-content:space-between;gap:16px;flex-wrap:wrap;margin-bottom:6px;">
	<div>
		<h1 class="h-display" style="font-size:34px;margin:0;">Vinyl &amp; Tape</h1>
		<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;">
			What you own, and what you actually put on. Playing a side records every
			track on it.
		</p>
	</div>
	<button class="btn btn-primary" on:click={() => (adding = !adding)}>
		{adding ? 'Cancel' : 'Add a record'}
	</button>
</div>

{#if adding}
	<div class="panel" style="border-radius:22px;padding:24px;margin:18px 0;">
		<div style="display:flex;gap:8px;margin-bottom:16px;">
			{#each ['vinyl', 'cassette'] as k}
				<button class="chip" class:on={lookupKind === k} on:click={() => (lookupKind = k as 'vinyl' | 'cassette')}>
					{k === 'vinyl' ? 'Vinyl' : 'Cassette'}
				</button>
			{/each}
		</div>

		<div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(200px,1fr));gap:12px;">
			<div class="field">
				<label for="f-bar">Barcode</label>
				<input id="f-bar" class="input" bind:value={barcode} placeholder="724358213614"
					on:keydown={(e) => e.key === 'Enter' && search()}>
			</div>
			<div class="field">
				<label for="f-cat">Catalogue number</label>
				<input id="f-cat" class="input" bind:value={catno} placeholder="SHVL 804"
					on:keydown={(e) => e.key === 'Enter' && search()}>
			</div>
			<div class="field">
				<label for="f-art">Artist</label>
				<input id="f-art" class="input" bind:value={qArtist} placeholder="Pink Floyd"
					on:keydown={(e) => e.key === 'Enter' && search()}>
			</div>
			<div class="field">
				<label for="f-rel">Release</label>
				<input id="f-rel" class="input" bind:value={qRelease} placeholder="The Wall"
					on:keydown={(e) => e.key === 'Enter' && search()}>
			</div>
		</div>

		<div style="display:flex;align-items:center;gap:12px;margin-top:14px;flex-wrap:wrap;">
			<button class="btn btn-primary" on:click={search} disabled={searching}>
				{searching ? 'Looking…' : 'Look it up'}
			</button>
			<span style="font-size:12.5px;color:var(--td-muted);">
				A barcode is the surest match. Older pressings often have only a
				catalogue number.
			</span>
		</div>

		{#if searched && discogsEnabled === false}
			<p style="font-size:12.5px;color:var(--td-muted-2);margin:12px 0 0;">
				Searching MusicBrainz only — Discogs isn't configured on this server, and
				it's usually better at identifying a specific pressing from a barcode.
				An admin can add a token in Settings.
			</p>
		{/if}

		{#each searchNotes as n}
			<p style="font-size:13px;color:#b2622d;margin:10px 0 0;">{n}</p>
		{/each}

		{#if candidates.length}
			<div style="margin-top:18px;">
				{#each candidates as c}
					<div class="cand">
						<div style="flex:1;min-width:0;">
							<div style="font-weight:600;font-size:15px;">{c.artist} — {c.title}</div>
							<div style="font-size:12.5px;color:var(--td-muted);margin-top:3px;">
								{[c.year, c.country, c.format, c.catalog_number].filter(Boolean).join(' · ')}
								· {c.tracks.length ? `${c.tracks.length} tracks` : 'tracklist fetched on add'}
								· <span style="text-transform:capitalize;">{c.source}</span>
							</div>
						</div>
						<button class="btn btn-secondary" on:click={() => shelve(c)}>Add</button>
					</div>
				{/each}
			</div>
		{:else if searched && !searchNotes.length}
			<p style="font-size:13px;color:var(--td-muted);margin:14px 0 0;">
				Nothing found. Try the catalogue number from the spine, or just the artist
				and release.
			</p>
		{/if}
	</div>
{/if}

<div style="display:flex;gap:8px;margin:18px 0;">
	{#each ['all', 'vinyl', 'cassette'] as f}
		<button class="chip" class:on={filter === f} on:click={() => (filter = f as Filter)}>
			{f === 'all' ? 'Everything' : f === 'vinyl' ? 'Vinyl' : 'Cassettes'}
		</button>
	{/each}
</div>

{#if error}
	<p style="color:#b2622d;font-size:14px;">{error}</p>
{/if}

{#if loading}
	<p style="color:var(--td-muted);font-size:14px;">Reading the shelf…</p>
{:else if !shown.length}
	<div class="panel" style="border-radius:22px;padding:32px;text-align:center;">
		<p style="margin:0 0 6px;font-size:15px;">Nothing on the shelf yet.</p>
		<p style="margin:0;color:var(--td-muted);font-size:13.5px;">
			Add a record or tape and Tapedeck will record each side you play.
		</p>
	</div>
{:else}
	<div style="display:grid;grid-template-columns:repeat(auto-fill,minmax(260px,1fr));gap:14px;">
		{#each shown as r (r.id)}
			<button class="card" on:click={() => openRelease(r)} class:open={open?.id === r.id}>
				<CoverArt url={r.artwork_url} caaId={null} releaseMbid={r.mbid_release}
					seed={`${r.artist} ${r.title}`} size={54} radius={r.kind === 'vinyl' ? 999 : 8} />
				<div style="flex:1;min-width:0;text-align:left;">
					<div style="font-weight:600;font-size:14.5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">{r.title}</div>
					<div style="font-size:12.5px;color:var(--td-muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">{r.artist}</div>
					<div style="font-size:11.5px;color:var(--td-muted-2);margin-top:5px;">
						{r.kind === 'vinyl' ? '33⅓' : 'Tape'}
						{#if r.play_count}
							· {r.play_count} side{r.play_count === 1 ? '' : 's'} · {formatDuration(r.played_secs)}
						{:else}
							· never played
						{/if}
					</div>
				</div>
			</button>
		{/each}
	</div>
{/if}

{#if open}
	<div class="panel" style="border-radius:22px;padding:24px;margin-top:20px;">
		<div style="display:flex;justify-content:space-between;gap:16px;flex-wrap:wrap;align-items:flex-start;">
			<div>
				<h3 class="h-display" style="font-size:21px;margin:0;">{open.title}</h3>
				<p style="margin:4px 0 0;color:var(--td-muted);font-size:14px;">
					{open.artist}{open.year ? ` · ${open.year}` : ''}
					{open.catalog_number ? ` · ${open.catalog_number}` : ''}
					{open.barcode ? ` · ${open.barcode}` : ''}
				</p>
				{#if open.last_played}
					<p style="margin:4px 0 0;color:var(--td-muted-2);font-size:12.5px;">
						Last played {formatDateTime(open.last_played)}
					</p>
				{/if}
			</div>
			<div style="display:flex;gap:8px;">
				<button class="btn btn-secondary" on:click={() => (open = null)}>Close</button>
				<button class="btn btn-secondary" on:click={() => open && remove(open)}>Remove</button>
			</div>
		</div>

		{#if !openTracks.length}
			<p style="font-size:13.5px;color:var(--td-muted);margin:16px 0 0;">
				No tracklist stored for this one, so there is nothing to record. Re-add it
				from a lookup that has one.
			</p>
		{:else}
			{#if flipPrompt}
				<div class="flip">
					<span style="font-size:20px;">↻</span>
					<div>
						<div style="font-weight:600;font-size:14.5px;">Flip it — side {flipPrompt} is next</div>
						<div style="font-size:12.5px;color:var(--td-muted);">
							{formatDuration(sideSecs(flipPrompt))} on that side.
						</div>
					</div>
					<button class="btn btn-primary" style="margin-left:auto;" disabled={playing}
						on:click={() => play(flipPrompt)}>Play side {flipPrompt}</button>
				</div>
			{/if}

			{#if playMsg}
				<p style="font-size:13px;color:var(--td-good-fg);margin:14px 0 0;">{playMsg}</p>
			{/if}

			<div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(240px,1fr));gap:14px;margin-top:18px;">
				{#each sides as side}
					<div class="side">
						<div style="display:flex;align-items:baseline;justify-content:space-between;gap:10px;">
							<span class="h-display" style="font-size:17px;">Side {side}</span>
							<span style="font-size:12px;color:var(--td-muted-2);">
								{formatDuration(sideSecs(side))}
							</span>
						</div>
						<ol style="margin:10px 0 0;padding-left:18px;">
							{#each openTracks.filter((t) => t.side === side) as t}
								<li style="font-size:13px;padding:2px 0;">
									{t.title}
									{#if t.duration_secs}
										<span style="color:var(--td-muted-2);">· {formatDuration(t.duration_secs)}</span>
									{/if}
								</li>
							{/each}
						</ol>
						<button class="btn btn-primary" style="margin-top:12px;width:100%;"
							disabled={playing} on:click={() => play(side)}>
							{playing ? '…' : `Played side ${side}`}
						</button>
					</div>
				{/each}
			</div>

			<p style="font-size:12px;color:var(--td-muted-2);margin:16px 0 0;">
				Press it when the side finishes. Each track is recorded at the time it
				would have played, worked out from the running order — you know when the
				side started, not when track three did.
			</p>
		{/if}
	</div>
{/if}

<style>
	.chip {
		border: 1px solid var(--td-line);
		background: transparent;
		color: var(--td-muted);
		border-radius: 999px;
		padding: 7px 16px;
		font-size: 13px;
		font-weight: 600;
		cursor: pointer;
	}
	.chip.on {
		background: var(--td-accent);
		border-color: var(--td-accent);
		color: #fff;
	}
	.card {
		display: flex;
		gap: 12px;
		align-items: center;
		padding: 14px;
		border-radius: 16px;
		border: 1px solid var(--td-line);
		background: var(--td-panel);
		cursor: pointer;
		text-align: left;
		transition: border-color 0.15s, transform 0.15s;
	}
	.card:hover { transform: translateY(-1px); }
	.card.open { border-color: var(--td-accent); }
	.cand {
		display: flex;
		gap: 12px;
		align-items: center;
		padding: 12px 0;
		border-top: 1px solid var(--td-line);
	}
	.side {
		border: 1px solid var(--td-line);
		border-radius: 14px;
		padding: 14px;
	}
	/* The flip reminder is the one thing you look for after walking back to the
	   deck, so it gets accent treatment rather than sitting in the list. */
	.flip {
		display: flex;
		gap: 12px;
		align-items: center;
		margin-top: 16px;
		padding: 12px 14px;
		border-radius: 14px;
		border: 1px solid var(--td-accent);
		background: color-mix(in srgb, var(--td-accent) 8%, transparent);
		flex-wrap: wrap;
	}
</style>
