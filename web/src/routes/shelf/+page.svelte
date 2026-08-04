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
	//
	// Two ways in, because there are two moments you might reach for this. If
	// you press *start* when you drop the needle, the start time is observed and
	// every track lands where it actually played. If you only think of it once
	// the side has finished, "already played" works it backwards from the
	// running time — right ordering, right gaps, but a start time nobody saw.
	//
	// The spinning state lives in localStorage rather than on the server: a
	// side in progress is a fact about *this* browser sitting in front of the
	// turntable, and syncing it would mean a phone in another room claiming the
	// record is on. It survives a refresh, which is the only durability it
	// needs.
	let open: PhysicalRelease | null = null;
	let openTracks: PhysicalTrack[] = [];
	let playing = false;
	let flipPrompt = '';
	let playMsg = '';

	type Spinning = { releaseId: number; side: string; startedAt: number };
	let spinning: Spinning | null = null;
	let nowSec = Math.floor(Date.now() / 1000);

	const SPIN_KEY = 'td-spinning';

	function loadSpinning() {
		try {
			const raw = localStorage.getItem(SPIN_KEY);
			spinning = raw ? (JSON.parse(raw) as Spinning) : null;
		} catch {
			spinning = null;
		}
	}
	function saveSpinning(v: Spinning | null) {
		spinning = v;
		try {
			if (v) localStorage.setItem(SPIN_KEY, JSON.stringify(v));
			else localStorage.removeItem(SPIN_KEY);
		} catch { /* private mode — the state is still live in memory */ }
	}

	function startSide(side: string) {
		if (!open) return;
		playMsg = '';
		flipPrompt = '';
		saveSpinning({ releaseId: open.id, side, startedAt: Math.floor(Date.now() / 1000) });
	}

	/** Elapsed and remaining for the side currently on. */
	$: spinElapsed = spinning ? Math.max(0, nowSec - spinning.startedAt) : 0;
	$: spinTotal = spinning ? sideSecs(spinning.side, spinRelease) : 0;
	$: spinLeft = Math.max(0, spinTotal - spinElapsed);
	$: spinPct = spinTotal ? Math.min(100, (spinElapsed / spinTotal) * 100) : 0;
	/** Is the side that's on the one belonging to the record being shown? */
	$: spinHere = !!(spinning && open && spinning.releaseId === open.id);
	/** The record that's on, whichever one it is. */
	$: spinRelease = spinning ? shelf.find((r) => r.id === spinning?.releaseId) ?? null : null;
	/** Something is on, but you're looking at a different record. Without this
	 *  the deck is invisible and every Start button is dead for no stated
	 *  reason — which is exactly what it looked like in testing. */
	$: spinElsewhere = !!(spinning && !spinHere);

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

	async function play(side: string, startedAt?: number) {
		if (!open) return;
		playing = true;
		playMsg = '';
		try {
			const r = await api.playSide(open.id, { side, started_at: startedAt });
			playMsg = r.listens_added
				? `Side ${side} — ${r.listens_added} listen${r.listens_added === 1 ? '' : 's'} recorded.`
				: `Side ${side} was already recorded at that time.`;
			// The flip reminder is the whole point of tracking sides: you are
			// away from the screen while it plays, and the next thing you do is
			// turn it over.
			flipPrompt = r.next_side ?? '';
			saveSpinning(null);
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

	function sideSecs(side: string, release?: PhysicalRelease | null): number {
		// Defaults to the open record's tracks, but takes any record — the
		// countdown has to keep running while you browse the rest of the shelf.
		let list = openTracks;
		if (release && release.id !== open?.id) {
			try {
				list = JSON.parse(release.tracks) as PhysicalTrack[];
			} catch {
				list = [];
			}
		}
		return list.filter((t) => t.side === side).reduce((n, t) => n + (t.duration_secs ?? 180), 0);
	}

	onMount(() => {
		loadSpinning();
		// Drives the countdown and the "side is over" prompt. One second is the
		// right resolution for a twenty-minute side and costs nothing.
		const tick = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 1000);
		load();
		return () => clearInterval(tick);
	});
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

{#if spinElsewhere && spinning}
	<button class="nowon" on:click={() => spinRelease && openRelease(spinRelease)}>
		<span class="pip"></span>
		<span style="flex:1;min-width:0;text-align:left;">
			<strong>Side {spinning.side} is on</strong>
			{#if spinRelease}— {spinRelease.title}{/if}
			<span style="color:var(--td-muted);font-variant-numeric:tabular-nums;">
				· {formatDuration(spinElapsed)} / {formatDuration(spinTotal)}
			</span>
		</span>
		<span style="color:var(--td-muted-2);font-size:12.5px;">Open it →</span>
	</button>
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
			{#if spinHere && spinning}
				<div class="deck" class:done={spinLeft === 0}>
					{#if open.kind === 'vinyl'}
						<!-- A platter, turning at 33⅓ — one revolution every 1.8s,
						     which is the real speed rather than a decorative one. -->
						<svg viewBox="0 0 80 80" width="76" height="76" class="platter" aria-hidden="true">
							<circle cx="40" cy="40" r="38" fill="#201e1d" />
							<circle cx="40" cy="40" r="38" fill="none" stroke="var(--td-accent)" stroke-width="1.5" opacity=".5" />
							{#each [30, 24, 18] as r}
								<circle cx="40" cy="40" r={r} fill="none" stroke="#3a3532" stroke-width="1" />
							{/each}
							<circle cx="40" cy="40" r="13" fill="var(--td-accent)" />
							<circle cx="40" cy="40" r="2.5" fill="#201e1d" />
							<!-- The mark that makes the rotation visible at all. -->
							<rect x="39" y="4" width="2" height="10" rx="1" fill="var(--td-accent-hi, #e8a06a)" />
						</svg>
					{:else}
						<!-- Two reels, the take-up one filling as the tape runs. -->
						<svg viewBox="0 0 100 60" width="92" height="56" aria-hidden="true">
							<rect x="2" y="6" width="96" height="48" rx="7" fill="#201e1d"
								stroke="var(--td-accent)" stroke-width="1.5" opacity=".9" />
							<rect x="18" y="18" width="64" height="24" rx="4" fill="#15130f" />
							{#each [32, 68] as cx}
								<g class="reel" style="transform-origin:{cx}px 30px;">
									<circle cx={cx} cy="30" r="10" fill="none" stroke="#6a6259" stroke-width="2.5" />
									<circle cx={cx} cy="30" r="4.5" fill="var(--td-accent)" />
									{#each [0, 60, 120] as a}
										<rect x={cx - 0.9} y="21" width="1.8" height="7" rx="0.9" fill="#8a8078"
											style="transform-origin:{cx}px 30px;transform:rotate({a}deg);" />
									{/each}
								</g>
							{/each}
						</svg>
					{/if}

					<div style="flex:1;min-width:0;">
						<div style="font-size:11px;letter-spacing:.16em;text-transform:uppercase;color:#c67139;font-weight:600;">
							{spinLeft === 0 ? 'Side finished' : `Side ${spinning.side} playing`}
						</div>
						<div class="h-display" style="font-size:19px;margin-top:2px;">
							{open.title}
						</div>
						<div style="font-variant-numeric:tabular-nums;font-size:13px;color:var(--td-muted);margin-top:4px;">
							{formatDuration(spinElapsed)} / {formatDuration(spinTotal)}
							{#if spinLeft > 0}· {formatDuration(spinLeft)} left{/if}
						</div>
						<div style="height:6px;border-radius:999px;background:var(--td-track);overflow:hidden;margin-top:8px;">
							<div style="height:100%;width:{spinPct}%;border-radius:999px;background:linear-gradient(90deg,var(--td-accent-2),var(--td-accent-hi));transition:width 1s linear;"></div>
						</div>
					</div>

					<div style="display:flex;flex-direction:column;gap:6px;">
						<button class="btn btn-primary" disabled={playing}
							on:click={() => spinning && play(spinning.side, spinning.startedAt)}>
							{playing ? '…' : spinLeft === 0 ? 'Record it' : 'Stop & record'}
						</button>
						<button class="btn btn-secondary" on:click={() => saveSpinning(null)}>Cancel</button>
					</div>
				</div>
			{/if}

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
						<div style="display:flex;gap:6px;margin-top:12px;">
							<button class="btn btn-primary" style="flex:1;"
								disabled={playing || !!spinning} on:click={() => startSide(side)}>
								Start side {side}
							</button>
							<button class="btn btn-secondary" title="Record it now, working the start time back from the running time"
								disabled={playing || !!spinning} on:click={() => play(side)}>
								Already played
							</button>
						</div>
					</div>
				{/each}
			</div>

			<p style="font-size:12px;color:var(--td-muted-2);margin:16px 0 0;">
				<strong>Start</strong> when you drop the needle and the start time is the
				real one. <strong>Already played</strong> works it backwards from the
				running time instead — the order and the gaps are right, but no one saw
				the clock.
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
	/* The deck only moves while a side is actually on — a still platter is the
	   honest signal that nothing is playing, the same rule the dashboard's reels
	   follow. */
	.deck {
		display: flex;
		gap: 16px;
		align-items: center;
		margin-top: 16px;
		padding: 14px 16px;
		border-radius: 16px;
		border: 1px solid var(--td-accent);
		background: color-mix(in srgb, var(--td-accent) 7%, transparent);
		flex-wrap: wrap;
	}
	.deck.done { border-style: dashed; }
	/* Shown when a side is on but you're looking elsewhere on the shelf. */
	.nowon {
		display: flex;
		align-items: center;
		gap: 10px;
		width: 100%;
		margin: 16px 0 0;
		padding: 10px 14px;
		border-radius: 12px;
		border: 1px solid var(--td-accent);
		background: color-mix(in srgb, var(--td-accent) 7%, transparent);
		color: inherit;
		font-size: 13.5px;
		cursor: pointer;
	}
	.pip {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--td-accent);
		animation: td-blink 1.6s ease-in-out infinite;
		flex: none;
	}
	@media (prefers-reduced-motion: reduce) { .pip { animation: none; } }
	/* 33⅓ rpm is one turn per 1.8s. Using the real speed costs nothing and is
	   the sort of detail this app is for. */
	.platter { animation: td-spin 1.8s linear infinite; }
	.deck :global(.reel) { animation: td-spin 2.4s linear infinite; }
	.deck.done .platter,
	.deck.done :global(.reel) { animation-play-state: paused; }
	@media (prefers-reduced-motion: reduce) {
		.platter, .deck :global(.reel) { animation: none; }
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
