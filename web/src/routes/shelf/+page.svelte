<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type PhysicalRelease, type PhysicalTrack, type ReleaseCandidate } from '$lib/api';
	import { formatDuration, formatDateTime } from '$lib/utils';
	import CoverArt from '$lib/components/CoverArt.svelte';

	// Records and tapes are one screen, not two: the workflow is identical and
	// most shelves hold both. The filter is a view, not a mode.
	type Filter = 'all' | 'vinyl' | 'cassette' | 'cd' | 'sacd';
	type Kind = 'vinyl' | 'cassette' | 'cd' | 'sacd';

	/** A disc has no side to flip — it plays start to finish. Everything that
	 *  says "Side A" or offers to remind you to turn it over is gated on this. */
	const isDisc = (k: string) => k === 'cd' || k === 'sacd';
	const KIND_LABEL: Record<string, string> = {
		vinyl: 'Vinyl', cassette: 'Cassette', cd: 'CD', sacd: 'SACD'
	};

	/** What one playable unit of this medium is called. A record and a tape have
	 *  sides you flip; a disc has none, so calling its tracklist "Side 1" would
	 *  be describing hardware it doesn't have. */
	const unit = (k: string) => (isDisc(k) ? 'Disc' : 'Side');
	let filter: Filter = 'all';

	let shelf: PhysicalRelease[] = [];
	let loading = true;
	let error = '';

	// ── Adding something ──
	let adding = false;
	let lookupKind: Kind = 'vinyl';
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

	/**
	 * A side in progress.
	 *
	 * `outcomes` is the record of what actually happened — the browser is the
	 * only thing running the clock, so it is the only thing that sees a pause
	 * or a skip. It is sent with the play and the server takes it as
	 * authoritative, rather than laying tracks out from a start time, which can
	 * only ever describe an uninterrupted side.
	 *
	 * `pausedAt` is null while running. `pausedTotal` accumulates, so elapsed
	 * is wall clock minus time spent stopped — a paused record is not playing,
	 * and counting it would put every later track minutes late.
	 */
	type Outcome = { position: number; started_at: number; skipped: boolean };
	type Spinning = {
		releaseId: number;
		/** The medium, so the "… is on" banner can name the unit without having
		 *  the record loaded — you can be on another page when it shows. Absent
		 *  on a spin started before this field existed, hence the fallback. */
		kind?: string;
		side: string;
		startedAt: number;
		pausedAt: number | null;
		pausedTotal: number;
		outcomes: Outcome[];
	};
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
		const now = Math.floor(Date.now() / 1000);
		saveSpinning({
			releaseId: open.id, kind: open.kind, side, startedAt: now,
			pausedAt: null, pausedTotal: 0,
			// The first track starts the moment the needle drops.
			outcomes: [{ position: sideTracks(side)[0]?.position ?? 1, started_at: now, skipped: false }]
		});
	}

	/** Tracks on a side, in order. */
	function sideTracks(side: string, release?: PhysicalRelease | null): PhysicalTrack[] {
		let list = openTracks;
		if (release && release.id !== open?.id) {
			try { list = JSON.parse(release.tracks) as PhysicalTrack[]; } catch { list = []; }
		}
		return list.filter((t) => t.side === side).sort((a, b) => a.position - b.position);
	}

	function togglePause() {
		if (!spinning) return;
		const now = Math.floor(Date.now() / 1000);
		if (spinning.pausedAt === null) {
			saveSpinning({ ...spinning, pausedAt: now });
		} else {
			// Resuming: everything after this shifts by however long it stood
			// still, which is what `pausedTotal` feeds into `spinElapsed`.
			saveSpinning({
				...spinning,
				pausedTotal: spinning.pausedTotal + (now - spinning.pausedAt),
				pausedAt: null
			});
		}
	}

	/** Which track the elapsed clock is currently inside. */
	function currentIndex(sp: Spinning): number {
		const list = sideTracks(sp.side, spinRelease);
		let acc = 0;
		for (let i = 0; i < list.length; i++) {
			acc += list[i].duration_secs ?? 180;
			if (spinElapsed < acc) return i;
		}
		return Math.max(0, list.length - 1);
	}

	/**
	 * Skip the track that's playing.
	 *
	 * It is still recorded — `scrobbles.skipped` exists precisely so skip-rate
	 * analytics has something to count — but marked, so it stays out of every
	 * total and is never forwarded. A skip on someone's Last.fm would be a
	 * wrong scrobble on a permanent record.
	 */
	function skipTrack() {
		if (!spinning) return;
		const list = sideTracks(spinning.side, spinRelease);
		const i = currentIndex(spinning);
		const cur = list[i];
		if (!cur) return;
		const now = Math.floor(Date.now() / 1000);

		const outcomes = spinning.outcomes.filter((o) => o.position !== cur.position);
		outcomes.push({
			position: cur.position,
			started_at: spinning.outcomes.find((o) => o.position === cur.position)?.started_at ?? now,
			skipped: true
		});
		const next = list[i + 1];
		if (next && !outcomes.some((o) => o.position === next.position)) {
			outcomes.push({ position: next.position, started_at: now, skipped: false });
		}

		// Jumping the clock forward past what's left of this track is what makes
		// the next one current — the skip cost you the remainder, not the whole.
		const played = list.slice(0, i + 1).reduce((n, t) => n + (t.duration_secs ?? 180), 0);
		const shift = Math.max(0, played - spinElapsed);
		saveSpinning({
			...spinning,
			outcomes,
			pausedTotal: spinning.pausedTotal - shift
		});
	}

	/** Elapsed and remaining for the side currently on. */
	// Wall clock minus time spent stopped. A paused record is not playing.
	$: spinElapsed = spinning
		? Math.max(0, (spinning.pausedAt ?? nowSec) - spinning.startedAt - spinning.pausedTotal)
		: 0;
	$: paused = !!spinning?.pausedAt;
	$: nowTrack = spinning && spinHere ? sideTracks(spinning.side)[currentIndex(spinning)] : null;
	$: skippedSoFar = spinning?.outcomes.filter((o) => o.skipped).length ?? 0;
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
			const r = await api.playSide(open.id, {
				side,
				started_at: startedAt,
				// Only when this deck ran the side; "already played" has none.
				tracks: spinning && spinning.releaseId === open.id ? spinning.outcomes : undefined
			});
			const sk = r.skipped ?? 0;
			playMsg = r.listens_added || sk
				? `${unit(open?.kind ?? 'vinyl')} ${side} — ${r.listens_added} listen${r.listens_added === 1 ? '' : 's'} recorded` +
					(sk ? `, ${sk} skipped.` : '.')
				: `${unit(open?.kind ?? 'vinyl')} ${side} was already recorded at that time.`;
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

