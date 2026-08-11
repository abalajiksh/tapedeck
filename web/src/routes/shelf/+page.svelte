<script lang="ts">
	import { onMount, tick } from 'svelte';
	import {
		api,
		type PhysicalRelease, type PhysicalTrack, type ReleaseCandidate,
		type PhysicalImage, type SignalChain, type Equipment, type Spin
	} from '$lib/api';
	import { formatDuration, formatDateTime, toDateInput, fromDateInput, ownedFor } from '$lib/utils';
	import CoverArt from '$lib/components/CoverArt.svelte';
	import ChainIcon from '$lib/components/ChainIcon.svelte';

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
	/** Free-text filter over the shelf. Client-side, like `/loved`: a shelf is
	 *  hundreds of rows at most and they are already in hand, so a query
	 *  parameter per keystroke would be a round trip to narrow a list the page
	 *  is holding. */
	let query = '';

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
	/** When you bought it, filled in while adding — the one moment you reliably
	 *  know, since the alternative is remembering to come back and say. */
	let buyDate = '';
	let addedNote = '';

	// ── Playing a side ──
	//
	// Two ways in, because there are two moments you might reach for this. If
	// you press *start* when you drop the needle, the start time is observed and
	// every track lands where it actually played. If you only think of it once
	// the side has finished, "already played" works it backwards from the
	// running time — right ordering, right gaps, but a start time nobody saw.
	//
	// The spinning state lives on the **server**, not in this browser. It began
	// in localStorage on the reasoning that a side in progress is a fact about
	// the machine in front of the turntable — true of where the clock runs, and
	// false of everything else: the record is on the platter whichever device
	// you are holding, so a phone could not see it, a second tab disagreed with
	// the first, and a crash lost the side along with everything already played.
	let open: PhysicalRelease | null = null;
	let openTracks: PhysicalTrack[] = [];
	let playing = false;
	let flipPrompt = '';
	let playMsg = '';

	// ── What it was played on ──
	//
	// `physical_plays` has carried `chain_id` and `equipment_id` since the shelf
	// was built and nothing ever sent them, so every side landed unattributed
	// and the gear it plainly ran through logged no hours. The picker is
	// pre-filled from the pressing's own default and the answer is written back,
	// because a record lives with one turntable — asking every time would be
	// asking the same question forever.
	let chains: SignalChain[] = [];
	let gear: Equipment[] = [];
	let playChain: number | null = null;
	let playGear: number | null = null;
	let purchased = '';
	let savingMeta = false;

	// ── The rest of the sleeve ──
	let images: PhysicalImage[] = [];
	let imagesFor: number | null = null;
	let imagesBusy = false;
	let imageNote = '';
	let uploadKind = 'booklet';
	let lightbox = -1;

	/** Kinds in the order you would meet them holding the object: the cover,
	 *  round the back, then what is inside. Anything the archive invents that
	 *  isn't here sorts last rather than being hidden. */
	const KIND_ORDER = [
		'front', 'back', 'spine', 'panel', 'obi', 'sticker',
		'booklet', 'liner', 'medium', 'tray', 'matrix', 'track', 'poster', 'other'
	];
	const IMAGE_KIND_LABEL: Record<string, string> = {
		front: 'Front', back: 'Back', spine: 'Spine', panel: 'Panels', obi: 'Obi strip',
		sticker: 'Sticker', booklet: 'Booklet', liner: 'Liner notes', medium: 'Label / disc',
		tray: 'Tray', matrix: 'Matrix & runout', track: 'Track listing', poster: 'Poster',
		other: 'Other'
	};
	const kindLabel = (k: string) =>
		IMAGE_KIND_LABEL[k] ?? k.charAt(0).toUpperCase() + k.slice(1);
	const kindRank = (k: string) => {
		const i = KIND_ORDER.indexOf(k);
		return i < 0 ? KIND_ORDER.length : i;
	};

	/** The gallery, grouped and in shelf order. The archive's own ordering is
	 *  kept *within* a group — it puts a booklet in page order, and no rule
	 *  invented here beats an editor who had the thing in their hands. */
	$: gallery = [...new Set(images.map((i) => i.kind))]
		.sort((a, b) => kindRank(a) - kindRank(b) || a.localeCompare(b))
		.map((k) => ({ kind: k, items: images.filter((i) => i.kind === k) }));
	/** Flattened in the same order, so the lightbox's arrows walk the page the
	 *  way it reads rather than the order rows happen to come back in. */
	$: flat = gallery.flatMap((g) => g.items);

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
		/** How far into the side the needle went down. */
		startSecs?: number;
		outcomes: Outcome[];
	};
	let spinning: Spinning | null = null;
	let nowSec = Math.floor(Date.now() / 1000);
	/** The record that's on, as the *server* describes it — so the deck can be
	 *  drawn before the shelf list has loaded, and on a device that has never
	 *  seen this record. */
	let spinMeta: Spin | null = null;

	/**
	 * Read the deck from the server.
	 *
	 * This used to be localStorage, on the reasoning that a side in progress is
	 * a fact about the browser sitting in front of the turntable. That holds for
	 * where the *clock* runs and for nothing else: the record is on the platter
	 * whichever device you happen to be holding, so your phone could not see it,
	 * a second tab disagreed with the first, and a crash lost the side outright
	 * along with everything that had already played.
	 */
	async function loadSpinning() {
		try {
			const { spin } = await api.getSpin();
			spinMeta = spin;
			spinning = spin
				? {
						releaseId: spin.release_id,
						kind: spin.kind,
						side: spin.side,
						startedAt: spin.started_at,
						pausedAt: spin.paused_at,
						pausedTotal: spin.paused_total,
						startSecs: spin.start_secs ?? 0,
						outcomes: spin.outcomes ?? []
					}
				: null;
		} catch {
			// Offline or signed out. The deck stays as it is rather than being
			// wiped by a failed read — losing a side in progress because the
			// network blinked is the failure this whole change is about.
		}
	}

	/**
	 * Put the deck's state on the server.
	 *
	 * Local state is set first so the UI never waits on a round trip, then the
	 * write is fired. Every event that changes the deck — start, pause, resume,
	 * skip, cancel, record — comes through here, so none of them can forget.
	 */
	function saveSpinning(v: Spinning | null) {
		spinning = v;
		if (!v) {
			spinMeta = null;
			// Clears the stored spin *and* the now-playing entry, so the two
			// cannot disagree about what is on.
			void api.clearSpin().catch(() => {});
			return;
		}
		void api
			.putSpin({
				release_id: v.releaseId,
				side: v.side,
				started_at: v.startedAt,
				paused_at: v.pausedAt,
				paused_total: v.pausedTotal,
				outcomes: v.outcomes,
				chain_id: playChain,
				equipment_id: playGear,
				start_secs: v.startSecs ?? 0
			})
			.catch(() => {
				// The side keeps running locally. The next event rewrites the
				// whole state, so a dropped write costs nothing lasting.
			});
	}

	/**
	 * Tell the server what is on the deck.
	 *
	 * The shelf is the only source where the **browser** is the player: the
	 * clock runs in front of the turntable, in localStorage, and nothing
	 * server-side can derive a playhead from a stack of rows. So it pushes,
	 * where Plex and Jellyfin are polled.
	 *
	 * Called on every track change, on pause and resume, and on a heartbeat.
	 * The heartbeat is not decoration: the registry expires an entry by running
	 * past the end of the track it knows about, so a paused record would drop
	 * off the dashboard one track length after it was paused.
	 */
	async function pushDeck() {
		if (!spinning || !spinTrack) return;
		try {
			await api.shelfNowPlaying(spinning.releaseId, {
				side: spinning.side,
				position: spinTrack.position,
				elapsed: trackElapsed,
				paused
			});
		} catch {
			// A failed push leaves the deck showing the last thing it was told,
			// which the next heartbeat corrects. The side itself is unaffected —
			// the listens are written when it is recorded, not from this.
		}
	}

	/**
	 * Which track a parked tape is sitting at, if this is that side.
	 *
	 * Cassettes only, and the server enforces that — a record has random access
	 * and no state to be in, so it never has a park point to report.
	 */
	function parkedAt(r: PhysicalRelease | null, side: string): number | null {
		if (!r || r.parked_side !== side || r.parked_secs == null) return null;
		const list = sideTracks(side, r);
		let acc = 0;
		for (const t of list) {
			acc += t.duration_secs ?? 180;
			if (r.parked_secs < acc) return t.position;
		}
		// Parked past the end — the tape ran out. Treat that as the start again
		// rather than pointing at a track that has already finished.
		return null;
	}

	/**
	 * Drop the needle.
	 *
	 * `from` is where on the side it goes down. You do not always start a side
	 * at its beginning — the needle goes where you put it, and a tape carries on
	 * from wherever it was stopped — so this is the ordinary case rather than an
	 * option. Everything before `from` is *not* played and is not recorded, and
	 * in particular is not recorded as skipped: a track you never reached is not
	 * a track you skipped, and `scrobbles.skipped` feeds skip-rate analytics.
	 */
	function startSide(side: string, from?: number) {
		if (!open) return;
		playMsg = '';
		flipPrompt = '';
		const now = Math.floor(Date.now() / 1000);
		const list = sideTracks(side);
		const first = from ?? list[0]?.position ?? 1;
		// Where in the side the clock starts, so the countdown reads a position
		// within the side rather than from zero — the tracks before it exist and
		// take time, they just were not played this time.
		const startSecs = list
			.filter((t) => t.position < first)
			.reduce((n, t) => n + (t.duration_secs ?? 180), 0);
		fromPosition = first;
		saveSpinning({
			releaseId: open.id, kind: open.kind, side, startedAt: now,
			pausedAt: null, pausedTotal: 0, startSecs,
			// The first track starts the moment the needle drops.
			outcomes: [{ position: first, started_at: now, skipped: false }]
		});
	}

	/** The track the current spin began at, so the play records only from there. */
	let fromPosition = 1;

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
		? Math.max(0, (spinning.pausedAt ?? nowSec) - spinning.startedAt - spinning.pausedTotal) +
			(spinning.startSecs ?? 0)
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
	/** What the hero deck draws. Falls back to the server's own description of
	 *  the spin, so the deck is right even before the shelf list has arrived —
	 *  which is the first paint after a reload, and every paint on a device that
	 *  has never opened this record. */
	$: deckRelease = spinRelease ??
		(spinMeta
			? { kind: spinMeta.kind, title: spinMeta.title, artist: spinMeta.artist }
			: null);
	/** Something is on, but you're looking at a different record. The hero deck
	 *  at the top of the page covers the "where did it go" case now, so this is
	 *  only used to explain why *this* record's Start buttons are disabled. */
	$: spinElsewhere = !!(spinning && !spinHere);

	/** The track the clock is inside, whichever record is on — unlike
	 *  `nowTrack`, which is scoped to the one being *looked at*. The deck has to
	 *  keep reporting while you browse the rest of the shelf, so the push reads
	 *  this one. */
	$: spinTrack = spinning ? sideTracks(spinning.side, spinRelease)[currentIndex(spinning)] ?? null : null;
	/** Seconds into the current track, not into the side. */
	$: trackElapsed = (() => {
		if (!spinning || !spinTrack) return 0;
		const list = sideTracks(spinning.side, spinRelease);
		const before = list
			.slice(0, currentIndex(spinning))
			.reduce((n, t) => n + (t.duration_secs ?? 180), 0);
		return Math.max(0, spinElapsed - before);
	})();

	// One push per *change*, plus the heartbeat below. Keyed on what the server
	// actually cares about, so a second of elapsed time does not become a
	// request — the registry extrapolates position between reports.
	let lastPush = '';
	$: {
		const key = spinning && spinTrack
			? `${spinning.releaseId}|${spinning.side}|${spinTrack.position}|${paused}`
			: '';
		if (key !== lastPush) {
			lastPush = key;
			if (key) void pushDeck();
		}
	}

	$: sides = [...new Set(openTracks.map((t) => t.side))].sort();
	/** Matched over artist, title, catalogue number and barcode — the four
	 *  things you would have in your hand looking for a record. Accent-folded
	 *  so "Coeur" finds "Cœur", the same fold `normalise_artist` makes on the
	 *  server; without it the shelf disagrees with every other screen. */
	const fold = (v: string) =>
		v.normalize('NFKD').replace(/[\u0300-\u036f]/g, '')
			.replace(/œ/g, 'oe').replace(/æ/g, 'ae').replace(/ß/g, 'ss')
			.toLowerCase();
	$: q = fold(query.trim());
	$: shown = shelf
		.filter((r) => filter === 'all' || r.kind === filter)
		.filter(
			(r) =>
				!q ||
				fold(`${r.artist} ${r.title} ${r.catalog_number ?? ''} ${r.barcode ?? ''}`).includes(q)
		);

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

	async function loadGear() {
		// Neither is fatal: a shelf with no chains configured still plays sides,
		// the picker just has nothing to offer.
		try { chains = (await api.getChains()).chains; } catch { chains = []; }
		try { gear = (await api.getEquipment()).equipment; } catch { gear = []; }
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
			const r = await api.addPressing({
				...full, kind: lookupKind, tracks: full.tracks ?? [],
				purchased_at: fromDateInput(buyDate)
			});
			adding = false;
			candidates = [];
			barcode = catno = qArtist = qRelease = buyDate = '';
			await load();
			// Said rather than left to be noticed: pulling twenty-nine images is
			// most of why shelving from MusicBrainz beats typing it in, and it is
			// invisible until the record is opened.
			if (r.images) error = '';
			addedNote = r.images
				? `Added, with ${r.images} image${r.images === 1 ? '' : 's'} from the Cover Art Archive.`
				: 'Added.';
		} catch (e) {
			searchNotes = [String(e).replace(/^API \d+: /, '')];
		}
	}

	let panelEl: HTMLElement | null = null;

	async function openRelease(r: PhysicalRelease) {
		open = r;
		playMsg = '';
		flipPrompt = '';
		imageNote = '';
		lightbox = -1;
		playChain = r.chain_id;
		playGear = r.equipment_id;
		purchased = toDateInput(r.purchased_at);
		try {
			openTracks = JSON.parse(r.tracks) as PhysicalTrack[];
		} catch {
			openTracks = [];
		}
		void loadImages(r.id);
		// The panel is above the grid, but on a long shelf you can still be
		// scrolled well past it — so bring it to you rather than leaving the
		// click looking inert.
		await tick();
		panelEl?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
	}

	// ── Scans ──

	async function loadImages(id: number) {
		// Guarded on the id rather than a boolean: opening a second record while
		// the first is still loading would otherwise paint its sleeve here.
		imagesFor = id;
		images = [];
		try {
			const r = await api.pressingImages(id);
			if (imagesFor === id) images = r.images;
		} catch (e) {
			if (imagesFor === id) imageNote = String(e).replace(/^API \d+: /, '');
		}
	}

	/** Ask the archive again. Idempotent on the provider's own image id, so this
	 *  picks up a booklet somebody uploaded since and leaves the rest alone. */
	async function fetchArt() {
		if (!open) return;
		imagesBusy = true;
		imageNote = '';
		try {
			const r = await api.fetchPressingImages(open.id);
			imageNote = r.notes.length
				? r.notes.join(' · ')
				: r.stored
					? `Found ${r.total} image${r.total === 1 ? '' : 's'}.`
					: 'Nothing new — you already have everything the archive holds.';
			await loadImages(open.id);
			await load();
		} catch (e) {
			imageNote = String(e).replace(/^API \d+: /, '');
		}
		imagesBusy = false;
	}

	async function uploadScans(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const files = Array.from(input.files ?? []);
		if (!open || !files.length) return;
		imagesBusy = true;
		imageNote = '';
		try {
			// Sorted by name, because a booklet scanned page by page comes off a
			// scanner as `page-01.jpg`… and the file picker's own order is the
			// operating system's business, not page order.
			files.sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
			const r = await api.uploadPressingImages(open.id, files, uploadKind);
			imageNote = `Added ${r.ids.length} scan${r.ids.length === 1 ? '' : 's'}.`;
			await loadImages(open.id);
			await load();
		} catch (err) {
			imageNote = String(err).replace(/^API \d+: /, '');
		}
		imagesBusy = false;
		input.value = '';
	}

	async function removeImage(img: PhysicalImage) {
		if (!open) return;
		const own = img.provider === 'upload';
		if (!confirm(own
			? 'Delete this scan? The file is removed from the server and cannot be recovered.'
			: 'Remove this image from the shelf? It stays at the provider and can be fetched again.')) return;
		try {
			await api.deletePressingImage(img.id);
			lightbox = -1;
			await loadImages(open.id);
			await load();
		} catch (e) {
			imageNote = String(e).replace(/^API \d+: /, '');
		}
	}

	/** Re-file an image the provider guessed wrong about — Discogs says only
	 *  "primary" or "secondary", so everything but the cover lands in Other. */
	async function refile(img: PhysicalImage, kind: string) {
		if (!open || kind === img.kind) return;
		try {
			await api.updatePressingImage(img.id, { kind });
			await loadImages(open.id);
		} catch (e) {
			imageNote = String(e).replace(/^API \d+: /, '');
		}
	}

	function lightboxKeys(e: KeyboardEvent) {
		if (lightbox < 0) return;
		if (e.key === 'Escape') lightbox = -1;
		if (e.key === 'ArrowRight') lightbox = (lightbox + 1) % flat.length;
		if (e.key === 'ArrowLeft') lightbox = (lightbox - 1 + flat.length) % flat.length;
	}

	// ── Shelf keeping ──

	/** Save the fields that describe the object rather than the music: when you
	 *  bought it, and what it goes on. */
	async function saveMeta() {
		if (!open) return;
		savingMeta = true;
		try {
			await api.updatePressing(open.id, {
				purchased_at: fromDateInput(purchased),
				chain_id: playChain,
				equipment_id: playGear
			});
			await load();
			const fresh = shelf.find((x) => x.id === open?.id);
			if (fresh) open = fresh;
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		savingMeta = false;
	}

	/**
	 * Move a record to another shelf.
	 *
	 * The picker when you add something is what you *thought* you were adding;
	 * the release's own medium is what it is. A box set looked up by barcode
	 * could be filed as vinyl while MusicBrainz says all five media are CDs, and
	 * until now there was no way to move it — which is how a five-CD set ends up
	 * on the vinyl shelf with 76 tracks split into two sides.
	 *
	 * Changing this rewrites the sides, because what a side *is* depends on the
	 * medium. The tracklist is refetched rather than patched locally, since the
	 * server is the one that did the splitting.
	 */
	let changingKind = false;
	async function changeKind(kind: Kind) {
		if (!open || open.kind === kind) return;
		changingKind = true;
		playMsg = '';
		try {
			const res = await api.updatePressing(open.id, { kind });
			await load();
			const fresh = shelf.find((x) => x.id === open?.id);
			if (fresh) {
				open = fresh;
				try { openTracks = JSON.parse(fresh.tracks) as PhysicalTrack[]; } catch { openTracks = []; }
			}
			playMsg = res.sides_rewritten
				? `Moved to ${KIND_LABEL[kind]} — the ${isDisc(kind) ? 'discs' : 'sides'} were worked out again.`
				: `Moved to ${KIND_LABEL[kind]}.`;
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		changingKind = false;
	}

	async function play(side: string, startedAt?: number) {
		if (!open) return;
		playing = true;
		playMsg = '';
		// Did *this* deck run the side, or is this the "already played" path?
		// Everything the deck observed — outcomes, where it started, where the
		// tape ended up — is only meaningful in the first case.
		const ranHere = !!(spinning && spinning.releaseId === open.id);
		try {
			const r = await api.playSide(open.id, {
				side,
				started_at: startedAt,
				// What it ran through. Sent explicitly rather than left to the
				// stored default so that changing the picker for one side takes
				// effect on that side, and is remembered for the next.
				chain_id: playChain,
				equipment_id: playGear,
				// Only when this deck ran the side; "already played" has none.
				tracks: ranHere ? spinning?.outcomes : undefined,
				// Where the needle went down. Without this every track before it
				// is recorded as having played.
				from_position: ranHere ? fromPosition : undefined,
				// Where the tape ends up. Only when it came off part-way — a side
				// played to the end has wound through, and the server treats an
				// absent value as "wind it back". Cassettes only; the server
				// refuses it for anything else rather than trusting the caller.
				// `spinElapsed` is already a position *within the side* — it counts
				// from the start of the side, not from where the needle went
				// down — so adding `startSecs` again double-counts it and parks
				// the tape past its own end.
				parked_secs:
					ranHere && open.kind === 'cassette' && spinLeft > 0 ? spinElapsed : undefined
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
		void loadSpinning();
		// Drives the countdown and the "side is over" prompt. One second is the
		// right resolution for a twenty-minute side and costs nothing.
		const tick = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 1000);
		// The heartbeat. Ten seconds against the registry's own ten-second grace
		// on a finished track, so a paused record never falls off the dashboard
		// between reports.
		const beat = setInterval(() => { if (spinning) void pushDeck(); }, 10_000);
		// Now that the deck lives on the server, another device can change it.
		// Re-reading on focus is enough: nobody watches two screens at once, and
		// polling would fight with the local edits this tab is making.
		const refocus = () => void loadSpinning();
		window.addEventListener('focus', refocus);
		load();
		loadGear();
		return () => {
			clearInterval(tick);
			clearInterval(beat);
			window.removeEventListener('focus', refocus);
		};
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
	<div style="display:flex;gap:8px;">
		<!-- A plain link, not a fetch: the browser downloads a zip far better
		     than any amount of Blob juggling, and the endpoint is a GET the
		     session cookie already authenticates. -->
		<a class="btn btn-secondary" href="/api/v1/physical/export"
			title="Everything about the objects you own — pressings, sides played, gear, and your own scans">
			Export the shelf
		</a>
		<button class="btn btn-primary" on:click={() => (adding = !adding)}>
			{adding ? 'Cancel' : 'Add to the shelf'}
		</button>
	</div>
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
			<div class="field">
				<label for="f-buy">Bought on</label>
				<input id="f-buy" class="input" type="date" bind:value={buyDate}>
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

			{#if addedNote}
				<p style="font-size:13px;color:var(--td-good-fg);margin:10px 0 0;">{addedNote}</p>
			{/if}

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

<!-- The "… is on" banner that used to sit here is gone: the hero deck below
     is now always at the top of this page and says the same thing with the
     record drawn, so the two were a duplicate stacked on each other. The
     banner still earns its place *elsewhere* in the app, where there is no
     deck — this page is the one screen that has one. -->

<!-- The decks are on the page whether or not anything is on them. An empty
     shelf otherwise opens on a wall of text, and this is a screen about
     objects — the platter sitting still is also the honest signal that
     nothing is playing, which is the rule the dashboard's reels follow. -->
{#if spinning && deckRelease}
	<div class="deck deck-hero" class:done={spinLeft === 0} class:paused>
		{#if deckRelease.kind === 'vinyl'}
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
		{:else if isDisc(deckRelease.kind)}
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
					? `${unit(deckRelease.kind)} finished`
					: paused
						? `${unit(deckRelease.kind)} ${spinning.side} paused`
						: `${unit(deckRelease.kind)} ${spinning.side} playing`}
			</div>
			<div class="h-display" style="font-size:19px;margin-top:2px;">
				{spinTrack ? spinTrack.title : deckRelease.title}
			</div>
			{#if spinTrack}
				<div style="font-size:12.5px;color:var(--td-muted);">
					{deckRelease.title}{skippedSoFar ? ` · ${skippedSoFar} skipped` : ''}
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

<div style="display:flex;gap:8px;margin:18px 0;flex-wrap:wrap;align-items:center;">
	{#each ['all', 'vinyl', 'cassette', 'cd', 'sacd'] as f}
		<button class="chip" class:on={filter === f} on:click={() => (filter = f as Filter)}>
			{f === 'all' ? 'Everything' : f === 'cassette' ? 'Cassettes' : KIND_LABEL[f]}
		</button>
	{/each}
	<span style="flex:1;"></span>
	<div style="position:relative;min-width:200px;">
		<input class="input" bind:value={query} placeholder="Find a record…"
			aria-label="Search the shelf" style="padding-right:30px;" />
		{#if query}
			<button class="clearq" on:click={() => (query = '')} aria-label="Clear search">×</button>
		{/if}
	</div>
</div>

{#if query && shown.length}
	<p style="font-size:12.5px;color:var(--td-muted-2);margin:-8px 0 12px;">
		{shown.length} of {shelf.length}
	</p>
{/if}

{#if error}
	<p style="color:#b2622d;font-size:14px;">{error}</p>
{/if}

{#if open}
	<!-- The open record sits **above** the grid, not after it. With thirty
	     records the panel opened somewhere off the bottom of the page, so
	     clicking a card looked like it had done nothing at all — the same
	     complaint Rediscovery's "See all" had, and the same fix: keep it in
	     reach, ring the card it belongs to, and scroll it into view. -->
	<div class="panel open-panel" bind:this={panelEl}
		style="border-radius:22px;padding:24px;margin:20px 0;">
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
				{#if open.purchased_at}
					<p style="margin:2px 0 0;color:var(--td-muted-2);font-size:12.5px;">
						Yours for {ownedFor(open.purchased_at)} — bought {formatDateTime(open.purchased_at).split(',')[0]}
					</p>
				{/if}
			</div>
			<div style="display:flex;gap:8px;">
				<button class="btn btn-secondary" on:click={() => (open = null)}>Close</button>
				<button class="btn btn-secondary" on:click={() => open && remove(open)}>Remove</button>
			</div>
		</div>

		<!-- Which shelf it is on. Filed by the picker when it was added, which is
		     a guess about the object made before anyone looked at it — so it has
		     to be changeable, and changing it re-splits the tracklist. -->
		<div class="kindrow">
			<span style="font-size:11px;letter-spacing:.14em;text-transform:uppercase;color:var(--td-muted-2);font-weight:600;">
				Filed as
			</span>
			{#each ['vinyl', 'cassette', 'cd', 'sacd'] as k}
				<button class="chip" class:on={open.kind === k} disabled={changingKind}
					on:click={() => changeKind(k as Kind)}>{KIND_LABEL[k]}</button>
			{/each}
			{#if changingKind}<span style="font-size:12.5px;color:var(--td-muted);">Moving…</span>{/if}
			<span style="flex:1;"></span>
			<span style="font-size:12px;color:var(--td-muted-2);">
				{isDisc(open.kind)
					? 'Plays straight through — no sides.'
					: 'Split into sides you flip.'}
			</span>
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

			<!-- What it goes on, and when it became yours. Both are facts about
			     the object rather than about the music, so they sit together and
			     above the sides — the picker decides how the next side is
			     attributed, which is no use discovered afterwards. -->
			<div class="keeping">
				<div class="field" style="flex:1;min-width:190px;">
					<label for="k-chain" style="display:flex;align-items:center;gap:6px;">
						<ChainIcon name={chains.find((c) => c.id === playChain)?.icon} size={14} />
						Played through
					</label>
					<select id="k-chain" class="input" bind:value={playChain} on:change={saveMeta}>
						<option value={null}>— no chain —</option>
						{#each chains as c}<option value={c.id}>{c.name}</option>{/each}
					</select>
				</div>
				<div class="field" style="flex:1;min-width:190px;">
					<label for="k-gear">On</label>
					<select id="k-gear" class="input" bind:value={playGear} on:change={saveMeta}>
						<option value={null}>— no deck —</option>
						{#each gear as g}<option value={g.id}>{g.name}{g.brand ? ` — ${g.brand}` : ''}</option>{/each}
					</select>
				</div>
				<div class="field" style="flex:0 0 160px;">
					<label for="k-buy">Bought on</label>
					<input id="k-buy" class="input" type="date" bind:value={purchased} on:change={saveMeta}>
				</div>
				<div style="font-size:12px;color:var(--td-muted-2);flex:1;min-width:180px;align-self:center;">
					{#if savingMeta}
						Saving…
					{:else if !chains.length}
						No signal chains yet — add one under Signal Chains and every side
						you play will be attributed to it.
					{:else}
						Remembered for next time. The stylus hours of the deck and the
						chain's own hours both come from the sides played through it.
					{/if}
				</div>
			</div>

			<div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(240px,1fr));gap:14px;margin-top:18px;">
				{#each sides as side}
					<div class="side">
						<div style="display:flex;align-items:baseline;justify-content:space-between;gap:10px;">
							<span class="h-display" style="font-size:17px;">{unit(open.kind)} {side}</span>
							<span style="font-size:12px;color:var(--td-muted-2);">
								{formatDuration(sideSecs(side))}
							</span>
						</div>
						<!-- Every track is a start point. You do not always begin a
						     side at its beginning: the needle goes down where you
						     put it, and a tape carries on from wherever it was
						     stopped. Clicking one starts there — and nothing before
						     it is recorded, since a track you never reached is not
						     a track you skipped. -->
						<ol style="margin:10px 0 0;padding-left:18px;">
							{#each openTracks.filter((t) => t.side === side) as t}
								<li style="font-size:13px;padding:1px 0;">
									<button class="trk" disabled={playing || !!spinning}
										title="Start {unit(open.kind).toLowerCase()} {side} from here"
										on:click={() => startSide(side, t.position)}>
										<span>{t.title}</span>
										{#if t.duration_secs}
											<span style="color:var(--td-muted-2);">· {formatDuration(t.duration_secs)}</span>
										{/if}
										{#if parkedAt(open, side) === t.position}
											<span class="parkpip" title="The tape is sitting here">◂ here</span>
										{/if}
									</button>
								</li>
							{/each}
						</ol>
						<div style="display:flex;gap:6px;margin-top:12px;flex-wrap:wrap;">
							{#if parkedAt(open, side)}
								<!-- A cassette is sequential: stop it halfway and eject,
								     and it is still parked there. So the honest first
								     offer is to carry on, not to start again. -->
								<button class="btn btn-primary" style="flex:1;min-width:150px;"
									disabled={playing || !!spinning}
									on:click={() => startSide(side, parkedAt(open, side) ?? 1)}>
									Carry on from track {parkedAt(open, side)}
								</button>
								<button class="btn btn-secondary" disabled={playing || !!spinning}
									title="Wind it back and start the side again"
									on:click={() => startSide(side, 1)}>
									From the top
								</button>
							{:else}
								<button class="btn btn-primary" style="flex:1;min-width:150px;"
									disabled={playing || !!spinning} on:click={() => startSide(side)}>
									Start {unit(open.kind).toLowerCase()} {side}
								</button>
							{/if}
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

		<!-- Outside the tracklist branch on purpose: a pressing nobody has
		     entered a tracklist for still has a sleeve, and the scans are most of
		     what makes this feel like the shelf rather than a database of it. -->
		<div class="scans">
			<div style="display:flex;align-items:flex-end;justify-content:space-between;gap:12px;flex-wrap:wrap;">
				<div>
					<h4 class="h-display" style="font-size:17px;margin:0;">Sleeve &amp; scans</h4>
					<p style="margin:3px 0 0;color:var(--td-muted);font-size:12.5px;">
						{images.length
							? `${images.length} image${images.length === 1 ? '' : 's'} — the archive's, and your own.`
							: 'Nothing yet. Pull what the Cover Art Archive holds, or scan your own copy.'}
					</p>
				</div>
				<div style="display:flex;gap:8px;align-items:center;flex-wrap:wrap;">
					<select class="input" style="width:auto;padding:7px 10px;font-size:13px;" bind:value={uploadKind}
						title="What the scans you are about to upload are">
						{#each KIND_ORDER as k}<option value={k}>{kindLabel(k)}</option>{/each}
					</select>
					<label class="btn btn-secondary" style="cursor:pointer;margin:0;">
						{imagesBusy ? 'Working…' : 'Upload scans'}
						<input type="file" accept="image/*" multiple on:change={uploadScans}
							disabled={imagesBusy} style="display:none;">
					</label>
					<button class="btn btn-secondary" on:click={fetchArt} disabled={imagesBusy}>
						{imagesBusy ? '…' : 'Fetch from the archive'}
					</button>
				</div>
			</div>

			{#if imageNote}
				<p style="font-size:12.5px;color:var(--td-muted);margin:10px 0 0;">{imageNote}</p>
			{/if}

			{#each gallery as group}
				<div style="margin-top:16px;">
					<div style="font-size:11px;letter-spacing:.14em;text-transform:uppercase;color:var(--td-muted-2);font-weight:600;margin-bottom:8px;">
						{kindLabel(group.kind)}
						<span style="letter-spacing:0;text-transform:none;font-weight:400;">
							· {group.items.length}
						</span>
					</div>
					<div class="thumbs">
						{#each group.items as img}
							<button class="thumb" on:click={() => (lightbox = flat.indexOf(img))}
								title={img.caption ?? kindLabel(img.kind)}>
								<img src={img.thumb} alt={img.caption ?? kindLabel(img.kind)} loading="lazy">
								{#if img.provider === 'upload'}
									<!-- Whose scan this is, at a glance. Your own copy and a
									     stranger's upload of a different pressing are not the
									     same thing, and only one of them is in this room. -->
									<span class="mine" title="Your own scan">yours</span>
								{/if}
							</button>
						{/each}
					</div>
				</div>
			{/each}
		</div>
	</div>
{/if}

<!-- The lightbox. A booklet is the one thing here worth looking at full size,
     and nineteen pages need arrows rather than nineteen round trips. -->
{#if lightbox >= 0 && flat[lightbox]}
	<div class="lb" role="presentation" on:click={() => (lightbox = -1)}>
		<img src={flat[lightbox].src} alt={flat[lightbox].caption ?? kindLabel(flat[lightbox].kind)}
			on:click|stopPropagation role="presentation">
		<div class="lb-bar" role="presentation" on:click|stopPropagation>
			<span style="flex:1;min-width:0;">
				<strong>{kindLabel(flat[lightbox].kind)}</strong>
				{#if flat[lightbox].caption}<span style="color:var(--td-muted);"> · {flat[lightbox].caption}</span>{/if}
				<span style="color:var(--td-muted-2);"> · {lightbox + 1} of {flat.length}</span>
				<span style="color:var(--td-muted-2);">
					· {flat[lightbox].provider === 'upload' ? 'your scan' : flat[lightbox].provider === 'caa' ? 'Cover Art Archive' : 'Discogs'}
				</span>
			</span>
			<select class="input" style="width:auto;padding:5px 8px;font-size:12.5px;"
				value={flat[lightbox].kind}
				on:change={(e) => refile(flat[lightbox], (e.currentTarget as HTMLSelectElement).value)}>
				{#each KIND_ORDER as k}<option value={k}>{kindLabel(k)}</option>{/each}
			</select>
			<button class="btn btn-secondary" on:click={() => (lightbox = (lightbox - 1 + flat.length) % flat.length)}>←</button>
			<button class="btn btn-secondary" on:click={() => (lightbox = (lightbox + 1) % flat.length)}>→</button>
			<button class="btn btn-secondary" on:click={() => removeImage(flat[lightbox])}>Remove</button>
			<button class="btn btn-secondary" on:click={() => (lightbox = -1)}>Close</button>
		</div>
	</div>
{/if}

<svelte:window on:keydown={lightboxKeys} />
{#if loading}
	<p style="color:var(--td-muted);font-size:14px;">Reading the shelf…</p>
{:else if !shown.length}
	<div class="panel" style="border-radius:22px;padding:32px;text-align:center;">
		{#if query || filter !== 'all'}
			<p style="margin:0 0 6px;font-size:15px;">Nothing here matches.</p>
			<p style="margin:0;color:var(--td-muted);font-size:13.5px;">
				{shelf.length} on the shelf altogether.
				<button class="btn btn-ghost" style="font-size:13px;padding:2px 8px;"
					on:click={() => { query = ''; filter = 'all'; }}>Clear the filters</button>
			</p>
		{:else}
			<p style="margin:0 0 6px;font-size:15px;">Nothing on the shelf yet.</p>
			<p style="margin:0;color:var(--td-muted);font-size:13.5px;">
				Add a record or tape and Tapedeck will record each side you play.
			</p>
		{/if}
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
						{#if r.image_count}· {r.image_count} scan{r.image_count === 1 ? '' : 's'}{/if}
					</div>
				</div>
			</button>
		{/each}
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
	/* The open card wears the ring its panel is wearing, so the two read as one
	   thing across the grid between them — the same treatment Rediscovery uses,
	   and the reason a click no longer looks like it did nothing. */
	.card.open {
		border-color: var(--td-accent);
		box-shadow: 0 0 0 2px color-mix(in srgb, var(--td-accent) 45%, transparent);
	}
	.open-panel {
		border: 1px solid var(--td-accent);
		box-shadow: 0 0 0 2px color-mix(in srgb, var(--td-accent) 22%, transparent);
	}

	/* The hero deck. The design draws this big — it is the one thing on the page
	   that is happening *now*, and at the old inline size it read as a row in a
	   list. Full width at the top, with the platter given room to be a platter. */
	.deck-hero {
		margin: 18px 0 4px;
		padding: 22px 24px;
		gap: 22px;
		border-radius: 22px;
		background: linear-gradient(
			135deg,
			color-mix(in srgb, var(--td-accent) 9%, var(--td-panel)),
			var(--td-panel) 62%
		);
	}
	/* 1.45x the inline size. Large enough that the tonearm swinging down and the
	   reels turning are legible from across a room, which is where you are when
	   a record is on. */
	.deck-hero > svg { flex: none; width: auto; height: 165px; }
	@media (max-width: 640px) {
		.deck-hero { flex-direction: column; align-items: flex-start; padding: 18px; }
		.deck-hero > svg { height: 130px; }
	}
	.kindrow {
		display: flex; align-items: center; gap: 8px; flex-wrap: wrap;
		margin: 14px 0 2px; padding: 12px 0;
		border-top: 1px solid var(--td-line); border-bottom: 1px solid var(--td-line);
	}
	/* A track is a start point, so it has to look like something you can press —
	   but a tracklist is a list first and a row of buttons second, so this is a
	   text button that only declares itself on hover. */
	.trk {
		display: inline-flex; align-items: baseline; gap: 5px; flex-wrap: wrap;
		background: none; border: 0; padding: 1px 4px; margin-left: -4px;
		border-radius: 6px; cursor: pointer; text-align: left;
		color: inherit; font: inherit; font-size: 13px;
	}
	.trk:hover:not(:disabled) {
		background: color-mix(in srgb, var(--td-accent) 14%, transparent);
	}
	.trk:disabled { cursor: default; }
	.parkpip {
		font-size: 11px; color: var(--td-accent);
		letter-spacing: .04em; white-space: nowrap;
	}
	.clearq {
		position: absolute; right: 8px; top: 50%; transform: translateY(-50%);
		background: none; border: 0; cursor: pointer; font-size: 17px; line-height: 1;
		color: var(--td-muted); padding: 0 2px;
	}
	.clearq:hover { color: var(--td-fg); }
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
	/* Facts about the object rather than about the music, so they read as a
	   strip of form rather than as another card. */
	.keeping {
		display: flex;
		gap: 12px;
		flex-wrap: wrap;
		align-items: flex-end;
		margin-top: 18px;
		padding: 14px;
		border: 1px solid var(--td-line);
		border-radius: 14px;
	}

	.scans {
		margin-top: 22px;
		padding-top: 18px;
		border-top: 1px solid var(--td-line);
	}
	/* Fixed rows rather than `auto-fill` alone: a booklet is twenty pages of the
	   same shape, and letting them size themselves makes a wall of slightly
	   different rectangles. */
	.thumbs {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(94px, 1fr));
		gap: 10px;
	}
	.thumb {
		position: relative;
		aspect-ratio: 1;
		padding: 0;
		border: 1px solid var(--td-line);
		border-radius: 10px;
		overflow: hidden;
		background: var(--td-panel);
		cursor: pointer;
		transition: border-color 0.15s, transform 0.15s;
	}
	.thumb:hover { border-color: var(--td-accent); transform: translateY(-1px); }
	.thumb img { width: 100%; height: 100%; object-fit: cover; display: block; }
	.mine {
		position: absolute;
		left: 5px;
		bottom: 5px;
		padding: 1px 6px;
		border-radius: 999px;
		font-size: 10px;
		font-weight: 600;
		letter-spacing: 0.04em;
		background: var(--td-accent);
		color: #fff;
	}

	.lb {
		position: fixed;
		inset: 0;
		z-index: 60;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 12px;
		padding: 24px;
		background: rgba(0, 0, 0, 0.82);
	}
	.lb img {
		max-width: 100%;
		/* Room for the bar underneath, or a tall booklet page pushes it off. */
		max-height: calc(100vh - 130px);
		object-fit: contain;
		border-radius: 8px;
	}
	.lb-bar {
		display: flex;
		gap: 8px;
		align-items: center;
		flex-wrap: wrap;
		max-width: 100%;
		padding: 10px 14px;
		border-radius: 14px;
		background: var(--td-panel);
		border: 1px solid var(--td-line);
		font-size: 13px;
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
	/* Shown in the hero deck's place when nothing is on. */
	.idle {
		display: flex; align-items: center; gap: 20px; flex-wrap: wrap;
		margin-top: 18px; padding: 18px 20px;
		border: 1px dashed var(--td-line); border-radius: 18px;
	}
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