<svelte:head><title>The Shelf — Tapedeck</title></svelte:head>

<div style="display:flex;align-items:flex-end;justify-content:space-between;gap:16px;flex-wrap:wrap;margin-bottom:6px;">
	<div>
		<h1 class="h-display" style="font-size:34px;margin:0;">The Shelf</h1>
		<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;">
			Records, tapes and discs you own, and what you actually put on. Playing a
			side or a disc records every track on it.
		</p>
	</div>
	<button class="btn btn-primary" on:click={() => (adding = !adding)}>
		{adding ? 'Cancel' : 'Add to the shelf'}
	</button>
</div>

{#if adding}
	<div class="panel" style="border-radius:22px;padding:24px;margin:18px 0;">
		<div style="display:flex;gap:8px;margin-bottom:16px;">
			{#each ['vinyl', 'cassette', 'cd', 'sacd'] as k}
				<button class="chip" class:on={lookupKind === k} on:click={() => (lookupKind = k as Kind)}>
					{KIND_LABEL[k]}
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
			<strong>{unit(spinning.kind ?? 'vinyl')} {spinning.side} is on</strong>
			{#if spinRelease}— {spinRelease.title}{/if}
			<span style="color:var(--td-muted);font-variant-numeric:tabular-nums;">
				· {formatDuration(spinElapsed)} / {formatDuration(spinTotal)}
			</span>
		</span>
		<span style="color:var(--td-muted-2);font-size:12.5px;">Open it →</span>
	</button>
{/if}

<!-- The decks are on the page whether or not anything is on them. An empty
     shelf otherwise opens on a wall of text, and this is a screen about
     objects — the platter sitting still is also the honest signal that
     nothing is playing, which is the rule the dashboard's reels follow. -->
{#if !spinning}
	<div class="idle">
		<svg viewBox="0 0 120 120" width="86" height="86" aria-hidden="true">
			<circle cx="60" cy="60" r="57" fill="var(--td-accent)" opacity=".10" />
			<circle cx="60" cy="60" r="50" fill="#0d0a07" />
			{#each [42, 34, 26] as r}
				<circle cx="60" cy="60" r={r} fill="none" stroke="#2a241d" stroke-width="1" />
			{/each}
			<circle cx="60" cy="60" r="18" fill="var(--td-accent)" opacity=".75" />
			<circle cx="60" cy="60" r="2.6" fill="#f4e7cf" />
		</svg>
		<svg viewBox="0 0 100 60" width="96" height="58" aria-hidden="true">
			<rect x="2" y="6" width="96" height="48" rx="7" fill="#0d0a07"
				stroke="var(--td-accent)" stroke-width="1.5" opacity=".55" />
			<rect x="18" y="18" width="64" height="24" rx="4" fill="#15130f" />
			{#each [32, 68] as cx}
				<circle cx={cx} cy="30" r="10" fill="none" stroke="#6a6259" stroke-width="2.5" />
				<circle cx={cx} cy="30" r="4.5" fill="var(--td-accent)" opacity=".75" />
			{/each}
		</svg>
		<div style="flex:1;min-width:200px;">
			<div style="font-size:11px;letter-spacing:.16em;text-transform:uppercase;color:var(--td-muted-2);font-weight:600;">
				Nothing on either deck
			</div>
			<p style="margin:6px 0 0;font-size:13.5px;color:var(--td-muted);max-width:46ch;">
				Open a record below and press <strong>Start</strong> when you drop the
				needle. The deck runs the clock, so pauses and skips are recorded as
				they happen.
			</p>
		</div>
	</div>
{/if}

<div style="display:flex;gap:8px;margin:18px 0;">
	{#each ['all', 'vinyl', 'cassette', 'cd', 'sacd'] as f}
		<button class="chip" class:on={filter === f} on:click={() => (filter = f as Filter)}>
			{f === 'all' ? 'Everything' : f === 'cassette' ? 'Cassettes' : KIND_LABEL[f]}
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
						{r.kind === 'vinyl' ? '33⅓' : r.kind === 'cassette' ? 'Tape' : KIND_LABEL[r.kind] ?? r.kind}
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
				<div class="deck" class:done={spinLeft === 0} class:paused>
					{#if open.kind === 'vinyl'}
						<!-- The deck, drawn as the design has it: a platter turning at
						     a real 33⅓ — one revolution every 1.8s — and a tonearm that
						     swings down onto the record when a side starts and lifts
						     when it stops. The arm is the detail that makes it read as
						     a turntable rather than a spinning circle. -->
						<svg viewBox="0 0 150 130" width="132" height="114" class="rig" aria-hidden="true">
							<g class="platter" style="transform-origin:60px 66px;">
								<circle cx="60" cy="66" r="54" fill="#16120d" />
								<circle cx="60" cy="66" r="52" fill="#0d0a07" />
								{#each [44, 37, 30, 23] as r}
									<circle cx="60" cy="66" r={r} fill="none" stroke="#241f18" stroke-width="1" />
								{/each}
								<circle cx="60" cy="66" r="19" fill="var(--td-accent)" />
								<circle cx="60" cy="66" r="15" fill="none" stroke="#f4e7cf" stroke-width="1" opacity=".55" />
								<line x1="60" y1="51" x2="60" y2="58" stroke="#f4e7cf" stroke-width="1.6" stroke-linecap="round" />
								<circle cx="60" cy="66" r="2.4" fill="#f4e7cf" />
							</g>
							<g class="arm" class:down={!paused && spinLeft > 0}>
								<line x1="131" y1="30" x2="74" y2="48" stroke="#d9ccb0" stroke-width="3.4" stroke-linecap="round" />
								<line x1="80" y1="46" x2="69" y2="50" stroke="#e9dcc4" stroke-width="5.5" stroke-linecap="round" />
								<circle cx="68" cy="50.5" r="2.2" fill="var(--td-accent)" />
								<circle cx="132" cy="29" r="9" fill="#231e18" stroke="#4a4038" stroke-width="1.6" />
								<circle cx="139" cy="21" r="5" fill="#3a3128" />
							</g>
						</svg>
					{:else if isDisc(open.kind)}
						<!-- A disc, and it spins far faster than a record: CD transports
						     run 200–500 rpm, so a decorative 1.8s turn would read as a
						     slow-motion LP. The sheen is what says "not vinyl" at a
						     glance, since the silhouette is the same circle. -->
						<svg viewBox="0 0 120 120" width="104" height="104" aria-hidden="true">
							<defs>
								<linearGradient id="td-disc-sheen" x1="0" y1="0" x2="1" y2="1">
									<stop offset="0" stop-color="#7a8a5e" stop-opacity=".55" />
									<stop offset="0.45" stop-color="#c67139" stop-opacity=".2" />
									<stop offset="0.75" stop-color="#8ea3c0" stop-opacity=".45" />
									<stop offset="1" stop-color="#c67139" stop-opacity=".3" />
								</linearGradient>
							</defs>
							<g class="disc" style="transform-origin:60px 60px;">
								<circle cx="60" cy="60" r="56" fill="#1a1712" />
								<circle cx="60" cy="60" r="54" fill="url(#td-disc-sheen)" />
								<circle cx="60" cy="60" r="54" fill="none" stroke="#3a3128" stroke-width="1" />
								<circle cx="60" cy="60" r="20" fill="var(--td-surface)" opacity=".85" />
								<circle cx="60" cy="60" r="20" fill="none" stroke="#3a3128" stroke-width="1" />
								<circle cx="60" cy="60" r="8" fill="var(--td-panel)" />
								<!-- The mark that makes the rotation legible at all. -->
								<rect x="58.6" y="8" width="2.8" height="12" rx="1.4" fill="#f4e7cf" opacity=".7" />
							</g>
						</svg>
					{:else}
						<!-- Two reels behind the window, both turning — the take-up side
						     is what a tape actually shows you is happening. -->
						<svg viewBox="0 0 150 100" width="132" height="88" aria-hidden="true">
							<rect x="3" y="4" width="144" height="92" rx="13" fill="#1c1712"
								stroke="var(--td-accent)" stroke-width="1.2" opacity=".95" />
							<rect x="17" y="12" width="116" height="24" rx="5" fill="#f4e7cf" />
							<line x1="25" y1="20" x2="125" y2="20" stroke="var(--td-accent)" stroke-width="2.4" opacity=".5" />
							<line x1="25" y1="28" x2="98" y2="28" stroke="#7a8a5e" stroke-width="2.4" opacity=".45" />
							<rect x="28" y="44" width="94" height="44" rx="7" fill="#14100b" />
							<rect x="48" y="59" width="54" height="15" rx="2" fill="#332a20" />
							{#each [50, 100] as cx}
								<g class="reel" style="transform-origin:{cx}px 66px;">
									<circle cx={cx} cy="66" r="15" fill="none" stroke="#e9dcc4" stroke-width="1.8" />
									<circle cx={cx} cy="66" r="9.5" fill="none" stroke="#7a6f5c" stroke-width="1.4" opacity=".6" />
									<circle cx={cx} cy="66" r="3.5" fill="#e9dcc4" />
									<line x1={cx - 15} y1="66" x2={cx + 15} y2="66" stroke="#e9dcc4" stroke-width="1.4" />
									<line x1={cx - 7.5} y1="53" x2={cx + 7.5} y2="79" stroke="#e9dcc4" stroke-width="1.4" />
									<line x1={cx - 7.5} y1="79" x2={cx + 7.5} y2="53" stroke="#e9dcc4" stroke-width="1.4" />
								</g>
							{/each}
						</svg>
					{/if}

					<div style="flex:1;min-width:0;">
						<div style="font-size:11px;letter-spacing:.16em;text-transform:uppercase;color:#c67139;font-weight:600;">
							{spinLeft === 0
								? `${unit(open.kind)} finished`
								: paused
									? `${unit(open.kind)} ${spinning.side} paused`
									: `${unit(open.kind)} ${spinning.side} playing`}
						</div>
						<div class="h-display" style="font-size:19px;margin-top:2px;">
							{nowTrack ? nowTrack.title : open.title}
						</div>
						{#if nowTrack}
							<div style="font-size:12.5px;color:var(--td-muted);">
								{open.title}{skippedSoFar ? ` · ${skippedSoFar} skipped` : ''}
							</div>
						{/if}
						<div style="font-variant-numeric:tabular-nums;font-size:13px;color:var(--td-muted);margin-top:4px;">
							{formatDuration(spinElapsed)} / {formatDuration(spinTotal)}
							{#if spinLeft > 0}· {formatDuration(spinLeft)} left{/if}
						</div>
						<div style="height:6px;border-radius:999px;background:var(--td-track);overflow:hidden;margin-top:8px;">
							<div style="height:100%;width:{spinPct}%;border-radius:999px;background:linear-gradient(90deg,var(--td-accent-2),var(--td-accent-hi));transition:width 1s linear;"></div>
						</div>
					</div>

					<div class="controls">
						<!-- Both decks get both controls. You can lift a needle and
						     you can fast-forward a tape; restricting either by
						     format would be arbitrary, and each has the same
						     consequence for what gets recorded. -->
						<button class="ctl" title={paused ? 'Resume' : 'Pause'}
							aria-label={paused ? 'Resume' : 'Pause'} on:click={togglePause}>
							{#if paused}
								<svg viewBox="0 0 24 24" width="18" fill="currentColor"><path d="M7 5l12 7-12 7z"/></svg>
							{:else}
								<svg viewBox="0 0 24 24" width="18" fill="currentColor"><rect x="6" y="5" width="4" height="14" rx="1"/><rect x="14" y="5" width="4" height="14" rx="1"/></svg>
							{/if}
						</button>
						<button class="ctl" title="Skip this track" aria-label="Skip this track"
							on:click={skipTrack} disabled={paused}>
							<svg viewBox="0 0 24 24" width="18" fill="currentColor"><path d="M5 5l10 7-10 7z"/><rect x="16" y="5" width="3" height="14" rx="1"/></svg>
						</button>
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
							<span class="h-display" style="font-size:17px;">{unit(open.kind)} {side}</span>
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
								Start {unit(open.kind).toLowerCase()} {side}
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
	.idle {
		display: flex; align-items: center; gap: 20px; flex-wrap: wrap;
		margin-top: 18px; padding: 18px 20px;
		border: 1px dashed var(--td-line); border-radius: 18px;
	}
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
	/* A CD transport runs 200–500 rpm, an order of magnitude past a record.
	   Drawn at 0.5s a turn: still legible, and unmistakably not vinyl. */
	.disc { animation: td-spin 0.5s linear infinite; }
	.deck.done .platter,
	.deck.done :global(.reel),
	.deck.done .disc,
	.deck.paused .platter,
	.deck.paused :global(.reel),
	.deck.paused .disc { animation-play-state: paused; }

	/* The tonearm is the whole reason this reads as a turntable. It rests off
	   the record and swings down onto it when a side starts — a transform, not
	   a second drawing, so the two states can't drift apart. */
	.arm {
		transform-box: view-box;
		transform-origin: 132px 29px;
		transform: rotate(-17deg);
		transition: transform 0.6s cubic-bezier(0.34, 1.3, 0.5, 1);
	}
	.arm.down { transform: rotate(0deg); }
	.rig { flex: none; }
	.controls { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
	/* Transport buttons read as a deck's, not as page actions. */
	.ctl {
		display: inline-flex; align-items: center; justify-content: center;
		width: 38px; height: 38px; flex: none;
		border-radius: 50%; cursor: pointer;
		border: 1px solid var(--td-accent);
		background: transparent; color: var(--td-accent);
		transition: background .15s, color .15s;
	}
	.ctl:hover:not(:disabled) { background: var(--td-accent); color: #fff; }
	.ctl:disabled { opacity: .4; cursor: default; }
	@media (prefers-reduced-motion: reduce) {
		.platter, .deck :global(.reel), .disc { animation: none; }
		.arm { transition: none; }
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
