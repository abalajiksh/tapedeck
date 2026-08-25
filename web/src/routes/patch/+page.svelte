<script lang="ts">
	// Patch — Phase 6.
	//
	// All six of the design's tabs, plus Dubs and Shared Spools.
	//
	// Everything here is **local**: both ends of every patch are accounts on this
	// instance, so nothing is signed and nothing is delivered. Outbound delivery
	// is the whole of what is left of the phase.
	import { onMount, onDestroy } from 'svelte';
	import { api } from '$lib/api';
	import type {
		SentSitting,
		PatchPeer,
		ReelListen,
		PatchArtist,
		DiscoverDeck,
		InstancePulse,
		FederationOps,
		Dub,
		SpoolView,
		SpoolCapture,
		SpoolMember,
		HandedSitting,
		HandedSittingDetail,
		SignalChain,
		LiveDeck
	} from '$lib/api';
	import CoverArt from '$lib/components/CoverArt.svelte';
	import ChainIcon from '$lib/components/ChainIcon.svelte';
	import { syncUrl } from '$lib/url';
	import { openArtist } from '$lib/entities';
	import { lovedKeys, isLovedNamed, toggleLoveByName, loadLoves } from '$lib/loves';

	type Tab = 'reel' | 'artists' | 'decks' | 'discover' | 'instance' | 'inbox';
	let tab: Tab = 'reel';
	const TABS: Tab[] = ['reel', 'artists', 'decks', 'discover', 'instance', 'inbox'];
	const TAB_LABELS: Record<Tab, string> = {
		reel: 'The Reel',
		artists: 'Artists',
		decks: 'Decks',
		discover: 'Discover',
		instance: 'Instance',
		inbox: 'Inbox'
	};

	let decks: PatchPeer[] = [];
	let following: PatchPeer[] = [];
	let followers: PatchPeer[] = [];
	let listens: ReelListen[] = [];
	let next: number | null = null;
	let loading = true;
	let reelLoading = false;
	// Both server-side — the Reel is keyset-paged, so filtering the loaded page
	// would silently search only what has been scrolled to and say nothing
	// about it, which looks exactly like a search that found nothing.
	let reelQ = '';
	let reelDeck: string | null = null;
	let reelTimer: ReturnType<typeof setTimeout>;
	/**
	 * Which Reel request is the current one.
	 *
	 * Two controls now fire loads, and they race: clearing the search and then
	 * picking a deck sent both, and the *earlier* one answered last and
	 * overwrote the list — leaving rows on screen that did not match the
	 * controls above them. Caught by driving the page rather than by any check,
	 * because both requests were correct and only their order was not.
	 *
	 * A sequence number rather than an AbortController: the older response is
	 * already on its way and may well be useful to the HTTP cache; what must not
	 * happen is it being *applied*.
	 */
	let reelSeq = 0;
	let busy: string | null = null;
	let error = '';
	/** Something that worked and left nothing on screen to show for it. */
	let notice = '';

	// Loaded when their tab is first opened rather than on mount: six tabs, and
	// most visits never leave the first one. The same call the Settings screen
	// makes about its dozen sections.
	let artists: PatchArtist[] = [];
	let artistsLoaded = false;
	type ArtistSort = 'plays' | 'az' | 'new' | 'cold' | 'common';
	let artistSort: ArtistSort = 'plays';
	/** Null is all time. */
	let artistDays: number | null = 30;
	const SORTS: { key: ArtistSort; label: string }[] = [
		{ key: 'plays', label: 'Most played' },
		{ key: 'az', label: 'A – Z' },
		{ key: 'new', label: 'Newest arrival' },
		{ key: 'cold', label: 'Cold storage' },
		{ key: 'common', label: 'In common' }
	];
	const RANGES: { days: number | null; label: string }[] = [
		{ days: 7, label: '7 days' },
		{ days: 30, label: '30 days' },
		{ days: 365, label: '12 months' },
		{ days: null, label: 'All time' }
	];
	const RANGE_LABEL: Record<number, string> = {
		7: 'Ranked by plays in the last 7 days.',
		30: 'Ranked by plays in the last 30 days.',
		365: 'Ranked by plays in the last 12 months.',
		0: 'Ranked by plays, all time.'
	};

	// Reordering a set already in hand — see the note in the markup for why the
	// range is a server parameter and this is not.
	$: sortedArtists = (() => {
		const a = [...artists];
		switch (artistSort) {
			case 'az':
				return a.sort((x, y) => x.name.localeCompare(y.name));
			case 'new':
				return a.sort((x, y) => (y.first_played ?? 0) - (x.first_played ?? 0));
			// Longest since anybody played them. Nulls sort as "never", which is
			// the coldest thing there is.
			case 'cold':
				return a.sort((x, y) => (x.last_played ?? 0) - (y.last_played ?? 0));
			// Artists you both play, the ones you don't dropped rather than
			// merely sorted down — "in common" is a filter, not an ordering.
			case 'common':
				return a.filter((x) => x.mine > 0).sort((x, y) => y.mine - x.mine);
			default:
				return a.sort((x, y) => y.plays - x.plays);
		}
	})();

	async function setArtistDays(days: number | null) {
		artistDays = days;
		await loadArtists();
	}

	/** A stable colour per artist, so a card is recognisable without a picture.
	 *  Hue from the name, not from anything about the music — it is an aid to
	 *  finding the card again, and must not read as a claim about a genre. */
	function artistTint(name: string, solid = false): string {
		let h = 0;
		for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) % 360;
		return solid ? `hsl(${h} 42% 52%)` : `hsl(${h} 34% 26%)`;
	}
	let discoverDecks: DiscoverDeck[] = [];
	let discoverLoaded = false;
	let pulse: InstancePulse | null = null;
	let pulseDays = 7;
	let ops: FederationOps | null = null;
	let vocab: { verb: string; object: string; namespace: string; frozen: boolean } | null = null;
	let instanceLoaded = false;

	/**
	 * The daily series, bucketed into the **reader's** days.
	 *
	 * The server sends raw `(timestamp, hours)` pairs precisely because the day
	 * boundary is not something it can know — the trap the genre-map trajectory
	 * already documents. A listen at 00:30 CET belongs to the new day, and only
	 * the browser is holding that answer.
	 *
	 * Days with nothing in them are kept rather than dropped: a bar chart with
	 * the quiet days missing reads as continuous listening.
	 */
	$: dayBars = (() => {
		if (!pulse) return [];
		const byDay = new Map<string, number>();
		const dayKey = (d: Date) =>
			`${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`;
		const today = new Date();
		today.setHours(0, 0, 0, 0);
		for (let i = pulseDays - 1; i >= 0; i--) {
			const d = new Date(today);
			d.setDate(d.getDate() - i);
			byDay.set(dayKey(d), 0);
		}
		for (const p of pulse.daily) {
			const k = dayKey(new Date(p.at * 1000));
			if (byDay.has(k)) byDay.set(k, (byDay.get(k) ?? 0) + p.hours);
		}
		return [...byDay.entries()].map(([key, hours]) => {
			const [y, m, d] = key.split('-').map(Number);
			const date = new Date(y, m - 1, d);
			return {
				key,
				hours,
				// A weekday reads at seven days and becomes a repeating smear at
				// thirty, where the date is what tells the bars apart.
				label:
					pulseDays <= 7
						? date.toLocaleDateString(undefined, { weekday: 'short' })
						: date.toLocaleDateString(undefined, { day: 'numeric' })
			};
		});
	})();
	$: peakHours = dayBars.reduce((m, b) => Math.max(m, b.hours), 0);
	let isAdmin = false;

	// The sitting, and what it has offered. Polled while one is running, because
	// captures arrive from *someone else's* deck and nothing else on this page
	// would learn about them.
	let spool: SpoolView | null = null;
	let captures: SpoolCapture[] = [];
	let dubs: Dub[] = [];
	// Decks playing right now. Read from the in-memory registry rather than the
	// history, so it costs no scan — and gated on the same shared chain the Reel
	// is, which is what `NowPlaying.chain_id` was added for.
	let live: LiveDeck[] = [];
	// Sittings somebody passed on. Loaded with the rest rather than on the tab,
	// because the inbox count on the tab strip has to include them.
	let sittings: HandedSitting[] = [];
	/**
	 * What you have handed over.
	 *
	 * Nothing listed these, which is why `withdrawSitting` shipped with no
	 * caller: you could pass an evening on and never see it again, so there was
	 * nowhere to take one back from.
	 *
	 * Kept out of the inbox badge on purpose — these are not waiting on you.
	 */
	let sentSittings: SentSitting[] = [];
	let openedSitting: HandedSittingDetail | null = null;
	let spoolPicked: Record<string, boolean> = {};
	let spoolTitle = '';
	let showSpoolDialog = false;

	// Ticks the elapsed clock on the running banner. Assigned only while a
	// sitting is open, and the assignment is what re-renders — so an idle page
	// pays for a timer callback and nothing else.
	let nowSec = Math.floor(Date.now() / 1000);

	// Who you can sit with: a deck you are patched with, **in either
	// direction** — the rule a dub follows, for the reason a dub follows it
	// (an invitation puts a banner on somebody's screen). `start_spool`
	// enforces it, so a list built any wider here would only produce a 403 at
	// the moment of asking.
	$: spoolCandidates = (() => {
		const out = new Map<string, PatchPeer>();
		for (const p of [...following, ...followers]) {
			if (p.state === 'accepted') out.set(p.username, p);
		}
		return [...out.values()].sort((a, b) =>
			(a.display_name || a.username).localeCompare(b.display_name || b.username)
		);
	})();
	$: spoolPickedCount = Object.values(spoolPicked).filter(Boolean).length;
	// Names `nowSec`, which is what makes it tick.
	// A sitting that has ended does not have a running clock, and drawing a
	// ticking one would say it is still going. It gets the **date** instead:
	// the question about an invitation you never answered is *which evening was
	// this*, and a span answers a question nobody asked — it also reads 0:00
	// for a room that was closed straight after it opened. How long it ran
	// moves to the subline, where it is a detail rather than the headline.
	//
	// `nowSec` is still *named* in the expression for a live sitting, which is
	// what makes it tick; see the note above `clock`.
	$: spoolClock = spool
		? spool.ended_at
			? new Date(spool.started_at * 1000).toLocaleDateString(undefined, {
					day: 'numeric',
					month: 'short'
				})
			: clock(spool.started_at, nowSec)
		: '';
	$: spoolRan =
		spool && spool.ended_at
			? spool.ended_at - spool.started_at < 60
				? 'it ran briefly'
				: `it ran for ${clock(spool.started_at, spool.ended_at)}`
			: '';

	// Your own handle and how open your deck is. Two switches, and the pill has
	// to be honest about both — see `deckState` for why it is not just
	// "private".
	let myHandle = '';
	let federating = false;
	let npPublic = false;

	async function loadMe() {
		try {
			const f = await api.getFederation();
			// Only when there is an actor. `handle` is a *preview* built from
			// today's origin until a keypair is minted, and showing an address
			// that resolves nowhere is the bug v0.78.0 found on the settings
			// page.
			myHandle = f.actor ? f.handle : '';
			federating = f.enabled;
		} catch {
			/* the header simply shows no handle */
		}
		try {
			npPublic = (await api.getPublicNowPlaying()).enabled;
		} catch {
			/* likewise */
		}
	}

	/**
	 * What the pill says.
	 *
	 * The design has a static "Deck is private", which is true of a fresh
	 * instance and quietly wrong the moment either switch is on. They answer
	 * different questions — being *findable* and being *readable* — and both
	 * are off by default, so the honest pill names whichever is on rather than
	 * asserting privacy the instance may no longer have.
	 */
	$: deckState = !federating && !npPublic
		? { label: 'Deck is private', open: false }
		: federating && npPublic
			? { label: 'Discoverable · deck readable', open: true }
			: federating
				? { label: 'Discoverable', open: true }
				: { label: 'Now playing is public', open: true };

	// A sitting captures only from shared chains, so a host who publishes
	// nothing passes nothing on. Read from the chains endpoint rather than
	// inferred from an empty capture list, which would be indistinguishable
	// from "nothing has played yet".
	let mySharedChains = 0;
	// The full list, so the Decks tab can offer the switch. **One state, two
	// places** is real rather than aspirational because this and the Signal
	// Chains drawer both call `PUT /api/v1/patch/chains/:id/shared` — flipping
	// it here and reloading that page shows the same answer.
	let myChains: SignalChain[] = [];
	let sharingChain: number | null = null;

	async function setShared(chain: SignalChain, on: boolean) {
		if (chain.shared === on) return;
		sharingChain = chain.id;
		try {
			await api.setChainShared(chain.id, on);
			myChains = myChains.map((c) => (c.id === chain.id ? { ...c, shared: on } : c));
			mySharedChains = myChains.filter((c) => c.shared).length;
			// What the Reel shows changes with it, so it is re-read rather than
			// left describing the previous setting.
			await loadReel();
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not save');
			// Re-read rather than keeping the optimistic value: what is published
			// is not something to guess at. The same call the chains page makes.
			try {
				myChains = (await api.getChains()).chains;
				mySharedChains = myChains.filter((c) => c.shared).length;
			} catch {
				/* leave the row as it was */
			}
		} finally {
			sharingChain = null;
		}
	}

	$: pendingCaptures = captures.filter((c) => c.state === 'pending');

	// Grouped by the sitting they came from, newest evening first. A capture on
	// its own is a track with no context; what makes it answerable is *which
	// evening* and *whose deck*.
	$: captureSittings = (() => {
		const by = new Map<string, { uuid: string; title: string; from: string; when: string; rows: SpoolCapture[] }>();
		for (const c of pendingCaptures) {
			let g = by.get(c.spool_uuid);
			if (!g) {
				g = {
					uuid: c.spool_uuid,
					title: c.spool_title || 'A sitting',
					from: c.from_display_name || c.from_username,
					when: '',
					rows: []
				};
				by.set(c.spool_uuid, g);
			}
			g.rows.push(c);
		}
		for (const g of by.values()) {
			g.rows.sort((a, b) => a.played_at - b.played_at);
			const first = g.rows[0]?.played_at;
			const last = g.rows[g.rows.length - 1]?.played_at;
			const d = new Date(first * 1000);
			const t = (x: number) =>
				new Date(x * 1000).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
			g.when = `${d.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' })} · ${t(first)} – ${t(last)} · from ${g.from}`;
		}
		return [...by.values()].sort((a, b) => b.rows[0].played_at - a.rows[0].played_at);
	})();

	// Everything starts ticked: the common answer to "you were in the room for
	// these" is yes, so the work should be un-ticking the odd one rather than
	// ticking all twelve.
	//
	// **Exclusions are the state, and `picked` is derived from them.** The
	// obvious version keeps the ticked set and tops it up in a reactive block —
	// but that block has to *read* `picked` to preserve what was un-ticked and
	// *assign* it to adopt new captures, which is a self-referencing cycle:
	// un-ticking a row re-ticked itself on the next run. Storing what was
	// removed has no such cycle, and it also survives the 15s capture poll
	// without the reader's choices being reset under them.
	let unpicked = new Set<number>();
	$: picked = new Set(pendingCaptures.map((c) => c.id).filter((id) => !unpicked.has(id)));

	function togglePick(id: number) {
		const next = new Set(unpicked);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		unpicked = next;
	}

	const pickedIn = (sit: { rows: SpoolCapture[] }) =>
		sit.rows.filter((c) => picked.has(c.id)).map((c) => c.id);
	$: openDubs = dubs.filter((d) => d.status === 'open');
	$: unseenSittings = sittings.filter((h) => !h.seen_at);
	// One number on the tab. Three kinds of thing wait in that inbox and a
	// person does not care which until they open it.
	$: inboxCount =
		pending.length + pendingCaptures.length + openDubs.length + unseenSittings.length;

	// Waiting on *you*. The inbox count is the one number that should pull
	// someone to a tab they were not going to open.
	$: pending = followers.filter((f) => f.state === 'pending');
	$: accepted = following.filter((f) => f.state === 'accepted');
	// A patch into a deck that publishes nothing is accepted and reads empty.
	// That is correct, and saying so is the difference between an honest empty
	// state and one that looks like a bug.
	$: silent = accepted.filter((f) => f.shared_chains === 0);

	async function loadDecks() {
		try {
			const d = await api.getPatchDecks();
			decks = d.decks;
			following = d.following;
			followers = d.followers;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not load';
		}
	}

	async function loadArtists() {
		artistsLoaded = true;
		try {
			artists = (await api.getPatchArtists(artistDays ?? undefined)).artists;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not load';
		}
	}

	async function loadDiscover() {
		discoverLoaded = true;
		try {
			discoverDecks = (await api.getPatchDiscover()).decks;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not load';
		}
	}

	async function loadInstance() {
		instanceLoaded = true;
		try {
			const r = await api.getPatchInstance(pulseDays);
			pulse = r.pulse;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not load';
		}
		// Admin-only, and a 403 here is a normal answer for most people rather
		// than an error worth showing.
		try {
			const o = await api.getPatchOps();
			ops = o.ops;
			vocab = o.vocabulary;
			isAdmin = true;
		} catch {
			isAdmin = false;
		}
	}

	/** The user's own answer to "do these reach my scrobble services". Read
	 *  here because the inbox states it in words right above the Accept button,
	 *  and a claim that is hardcoded is one that goes quietly false the moment
	 *  somebody flips the switch. */
	let forwardSpool = false;
	let forwardWindowDays = 14;

	async function loadForwarding() {
		try {
			const r = await api.getSpoolForwarding();
			forwardSpool = r.enabled;
			forwardWindowDays = r.window_days;
		} catch {
			// A failed read must not claim the safer answer any more than the
			// riskier one, so it leaves the default alone and the copy simply
			// says less.
		}
	}

	async function loadSpool() {
		try {
			const r = await api.getSpool();
			spool = r.spool;
			captures = r.captures;
		} catch {
			/* the tab shows its own empty state */
		}
	}

	async function loadLive() {
		try {
			live = (await api.getPatchLive()).live;
		} catch {
			/* the strip simply does not render */
		}
	}

	async function loadSittings() {
		try {
			sittings = (await api.getSittings()).sittings;
			sentSittings = (await api.getSentSittings()).sittings;
		} catch {
			/* the section simply does not render */
		}
	}

	/**
	 * Open the document, and mark it seen in the same gesture.
	 *
	 * Marking on *open* rather than behind a second button because there is
	 * nothing to decide — it wrote nothing to a history, so "seen" is the whole
	 * of the state. Dismiss exists for the case where you do not want to read
	 * it now and do not want it nagging.
	 */
	async function openSitting(h: HandedSitting) {
		busy = h.uuid;
		try {
			openedSitting = await api.getSitting(h.uuid);
			await api.markSittingSeen(h.uuid);
			sittings = sittings.map((x) =>
				x.uuid === h.uuid ? { ...x, seen_at: Math.floor(Date.now() / 1000) } : x
			);
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not open that');
		} finally {
			busy = null;
		}
	}

	/**
	 * Take a handed-over sitting back.
	 *
	 * Genuinely undoes it: the document is rendered from *your* database on
	 * every read, so removing the row is the whole of it — the divergence from
	 * "no delete sweep on un-patching" that Phase 6 already records, and it
	 * holds only because nothing has been copied anywhere. When this federates
	 * it becomes a `Delete` to send rather than a row to remove.
	 */
	/**
	 * Read back one you sent.
	 *
	 * Its own function rather than `openSitting`, which also marks the thing
	 * **seen** — that is the recipient's state, and looking at your own copy is
	 * not them reading it. `handed_sitting` already lets either end read it, so
	 * this is the same request without the side effect.
	 */
	async function viewSentSitting(h: SentSitting) {
		busy = h.uuid;
		try {
			openedSitting = await api.getSitting(h.uuid);
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not open that');
		} finally {
			busy = null;
		}
	}

	async function withdrawSitting(h: SentSitting) {
		const who = h.to_display_name || `@${h.to_username}`;
		if (
			!confirm(
				h.seen_at
					? `Take "${h.title || 'this sitting'}" back from ${who}?\n\nThey have already read it — this only stops them opening it again.`
					: `Take "${h.title || 'this sitting'}" back from ${who}?\n\nThey have not opened it yet, so it will simply disappear.`
			)
		)
			return;
		busy = h.uuid;
		try {
			await api.withdrawSitting(h.uuid);
			sentSittings = sentSittings.filter((x) => x.uuid !== h.uuid);
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not withdraw that');
		} finally {
			busy = null;
		}
	}

	async function dismissSitting(h: HandedSitting) {
		busy = h.uuid;
		try {
			await api.markSittingSeen(h.uuid);
			sittings = sittings.map((x) =>
				x.uuid === h.uuid ? { ...x, seen_at: Math.floor(Date.now() / 1000) } : x
			);
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not save that');
		} finally {
			busy = null;
		}
	}

	async function loadDubs() {
		try {
			dubs = (await api.getDubs()).dubs;
		} catch {
			/* likewise */
		}
	}

	async function loadReel(more = false) {
		const seq = ++reelSeq;
		reelLoading = true;
		try {
			const page = await api.getReel({
				limit: 40,
				before: more ? (next ?? undefined) : undefined,
				q: reelQ.trim() || undefined,
				deck: reelDeck ?? undefined
			});
			// A response for filters that have since changed is dropped rather
			// than drawn. Without this the list can end up disagreeing with the
			// controls, which reads as the filter not working.
			if (seq !== reelSeq) return;
			listens = more ? [...listens, ...page.listens] : page.listens;
			next = page.next;
		} catch (e) {
			if (seq !== reelSeq) return;
			error = e instanceof Error ? e.message : 'Could not load the Reel';
		} finally {
			if (seq === reelSeq) reelLoading = false;
		}
	}

	onMount(async () => {
		const t = new URLSearchParams(window.location.search).get('tab');
		if (t && (TABS as string[]).includes(t)) tab = t as Tab;
		if (tab === 'artists') void loadArtists();
		if (tab === 'discover') void loadDiscover();
		if (tab === 'instance') void loadInstance();
		await Promise.all([
			loadDecks(),
			loadReel(),
			loadSpool(),
			loadForwarding(),
			loadDubs(),
			loadSittings(),
			loadLive(),
			loadMe(),
			// The heart on a Reel row reads this store, so it has to be filled
			// before the rows draw or every one of them starts empty and fills
			// in a moment later.
			loadLoves()
		]);
		try {
			myChains = (await api.getChains()).chains;
			mySharedChains = myChains.filter((c) => c.shared).length;
		} catch {
			/* the warning simply does not show */
		}
		loading = false;

		// A capture is written by somebody else's deck, so nothing on this page
		// would otherwise learn one had arrived. Only while a sitting is
		// running — there is nothing to poll for otherwise.
		//
		// Cleared in `onDestroy` rather than returned from here: an *async*
		// `onMount` returns a promise, and Svelte cannot use that as a teardown
		// — the timer would outlive the page and keep polling forever.
		timer = setInterval(() => {
			if (spool) void loadSpool();
		}, 15000);
		// The elapsed clock. Separate from the poll because it is a second a
		// tick rather than fifteen, and it assigns only while a sitting is
		// open — an idle page re-renders nothing.
		tick = setInterval(() => {
			if (spool) nowSec = Math.floor(Date.now() / 1000);
			// The strip's bars are extrapolated between polls, the same way the
			// dashboard deck does it — otherwise a progress bar only moves every
			// few seconds and reads as stuck.
			if (live.length) livesSec = Math.floor(Date.now() / 1000);
		}, 1000);
		// Only on the Reel, where the strip is drawn. Somebody reading the
		// Instance tab does not need the registry polled at them.
		liveTimer = setInterval(() => {
			if (tab === 'reel') void loadLive();
		}, LIVE_POLL_MS);
	});

	const LIVE_POLL_MS = 10_000;
	let livesSec = Math.floor(Date.now() / 1000);
	let timer: ReturnType<typeof setInterval> | undefined;
	let tick: ReturnType<typeof setInterval> | undefined;
	let liveTimer: ReturnType<typeof setInterval> | undefined;
	onDestroy(() => {
		clearInterval(timer);
		clearInterval(tick);
		clearInterval(liveTimer);
	});

	/**
	 * Where the bar is *now*, extrapolated from the last poll.
	 *
	 * `livesSec` is named in the expression rather than read inside a helper,
	 * which is the Svelte trap this file has now hit three times: a template
	 * expression re-runs when the variables it *names* change and does not
	 * trace them through a call.
	 */
	function livePos(l: LiveDeck, now: number): number {
		if (l.paused) return l.position;
		const at = l.position + Math.max(0, now - liveReadAt);
		return l.duration ? Math.min(at, l.duration) : at;
	}
	let liveReadAt = Math.floor(Date.now() / 1000);
	$: if (live) liveReadAt = Math.floor(Date.now() / 1000);

	function go(t: Tab) {
		tab = t;
		if (t === 'artists' && !artistsLoaded) void loadArtists();
		if (t === 'discover' && !discoverLoaded) void loadDiscover();
		if (t === 'instance' && !instanceLoaded) void loadInstance();
		// Through `syncUrl`, never `history.replaceState` — the latter wipes
		// SvelteKit's own history index and breaks the Back button silently.
		const url = new URL(window.location.href);
		if (t === 'reel') url.searchParams.delete('tab');
		else url.searchParams.set('tab', t);
		syncUrl(url.pathname + url.search);
	}

	async function patch(username: string) {
		busy = username;
		try {
			await api.requestPatch(username);
			await loadDecks();
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not ask');
		} finally {
			busy = null;
		}
	}

	async function withdraw(username: string) {
		busy = username;
		try {
			await api.unpatch(username);
			await Promise.all([loadDecks(), loadReel()]);
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not withdraw');
		} finally {
			busy = null;
		}
	}

	async function settle(p: PatchPeer, accept: boolean) {
		if (!p.uuid) return;
		busy = p.username;
		try {
			await api.settlePatch(p.uuid, accept);
			await loadDecks();
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not save');
		} finally {
			busy = null;
		}
	}

	function openSpool() {
		spoolTitle = '';
		spoolPicked = {};
		showSpoolDialog = true;
	}

	// `now` is a **parameter**, and that is load-bearing. A Svelte template
	// expression re-runs when the variables it *names* change; it does not
	// trace them through a call — so `{clock(spool.started_at)}` reading
	// `nowSec` from the closure renders once and sits there, which is what a
	// stuck 0:00 looked like on screen. Taking it as an argument forces the
	// caller to name it, so `spoolClock` below cannot be written the wrong
	// way. Same trap as `noteCurrentTrack` and `sideTotals` on the shelf.
	/** Everyone in the sitting but you. */
	const others = (sp: SpoolView): SpoolMember[] =>
		sp.members.filter((m) => m.state === 'joined' && m.user_id !== sp.viewer_id);

	/** "with Noor and Tobias" — the design's main line, and the honest one: a
	 *  sitting is who is in it. */
	function withLabel(sp: SpoolView): string {
		const names = others(sp).map((m) => m.display_name || m.username);
		if (!names.length) return sp.hosting ? 'no one yet' : 'just you so far';
		if (names.length === 1) return `with ${names[0]}`;
		const last = names[names.length - 1];
		return `with ${names.slice(0, -1).join(', ')} and ${last}`;
	}

	const invitedCount = (sp: SpoolView) => sp.members.filter((m) => m.state === 'invited').length;

	function clock(from: number, now: number): string {
		const s = Math.max(0, now - from);
		const h = Math.floor(s / 3600);
		const m = Math.floor((s % 3600) / 60);
		return h ? `${h}:${String(m).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`
			: `${m}:${String(s % 60).padStart(2, '0')}`;
	}

	async function startSpool() {
		const with_ = Object.keys(spoolPicked).filter((u) => spoolPicked[u]);
		busy = 'spool';
		try {
			await api.startSpool({ title: spoolTitle.trim() || null, with: with_ });
			showSpoolDialog = false;
			spoolTitle = '';
			spoolPicked = {};
			await loadSpool();
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not start that');
		} finally {
			busy = null;
		}
	}

	async function answerSpool(accept: boolean) {
		if (!spool) return;
		const wasOver = !!spool.ended_at;
		busy = 'spool';
		try {
			const r = await api.joinSpool(spool.uuid, accept);
			await loadSpool();
			// Joining answers the invitation, so the banner goes — which for a
			// sitting that had already ended leaves the screen looking as if
			// nothing happened. Say where the plays went, and only for the case
			// that needs it: a live sitting keeps its banner and its counter.
			if (accept && wasOver) {
				notice = r.offered
					? r.offered === 1
						? '1 play from that sitting is waiting in your inbox.'
						: `${r.offered} plays from that sitting are waiting in your inbox.`
					: 'Joined. That sitting had nothing to pass on — the plays ran through chains the host does not share.';
			}
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not save');
		} finally {
			busy = null;
		}
	}

	async function endSpool() {
		if (!spool) return;
		busy = 'spool';
		try {
			await api.endSpool(spool.uuid);
			await loadSpool();
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not save');
		} finally {
			busy = null;
		}
	}

	async function settleCaptures(ids: number[], accept: boolean) {
		if (!ids.length) return;
		busy = 'captures';
		try {
			const r = await api.settleCaptures(ids, accept);
			// Only when something was actually relayed. Accepting writes to a
			// permanent public record in that case, and a click that does so
			// silently is the one thing this switch exists to stop being a
			// surprise. Saying it every time — including when nothing was
			// forwarded — would make it noise nobody reads.
			if (accept && r.forwarded) {
				notice =
					r.forwarded === r.settled
						? `Kept, and forwarded on to your scrobble services.`
						: `Kept. ${r.forwarded} of ${r.settled} were forwarded on — the rest were older than ${forwardWindowDays} days.`;
			}
			await Promise.all([loadSpool(), loadReel()]);
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not save');
		} finally {
			busy = null;
		}
	}

	function initials(p: { display_name: string | null; username: string }) {
		const n = (p.display_name || p.username).trim();
		const parts = n.split(/\s+/).filter(Boolean);
		return ((parts[0]?.[0] ?? '') + (parts[1]?.[0] ?? '')).toUpperCase() || n.slice(0, 2).toUpperCase();
	}

	function ago(ts: number) {
		const s = Math.max(0, Math.floor(Date.now() / 1000) - ts);
		if (s < 60) return `${s}s`;
		if (s < 3600) return `${Math.floor(s / 60)}m`;
		if (s < 86400) return `${Math.floor(s / 3600)}h`;
		return `${Math.floor(s / 86400)}d`;
	}

	function dayLabel(ts: number) {
		const d = new Date(ts * 1000);
		const today = new Date();
		const y = new Date(today);
		y.setDate(today.getDate() - 1);
		const same = (a: Date, b: Date) => a.toDateString() === b.toDateString();
		if (same(d, today)) return 'Today';
		if (same(d, y)) return 'Yesterday';
		return d.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' });
	}

	// Grouped by day the way the design draws it, and by the *browser's* day —
	// the Reel is a live feed rather than an analytic, so there is no ranged
	// query behind it whose buckets would have to honour the user's timezone
	// preference.
	$: days = (() => {
		const out: { label: string; rows: ReelListen[] }[] = [];
		for (const l of listens) {
			const label = dayLabel(l.timestamp);
			if (out.length && out[out.length - 1].label === label) out[out.length - 1].rows.push(l);
			else out.push({ label, rows: [l] });
		}
		return out;
	})();

	function onReelQ() {
		clearTimeout(reelTimer);
		reelTimer = setTimeout(() => loadReel(), 300);
	}

	function pickDeck(username: string | null) {
		reelDeck = reelDeck === username ? null : username;
		void loadReel();
	}

	/** `@user@host`, from **their** actor rather than today's origin. */
	function handleOf(l: ReelListen): string | null {
		if (!l.actor_uri) return null;
		try {
			return `@${l.username}@${new URL(l.actor_uri).host}`;
		} catch {
			return null;
		}
	}

	function steps(l: ReelListen): { name: string; type: string }[] {
		try {
			return JSON.parse(l.chain_components ?? '[]');
		} catch {
			return [];
		}
	}

	const ROLE_COLOURS: Record<string, string> = {
		source: 'var(--td-accent)',
		dac: 'var(--td-accent-2)',
		amp: 'var(--td-muted-2)',
		transducer: 'var(--td-accent-hi)'
	};
	const roleColour = (t: string) => ROLE_COLOURS[t] ?? 'var(--td-muted-3)';

	/** `mm:ss`, or `h:mm:ss` past an hour. */
	function clockOf(secs: number): string {
		const s = Math.max(0, Math.floor(secs));
		const h = Math.floor(s / 3600);
		const m = Math.floor((s % 3600) / 60);
		const r = String(s % 60).padStart(2, '0');
		return h ? `${h}:${String(m).padStart(2, '0')}:${r}` : `${m}:${r}`;
	}

	function liveClock(l: LiveDeck, now: number): string {
		const at = clockOf(livePos(l, now));
		const head = l.position_known ? at : `≈${at}`;
		return l.duration ? `${head} / ${clockOf(l.duration)}` : head;
	}

	function liveBadge(l: LiveDeck): string {
		if (l.format_type === 'dsd') return 'DSD';
		if (l.is_lossless && l.sample_rate && l.bit_depth)
			return `${(l.codec ?? 'FLAC').toUpperCase()} ${l.bit_depth}/${Math.round(l.sample_rate / 1000)}`;
		if (l.is_lossless) return (l.codec ?? 'Lossless').toUpperCase();
		return '';
	}

	function badge(l: ReelListen) {
		if (l.format_type === 'dsd') return 'DSD';
		if (l.is_lossless && l.sample_rate && l.bit_depth)
			return `${(l.codec ?? 'FLAC').toUpperCase()} ${l.bit_depth}/${Math.round(l.sample_rate / 1000)}`;
		if (l.is_lossless) return (l.codec ?? 'Lossless').toUpperCase();
		if (l.bitrate) return `${(l.codec ?? 'MP3').toUpperCase()} ${l.bitrate}`;
		return '';
	}
</script>

<svelte:head><title>Patch — Tapedeck</title></svelte:head>

<div class="head">
	<div>
		<h1 class="h-display">Patch</h1>
		<p class="sub">Patch into another deck and hear what it's running.</p>
	</div>
	<div class="headright">
		<div class="whoami">
			{#if myHandle}<span class="myhandle">{myHandle}</span>{/if}
			<!-- Links to where it is changed, rather than being a label that
			     leaves you hunting for the switch. -->
			<a class="tag deckpill" class:open={deckState.open} href="/settings#sharing">
				{deckState.label}
			</a>
		</div>
	<!--
		Only when there is somebody to sit with and nothing already running.
		One sitting per host is a partial unique index, so offering the button
		during one would be offering a 409 — and with no accepted patch there
		is nobody the server would let you invite.
	-->
	{#if !loading && !spool && spoolCandidates.length}
		<button class="btn btn-secondary start-spool" on:click={openSpool}>
			<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" aria-hidden="true">
				<circle cx="8" cy="12" r="4.4" /><circle cx="16" cy="12" r="4.4" />
			</svg>
			Start a Shared Spool
		</button>
	{/if}
	</div>
</div>

<div class="seg">
	{#each TABS as t}
		<button class="seg-btn" class:on={tab === t} on:click={() => go(t)}>
			{TAB_LABELS[t]}{#if t === 'inbox' && inboxCount}<span class="count">{inboxCount}</span>{/if}
		</button>
	{/each}
</div>

<!--
	The sitting, above everything. It is the one thing on this page that is
	happening *now*, and burying it in a tab would mean the person whose deck is
	feeding it never sees it.
-->
{#if spool}
	<div class="spool" class:invited={spool.my_state === 'invited'} class:over={!!spool.ended_at}>
		<!--
			The people first, then the pulse. The design leads with an avatar
			stack and "with Noor and Tobias" — a sitting is *who you are in a
			room with*, and the name you gave it is the secondary fact.
		-->
		{#if others(spool).length}
			<div class="savatars">
				{#each others(spool).slice(0, 4) as m (m.user_id)}
					<span class="sav" title={m.display_name || m.username}>
						{(m.display_name || m.username).slice(0, 2).toUpperCase()}
					</span>
				{/each}
			</div>
		{/if}
		<!-- The pulse says "happening now", so an ended sitting does not get one. -->
		{#if !spool.ended_at}
			<div class="spulse"><span></span><span></span></div>
		{/if}
		<div class="smain">
			<div class="slabel">
				{spool.ended_at
					? 'You were asked into a Shared Spool that has since ended'
					: spool.my_state === 'invited'
						? 'You were asked into a Shared Spool'
						: 'Shared Spool running'}
			</div>
			<div class="stitle">{withLabel(spool)}</div>
			<div class="smembers">
				{#if spool.title}{spool.title}{/if}
				{#if spool.title && invitedCount(spool)} · {/if}
				{#if invitedCount(spool)}{invitedCount(spool)} invited{/if}
			</div>
			<!--
				A sitting does not bypass the sharing switch, so a host who
				publishes no chain captures nothing — and that has to be said
				here rather than discovered from an empty inbox.
			-->
			<!--
				The reported bug in one line. With the room closed, "Join" reads
				as though it can only be pointless — so the banner says what it
				actually does, which is hand over the evening it held.
			-->
			{#if spool.ended_at && spool.my_state === 'invited'}
				<div class="swarn">
					Joining still works. It puts the plays from that sitting in your
					inbox, and each one is yours to keep or drop.
				</div>
			{/if}
			{#if spool.hosting && !mySharedChains}
				<div class="swarn">
					You share no chains, so nothing you play is being passed on.
					<a href="/chains">Share one</a> to change that.
				</div>
			{/if}
		</div>
		<div class="sclock">
			<div class="ctime">{spoolClock}</div>
			<div class="csub">
				{#if spool.ended_at}{spoolRan}{:else}{spool.captured}
					{spool.captured === 1 ? 'play' : 'plays'} offered to you{/if}
			</div>
		</div>
		<div class="sacts">
			{#if spool.my_state === 'invited'}
				<button class="btn btn-primary" disabled={busy === 'spool'} on:click={() => answerSpool(true)}>Join</button>
				<button class="btn btn-secondary" disabled={busy === 'spool'} on:click={() => answerSpool(false)}>No thanks</button>
			{:else}
				{#if pendingCaptures.length}
					<button class="btn btn-secondary" on:click={() => go('inbox')}>
						Review {pendingCaptures.length}
					</button>
				{/if}
				{#if spool.hosting}
					<button class="btn btn-primary" disabled={busy === 'spool'} on:click={endSpool}>End spool</button>
				{:else}
					<button class="btn btn-secondary" disabled={busy === 'spool'} on:click={() => answerSpool(false)}>Leave</button>
				{/if}
			{/if}
		</div>
	</div>
{/if}

{#if error}
	<div class="panel note">{error}</div>
{/if}

{#if notice}
	<div class="panel note said">
		{notice}
		<button class="linkish" on:click={() => (notice = '')}>Dismiss</button>
	</div>
{/if}

{#if loading}
	<div class="panel note">Loading…</div>
{:else if tab === 'reel'}
	<!--
		Rolling now. Only decks with an accepted patch playing through a **shared
		chain** — the same gate the Reel below uses, applied to a live entry
		because `NowPlaying` carries the chain its listen will get.
	-->
	{#if live.length}
		<div class="livehead">
			<span class="livedot"></span>
			<h2 class="h-display sech" style="margin:0;">Rolling now</h2>
			<span class="esub">{live.length} {live.length === 1 ? 'deck' : 'decks'} live</span>
		</div>
		<div class="livegrid">
			{#each live as l (l.user_id)}
				<div class="livecard">
					<div class="livetop">
						<span class="avatar small">{(l.display_name || l.username).slice(0, 2).toUpperCase()}</span>
						<div style="min-width:0;flex:1;">
							<a class="livename" href="/users/{l.username}">{l.display_name || l.username}</a>
							<div class="phandle">@{l.username}</div>
						</div>
						{#if l.paused}
							<span class="tag">Paused</span>
						{:else}
							<!-- Bars rather than a dot: a still deck and a playing one
							     should not look the same at a glance. -->
							<span class="vu" aria-hidden="true">
								{#each [0, 1, 2, 3, 4] as i}<span style="animation-delay:{i * 0.12}s"></span>{/each}
							</span>
						{/if}
					</div>
					<div class="livetitle">{l.title}</div>
					<div class="liveartist">{l.artist}{l.album ? ` — ${l.album}` : ''}</div>
					<div class="livebar" class:guess={!l.position_known}>
						<div style="width:{l.duration ? Math.min(100, (livePos(l, livesSec) / l.duration) * 100) : 0}%"></div>
					</div>
					<div class="livefoot">
						<span class="tag">{l.chain_name}</span>
						{#if liveBadge(l)}<span class="tag">{liveBadge(l)}</span>{/if}
						<!-- Built in one expression rather than with an inline
						     `{#if}`: Svelte trims the whitespace around a block, so
						     the separator came out as "3:16/ 5:48". An estimate is
						     also marked as one — presenting wall-clock arithmetic as
						     a playhead is the mistake `Progress` exists to stop. -->
						<span class="livetime">{liveClock(l, livesSec)}</span>
					</div>
				</div>
			{/each}
		</div>
	{/if}

	{#if !accepted.length}
		<div class="panel empty">
			<div class="h-display" style="font-size:19px;margin-bottom:6px;">Nothing on the Reel yet</div>
			<div class="esub">You haven't patched into anyone. Find a deck under <button class="linkish" on:click={() => go('decks')}>Decks</button>.</div>
		</div>
	{:else if !listens.length}
		<div class="panel empty">
			<div class="h-display" style="font-size:19px;margin-bottom:6px;">Quiet so far</div>
			<div class="esub">
				{#if silent.length === accepted.length}
					{silent.length === 1 ? 'The deck you patched into publishes' : 'The decks you patched into publish'}
					no chains yet — so there is nothing to show. Sharing is per chain and off by default,
					which is theirs to switch on, not yours.
				{:else}
					Nothing has played through a shared chain yet.
				{/if}
			</div>
		</div>
	{:else}
		<!--
			Search and the deck chips. Both hit the server: the Reel is
			keyset-paged, so narrowing what has already loaded would search only
			what you happened to have scrolled to — and say nothing about it,
			which reads exactly like a search that found nothing.
		-->
		<div class="reelctl">
			<div class="reelsearch">
				<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" aria-hidden="true">
					<circle cx="11" cy="11" r="7" /><path d="M16.5 16.5L21 21" />
				</svg>
				<input
					class="input"
					bind:value={reelQ}
					on:input={onReelQ}
					placeholder="Search track, artist, chain or deck"
					aria-label="Search the Reel"
				/>
			</div>
			{#if accepted.length > 1}
				<div class="deckchips">
					<button class="chip" class:on={!reelDeck} on:click={() => pickDeck(null)}>All</button>
					{#each accepted as p (p.username)}
						<button
							class="chip"
							class:on={reelDeck === p.username}
							title={p.display_name || p.username}
							on:click={() => pickDeck(p.username)}
						>{(p.display_name || p.username).slice(0, 2).toUpperCase()}</button>
					{/each}
				</div>
			{/if}
		</div>

		<div class="feed">
			{#each days as day}
				<div>
					<div class="daybar">
						<span class="daylabel">{day.label}</span>
						<span class="rule"></span>
						<span class="daycount">{day.rows.length}</span>
					</div>
					<div class="listrows">
						{#each day.rows as l (l.id)}
							<div class="lrow">
								<!--
									The deck's avatar, not the sleeve. The Reel's
									organising principle is *whose* — the design puts
									the person first, and `avatar_url` had been in
									this payload since v0.80.0 drawn nowhere.
								-->
								<a class="ravatar" href="/users/{l.username}" title={l.display_name || l.username}>
									{#if l.avatar_url}
										<img src={l.avatar_url} alt="" />
									{:else}
										{(l.display_name || l.username).slice(0, 2).toUpperCase()}
									{/if}
								</a>
								<CoverArt
									url={l.artwork_url}
									caaId={l.caa_id}
									releaseMbid={l.caa_release_mbid}
									seed={l.album || l.artist}
									size={32}
								/>
								<div class="lmain">
									<div class="ltop">
										<span class="ltitle">{l.title}</span>
										<button class="xref lartist" on:click={() => openArtist(l.artist)}>{l.artist}</button>
									</div>
									<div class="lsub">
										<a class="lwho" href="/users/{l.username}">{l.display_name || l.username}</a>
										{#if handleOf(l)}<span class="rhandle">{handleOf(l)}</span>{/if}
										<span class="lchain">
											<ChainIcon name={l.chain_icon} size={13} title={l.chain_name} />
											{l.chain_name}
										</span>
									</div>
								</div>
								<div class="ldots" title={steps(l).map((x) => x.name).join(' → ')}>
									{#each steps(l).slice(0, 4) as st}
										<span class="dot" style="background:{roleColour(st.type)};"></span>
									{/each}
								</div>
								{#if badge(l)}<span class="tag">{badge(l)}</span>{/if}
								<span class="lago">{ago(l.timestamp)}</span>
								<!--
									Loving somebody else's listen loves the *track*, into
									your own loves — it does nothing to their deck. The
									key is derived from artist and title, so it works
									without the listen being yours.
								-->
								<button
									class="rlove"
									class:on={isLovedNamed($lovedKeys, 'track', l.artist, l.title)}
									title={isLovedNamed($lovedKeys, 'track', l.artist, l.title)
										? 'Loved — click to remove'
										: 'Love this track'}
									on:click={() => toggleLoveByName('track', l.artist, l.title)}
								>
									<svg viewBox="0 0 24 24" width="15" fill={isLovedNamed($lovedKeys, 'track', l.artist, l.title) ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20.3l-1.35-1.2C5.9 14.85 3 12.2 3 8.9 3 6.3 5 4.3 7.5 4.3c1.6 0 3.2.8 4.5 2.6 1.3-1.8 2.9-2.6 4.5-2.6C19 4.3 21 6.3 21 8.9c0 3.3-2.9 5.95-7.65 10.2z"/></svg>
								</button>
							</div>
						{/each}
					</div>
				</div>
			{/each}
		</div>
		{#if next}
			<div class="more">
				<button class="btn btn-secondary" disabled={reelLoading} on:click={() => loadReel(true)}>
					{reelLoading ? 'Loading…' : 'Older'}
				</button>
			</div>
		{/if}
	{/if}

	<p class="footnote">
		Listens only reach the Reel from chains their owner switched to shared — everything defaults
		to private. <a href="/chains">Choose what your deck publishes</a>.
	</p>
{:else if tab === 'artists'}
	<div class="secbar">
		<h2 class="h-display sech">Artists</h2>
		<span class="seccount">{artists.length}</span>
		<span style="flex:1;"></span>
		<span class="rankedby">{RANGE_LABEL[artistDays ?? 0]}</span>
	</div>
	<p class="sub">
		Across the decks you've patched into, from the chains they share. The number beside a card
		is <strong>your own plays</strong> — the overlap is shown as which artists, never as a
		score.
	</p>

	<!--
		Sort is client-side and the range is not, and the asymmetry is the point:
		the range decides *which* artists come back (the query cuts to the top N
		by plays), so narrowing here would rank a week among artists chosen on
		all-time totals. Reordering a set already in hand is free and exact.
	-->
	<div class="chiprow">
		{#each SORTS as srt}
			<button class="chip" class:on={artistSort === srt.key} on:click={() => (artistSort = srt.key)}>
				{srt.label}
			</button>
		{/each}
	</div>
	<div class="seg rangeseg">
		{#each RANGES as r}
			<button class="seg-btn" class:on={artistDays === r.days} on:click={() => setArtistDays(r.days)}>
				{r.label}
			</button>
		{/each}
	</div>

	{#if artists.length}
		<div class="agrid">
			{#each sortedArtists as a (a.name)}
				<!--
					A card is a link now. "I cannot click the artist to see the
					stats" — and it went nowhere because it was a `<div>`. Through
					`openArtist`, never a bare href: the row knows a name and the
					entity may not exist yet, so it resolves first.
				-->
				<button
					class="acard"
					class:both={a.mine > 0}
					on:click={() => openArtist(a.name)}
					aria-label="Open {a.name}"
				>
					<div class="atop">
						<span class="amono" style="background:{artistTint(a.name)};">{a.name.slice(0, 2).toUpperCase()}</span>
						<div style="min-width:0;flex:1;">
							<div class="aname">{a.name}</div>
							{#if a.genre}<div class="agenre">{a.genre}</div>{/if}
						</div>
					</div>
					<div class="abar">
						<div style="width:{Math.min(100, (a.plays / (sortedArtists[0]?.plays || 1)) * 100)}%;background:{artistTint(a.name, true)};"></div>
					</div>
					<div class="ameta">
						<span>{a.plays} play{a.plays === 1 ? '' : 's'}{#if a.mine} · {a.mine} yours{/if}</span>
						<span class="awho">
							{#each (a.who ?? '').split(',').filter(Boolean).slice(0, 3) as w}
								<span class="wchip" title={w}>{w.slice(0, 2).toUpperCase()}</span>
							{/each}
						</span>
					</div>
				</button>
			{/each}
		</div>
	{:else}
		<div class="panel empty">
			<div class="esub">
				Nothing yet. Artists appear here once a deck you're patched into plays something
				through a chain it shares.
			</div>
		</div>
	{/if}
{:else if tab === 'discover'}
	<h2 class="h-display sech">Decks on this instance</h2>
	<p class="sub">Picked on overlap, shown as overlap — the artists, not a score.</p>
	<div class="dgrid">
		{#each discoverDecks as d (d.user_id)}
			<div class="dcard">
				<div class="dtop">
					<span class="avatar">{initials(d)}</span>
					<div style="min-width:0;">
						<div class="pname">{d.display_name || d.username}</div>
						<div class="phandle">@{d.username}</div>
					</div>
				</div>
				{#if d.bio}<p class="dbio">{d.bio}</p>{/if}
				{#if d.overlap}
					<div class="chips">
						{#each d.overlap.split('\u001f') as a}<span class="chip">{a}</span>{/each}
					</div>
					<div class="dnote">You both play these.</div>
				{:else}
					<div class="dnote">
						{d.shared_chains === 0
							? 'They publish no chains yet.'
							: 'Nothing in common from what they share.'}
					</div>
				{/if}
				<div class="dacts">
					{#if d.state === 'accepted'}
						<a class="btn btn-secondary" href="/users/{d.username}">Open deck</a>
					{:else if d.state === 'pending'}
						<span class="statepill">Waiting</span>
					{:else}
						<button class="btn btn-primary" disabled={busy === d.username}
							on:click={() => patch(d.username)}>Patch in</button>
					{/if}
					<span class="dchains">
						{d.shared_chains} shared {d.shared_chains === 1 ? 'chain' : 'chains'}
					</span>
				</div>
			</div>
		{:else}
			<div class="panel empty">
				<div class="esub">Nobody else has an account on this instance yet.</div>
			</div>
		{/each}
	</div>

	<div class="remote">
		<h3 class="h-display" style="font-size:18px;margin:0 0 4px;">A deck on another instance</h3>
		<p class="esub" style="max-width:60ch;">
			Not yet. Your deck already answers to other servers — it has an actor, a WebFinger
			record and an inbox that checks signatures — but nothing is <em>sent</em> outward, so
			patching into a stranger's deck has nowhere to deliver the request. That's the last
			piece of this phase.
		</p>
	</div>
{:else if tab === 'instance'}
	{#if pulse}
		<div class="ihero">
			<div style="flex:1;min-width:220px;">
				<h2 class="h-display" style="font-size:22px;margin:0 0 3px;">This instance</h2>
				<div class="phandle">
					{pulse.decks} {pulse.decks === 1 ? 'deck' : 'decks'} · {pulse.publishing} publishing
				</div>
			</div>
			<div class="seg small">
				{#each [7, 30, 90] as d}
					<button class="seg-btn" class:on={pulseDays === d}
						on:click={() => { pulseDays = d; instanceLoaded = false; loadInstance(); }}>{d}d</button>
				{/each}
			</div>
		</div>

		<div class="tiles">
			{#each [['Decks', String(pulse.decks), `${pulse.publishing} with shared chains`], [`Listens · ${pulseDays}d`, pulse.listens.toLocaleString(), 'across shared chains'], ['Collective hours', `${pulse.hours.toFixed(0)}h`, `last ${pulseDays} days`], ['Artists heard', pulse.artists_heard.toLocaleString(), `${pulse.shared_chains} shared chains`]] as [label, value, sub]}
				<div class="tile">
					<div class="tlabel">{label}</div>
					<div class="tvalue">{value}</div>
					<div class="tsub">{sub}</div>
				</div>
			{/each}
		</div>

		<div class="icols">
			<div class="panel icard">
				<h3 class="h-display" style="font-size:18px;margin:0 0 4px;">What it played</h3>
				<p class="esub" style="margin-bottom:14px;">Shared chains only, across all decks.</p>
				{#each pulse.artists as a}
					<div class="irow">
						<div class="itop"><span>{a.name}</span><span class="iplays">{a.plays}</span></div>
						<div class="abar">
							<div style="width:{Math.min(100, (a.plays / (pulse.artists[0]?.plays || 1)) * 100)}%"></div>
						</div>
					</div>
				{:else}
					<div class="esub">Nothing has played through a shared chain in this window.</div>
				{/each}
			</div>
			<div class="panel icard">
				<!--
					`pulse.daily` has been computed, sent and drawn nowhere since
					v0.80.0 — the third thing in this feature that shipped as live
					data with no reader.

					Bucketed **here** rather than server-side, because the day
					boundary belongs to the reader's timezone and the server does
					not know it: the payload is raw (timestamp, hours) pairs for
					exactly that reason.
				-->
				<h3 class="h-display" style="font-size:18px;margin:0 0 4px;">Collective hours</h3>
				<p class="esub" style="margin-bottom:14px;">Last {pulseDays} days, all decks.</p>
				{#if dayBars.length}
					<div class="hbars">
						{#each dayBars as b (b.key)}
							<div class="hbcol">
								<span class="hbval">{b.hours >= 1 ? `${Math.round(b.hours)}h` : ''}</span>
								<!--
									The bar is measured against a track of its own rather than
									against the column. As a direct flex child of a fixed-height
									column that also holds two labels, `flex-shrink` clamped every
									bar over ~76% of the peak to the same height — so 9h, 10h and
									11h all drew at exactly 112px and the chart went flat at the
									top, worst of all for the peak that defines the scale.
								-->
								<div class="hbtrack">
									<div
										class="hbar"
										class:top={b.hours === peakHours && peakHours > 0}
										style="height:{peakHours > 0 ? Math.max(4, (b.hours / peakHours) * 100) : 4}%"
										title="{b.label}: {b.hours.toFixed(1)}h"
									></div>
								</div>
								<span class="hblabel">{b.label}</span>
							</div>
						{/each}
					</div>
				{:else}
					<div class="esub">Nothing with a known length has played in this window.</div>
				{/if}

				<h3 class="h-display" style="font-size:18px;margin:22px 0 4px;">Most-shared chains</h3>
				<p class="esub" style="margin-bottom:14px;">By listens, not by hours.</p>
				<div class="chips">
					{#each pulse.chains as c}<span class="chip neutral">{c.name} · {c.plays}</span>{:else}
						<span class="esub">No shared chains yet.</span>
					{/each}
				</div>
			</div>
		</div>

		{#if isAdmin && ops}
			<div class="opshead">
				<h2 class="h-display sech" style="margin:0;">Operations</h2>
				<span class="statepill">Admin only</span>
				<span class="rule"></span>
			</div>
			<div class="tiles small">
				{#each [['Identities', ops.identities], ['Published', ops.published], ['Frozen', ops.frozen], ['Remote actors', ops.remote_actors], ['Inbox activities', ops.inbox_activities], ['Deliveries', ops.deliveries]] as [label, v]}
					<div class="tile">
						<div class="tlabel">{label}</div>
						<div class="tvalue small">{v}</div>
					</div>
				{/each}
			</div>
			{#if vocab}
				<div class="panel icard">
					<h3 class="h-display" style="font-size:18px;margin:0 0 4px;">Listen vocabulary</h3>
					<p class="esub" style="max-width:62ch;">
						Frozen, and read-only on purpose. A listen goes out as
						<code>{vocab.verb}</code> wrapping <code>{vocab.object}</code>, so a server
						that understands nothing renders the sentence and one that understands
						<code>td:</code> also gets the signal chain and the format. Three
						vocabularies would mean three things to maintain for a decision made once.
					</p>
					<div class="urirow"><span class="urilabel">Namespace</span><code>{vocab.namespace}</code></div>
					<p class="esub" style="margin-top:12px;max-width:62ch;">
						Nothing is delivered outbound yet, so there are no peers and no delivery
						failures. Signature refusals are written to the log rather than counted here
						— an invented number would be worse than an honest absence.
					</p>
				</div>
			{/if}
		{/if}
	{:else}
		<div class="panel note">Loading…</div>
	{/if}
{:else if tab === 'decks'}
	<!--
		What your deck publishes. **One state, two places**: this and the drawer
		on Signal Chains both call `PUT /api/v1/patch/chains/:id/shared`, so
		flipping it in either is the same flip.

		The design draws a knob switch; this uses the `Private | Shared`
		segmented pair the chains page settled on, because a switch labelled with
		its own state is the ambiguity that pair was built to remove — and having
		the two places disagree about how the same decision *looks* is its own
		kind of confusing.
	-->
	<div class="panel publish">
		<div class="pubhead">
			<div>
				<h2 class="h-display sech" style="margin:0 0 4px;">What your deck publishes</h2>
				<p class="esub" style="margin:0;max-width:56ch;">
					Per chain, off by default. The same switch sits in each chain's drawer on
					<a href="/chains">Signal Chains</a> — one state, two places.
				</p>
			</div>
			<span class="tag" class:tag-on={mySharedChains > 0}>
				{mySharedChains} of {myChains.length} shared
			</span>
		</div>

		{#if myChains.length}
			<div class="pubgrid">
				{#each myChains as c (c.id)}
					<div class="pubrow">
						<div class="ldots" title={(c.components ?? []).map((x) => x.name).join(' → ')}>
							{#each (c.components ?? []).slice(0, 4) as st}
								<span class="dot" style="background:{roleColour(st.type)};"></span>
							{/each}
						</div>
						<div style="flex:1;min-width:0;">
							<div class="pubname">{c.name}</div>
							<div class="pubsub">
								{c.total_hours >= 1
									? `${c.total_hours.toFixed(1)}h logged`
									: c.total_hours > 0
										? `${Math.round(c.total_hours * 60)}min logged`
										: 'nothing logged yet'}
							</div>
						</div>
						<div class="seg pubseg" role="group" aria-label="Who hears {c.name}">
							<button
								class="seg-btn"
								class:on={!c.shared}
								aria-pressed={!c.shared}
								disabled={sharingChain === c.id}
								on:click={() => setShared(c, false)}
							>Private</button>
							<button
								class="seg-btn"
								class:on={c.shared}
								aria-pressed={c.shared}
								disabled={sharingChain === c.id}
								on:click={() => setShared(c, true)}
							>Shared</button>
						</div>
					</div>
				{/each}
			</div>
		{:else}
			<div class="esub">
				You have no chains yet. <a href="/chains">Build one</a> — a chain is what makes a
				listen here a Tapedeck listen rather than a sentence.
			</div>
		{/if}
	</div>

	<div class="grid">
		<div>
			<div class="secbar">
				<h2 class="h-display sech">You're patched into</h2>
				<span class="seccount">{accepted.length}</span>
			</div>
			<div class="panel rows">
				{#each following as p (p.user_id)}
					<div class="prow">
						<span class="avatar">{initials(p)}</span>
						<div class="pmain">
							<div class="pname">{p.display_name || p.username}</div>
							<div class="phandle">@{p.username}</div>
							<div class="pmeta">
								{#if p.state === 'pending'}
									Waiting on them
								{:else if p.shared_chains === 0}
									Shares no chains yet — nothing will appear
								{:else}
									{p.shared_chains} shared {p.shared_chains === 1 ? 'chain' : 'chains'}
								{/if}
							</div>
						</div>
						<button class="btn btn-secondary" disabled={busy === p.username}
							on:click={() => withdraw(p.username)}>
							{p.state === 'pending' ? 'Cancel' : 'Unpatch'}
						</button>
					</div>
				{:else}
					<div class="rowempty">You haven't patched into anyone yet.</div>
				{/each}
			</div>
		</div>

		<div>
			<div class="secbar">
				<h2 class="h-display sech">Decks on this instance</h2>
				<span class="seccount">{decks.length}</span>
			</div>
			<div class="panel rows">
				{#each decks as p (p.user_id)}
					{@const mine = following.find((f) => f.user_id === p.user_id)}
					<div class="prow">
						<span class="avatar">{initials(p)}</span>
						<div class="pmain">
							<div class="pname">{p.display_name || p.username}</div>
							<div class="phandle">@{p.username}</div>
							<div class="pmeta">
								{p.shared_chains} shared {p.shared_chains === 1 ? 'chain' : 'chains'}
							</div>
						</div>
						{#if mine?.state === 'accepted'}
							<span class="statepill on">Patched</span>
						{:else if mine?.state === 'pending'}
							<span class="statepill">Waiting</span>
						{:else}
							<button class="btn btn-primary" disabled={busy === p.username}
								on:click={() => patch(p.username)}>Patch in</button>
						{/if}
					</div>
				{:else}
					<div class="rowempty">No other decks on this instance yet.</div>
				{/each}
			</div>
			<p class="footnote">
				Your deck is private, so a patch is a request either way — theirs waits in
				<button class="linkish" on:click={() => go('inbox')}>your inbox</button>,
				and yours waits in theirs.
			</p>
		</div>
	</div>
{:else}
	<!--
		Sittings somebody passed on. **A fourth kind of thing in this inbox, and
		the only one with nothing to decide** — it writes nothing to a history,
		so "Open" and "Dismiss" is the whole of it. A capture below needs
		accepting because accepting really does put a play into your listening;
		this does not, and the two must not be made to look alike.
	-->
	{#if unseenSittings.length}
		<div class="secbar">
			<h2 class="h-display sech">Sittings passed to you</h2>
			<span class="seccount">{unseenSittings.length}</span>
		</div>
		<p class="sub" style="margin-top:0;">
			An evening somebody wants you to hear about. It is a record of what they played —
			<strong>none of it enters your history</strong>, so there is nothing to accept.
		</p>
		<div class="panel rows">
			{#each unseenSittings as h (h.uuid)}
				<div class="prow">
					<span class="avatar small">{h.from_username.slice(0, 2).toUpperCase()}</span>
					<div class="pmain">
						<div class="pname">{h.title || 'A sitting'}</div>
						<div class="phandle">
							from @{h.from_username}{' '}{#if h.started_at}· {new Date(h.started_at * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'long' })}{/if}
						</div>
						{#if h.note}<div class="dbecause">“{h.note}”</div>{/if}
					</div>
					<div class="pacts">
						<button class="btn btn-primary" on:click={() => openSitting(h)}>Open</button>
						<button class="btn btn-secondary" disabled={busy === h.uuid} on:click={() => dismissSitting(h)}>
							Dismiss
						</button>
					</div>
				</div>
			{/each}
		</div>
	{/if}

	<!-- What you passed on. Not part of the inbox count — none of it is
	     waiting on you — but it belongs on the same tab, because "an evening
	     somebody sent me" and "an evening I sent" are the same object seen from
	     either end. -->
	{#if sentSittings.length}
		<div class="secbar" style="margin-top:26px;">
			<h2 class="h-display sech">Sittings you passed on</h2>
			<span class="seccount">{sentSittings.length}</span>
		</div>
		<p class="sub" style="margin-top:0;">
			Each one is rendered from your listens every time it is opened, so this really does
			take it back — and un-sharing a chain really does remove those plays from it.
		</p>
		<div class="panel rows">
			{#each sentSittings as h (h.uuid)}
				<div class="prow">
					<span class="avatar small">{h.to_username.slice(0, 2).toUpperCase()}</span>
					<div class="pmain">
						<div class="pname">{h.title || 'A sitting'}</div>
						<div class="phandle">
							to @{h.to_username}{' '}{#if h.started_at}· {new Date(h.started_at * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'long' })}{/if}
							<!-- Whether they have read it, because taking back something
							     already read is a different act from taking back something
							     nobody saw — and only one of them is really undone. -->
							· {h.seen_at ? 'read' : 'not opened yet'}
						</div>
						{#if h.note}<div class="dbecause">“{h.note}”</div>{/if}
					</div>
					<div class="pacts">
						<button class="btn btn-secondary" disabled={busy === h.uuid}
							on:click={() => viewSentSitting(h)}>
							View
						</button>
						<button class="btn btn-danger" disabled={busy === h.uuid}
							on:click={() => withdrawSitting(h)}>
							{busy === h.uuid ? '…' : 'Withdraw'}
						</button>
					</div>
				</div>
			{/each}
		</div>
	{/if}

	{#if pendingCaptures.length}
		<div class="secbar" style="margin-top:26px;">
			<h2 class="h-display sech">Spool handoffs</h2>
			<span class="seccount">{pendingCaptures.length}</span>
		</div>
		<p class="sub" style="margin-top:0;">
			Plays captured on someone else's deck while you were in a Shared Spool.
			<strong>Nothing enters your history until you accept it</strong> —
			{#if forwardSpool}
				and what you accept is forwarded on to Last.fm and ListenBrainz like your own
				listens, unless it is more than {forwardWindowDays} days old.
				<a href="/settings#connections">Change that</a>.
			{:else}
				and what you accept is stored only here, never forwarded on to Last.fm or
				ListenBrainz. <a href="/settings#connections">Change that</a>.
			{/if}
		</p>
		<!--
			**One card per sitting**, per the design. A flat list loses the thing
			that makes a capture make sense — which evening it came from and
			whose deck played it — and it forces a decision per row where the
			real decision is usually "that evening, yes".

			Ticking rather than Keep/Skip per row for the same reason: the
			endpoint has always taken a set of ids, and the common case is
			several at once.
		-->
		{#each captureSittings as sit (sit.uuid)}
			<div class="panel sitcard">
				<div class="sithead">
					<span class="avatar small">{sit.from.slice(0, 2).toUpperCase()}</span>
					<div style="flex:1;min-width:200px;">
						<div class="sittitle">{sit.title}</div>
						<div class="sitwhen">{sit.when}</div>
					</div>
					<span class="tag">{sit.rows.length} pending</span>
				</div>

				<div class="sittracks">
					{#each sit.rows as c (c.id)}
						<label class="sittrack" class:on={picked.has(c.id)}>
							<input
								type="checkbox"
								checked={picked.has(c.id)}
								on:change={() => togglePick(c.id)}
								aria-label="Keep {c.title}"
							/>
							<div style="flex:1;min-width:0;">
								<div class="pname">{c.title}</div>
								<div class="pmeta">{c.artist}{#if c.album} · {c.album}{/if}</div>
							</div>
							<span class="sitat">{new Date(c.played_at * 1000).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })}</span>
						</label>
					{/each}
				</div>

				<div class="bulk">
					<button
						class="btn btn-primary"
						disabled={busy === 'captures' || !pickedIn(sit).length}
						on:click={() => settleCaptures(pickedIn(sit), true)}
					>
						Accept {pickedIn(sit).length} play{pickedIn(sit).length === 1 ? '' : 's'}
					</button>
					<button class="btn btn-secondary" disabled={busy === 'captures'}
						on:click={() => settleCaptures(sit.rows.map((c) => c.id), false)}>
						Decline the rest
					</button>
					<span class="esub" style="margin:0;">
						Accepted plays land in your history credited to {sit.from}'s deck — and are
						never forwarded on.
					</span>
				</div>
			</div>
		{/each}
	{/if}

	{#if openDubs.length}
		<div class="secbar" style="margin-top:26px;">
			<h2 class="h-display sech">Dubs for you</h2>
			<span class="seccount">{openDubs.length}</span>
		</div>
		<p class="sub" style="margin-top:0;">
			A dub is a record passed hand to hand. They land in
			<a href="/crate">the Crate</a>, with everything else you're still flipping through.
		</p>
		<div class="panel rows">
			{#each openDubs as d (d.id)}
				<div class="prow">
					<span class="avatar small">{d.from_username.slice(0, 2).toUpperCase()}</span>
					<div class="pmain">
						<div class="pname">{d.name || d.artist}</div>
						<div class="pmeta">{d.name ? d.artist : d.kind}</div>
						{#if d.because}<div class="dbecause">“{d.because}”</div>{/if}
						<div class="phandle">from @{d.from_username}</div>
					</div>
					<a class="btn btn-secondary" href="/crate">Open the Crate</a>
				</div>
			{/each}
		</div>
	{/if}

	<div class="secbar" class:spaced={pendingCaptures.length || openDubs.length}>
		<h2 class="h-display sech">Patch requests</h2>
		<span class="seccount">{pending.length}</span>
	</div>
	<p class="sub" style="margin-top:0;">
		Your deck is private, so every patch needs a yes. Approving shares only the chains you've
		switched to shared — and nothing else.
	</p>
	<div class="panel rows">
		{#each pending as p (p.user_id)}
			<div class="prow">
				<span class="avatar">{initials(p)}</span>
				<div class="pmain">
					<div class="pname">{p.display_name || p.username}</div>
					<div class="phandle">@{p.username}</div>
					{#if p.bio}<div class="pmeta">{p.bio}</div>{/if}
				</div>
				<div class="pacts">
					<button class="btn btn-primary" disabled={busy === p.username}
						on:click={() => settle(p, true)}>Approve</button>
					<!-- Secondary, not ghost: `btn-ghost` renders in the accent, which
					     put two equally-loud actions side by side where one of them is
					     "no". Not `btn-danger` either — declining destroys nothing and
					     the person can ask again; it is the lesser action, not the
					     dangerous one. -->
					<button class="btn btn-secondary" disabled={busy === p.username}
						on:click={() => settle(p, false)}>Decline</button>
				</div>
			</div>
		{:else}
			<div class="rowempty">Nothing waiting on you.</div>
		{/each}
	</div>

	<div class="secbar" style="margin-top:26px;">
		<h2 class="h-display sech">Patched into you</h2>
		<span class="seccount">{followers.filter((f) => f.state === 'accepted').length}</span>
	</div>
	<div class="panel rows">
		{#each followers.filter((f) => f.state === 'accepted') as p (p.user_id)}
			<div class="prow">
				<span class="avatar">{initials(p)}</span>
				<div class="pmain">
					<div class="pname">{p.display_name || p.username}</div>
					<div class="phandle">@{p.username}</div>
				</div>
				<span class="statepill on">Approved</span>
			</div>
		{:else}
			<div class="rowempty">Nobody is patched into your deck.</div>
		{/each}
	</div>
	<p class="footnote">
		They see listens from the chains you share, and nothing else — no history, no counts, no
		notes. <a href="/chains">Choose what your deck publishes</a>.
	</p>
{/if}

<!--
	The handed-over sitting itself. A document: what they played, in order.
	Nothing here is a listen of the reader's, which is why it draws rows by hand
	rather than reusing `ScrobbleRow` — that component carries a love heart and a
	row menu, both of which act on *your own* history and would be acting on
	somebody else's listening here.
-->
{#if openedSitting}
	<div
		class="dialog-backdrop"
		on:click={() => (openedSitting = null)}
		on:keydown={() => {}}
		role="presentation"
	>
		<div
			class="dialog sitting-doc"
			on:click|stopPropagation
			on:keydown|stopPropagation
			role="presentation"
		>
			<div class="dialog-title">{openedSitting.title || 'A sitting'}</div>
			<div class="dialog-body" style="min-width:0;">
				<p class="shead">
					{openedSitting.from_display_name || openedSitting.from_username}
					{#if openedSitting.started_at}
						· {new Date(openedSitting.started_at * 1000).toLocaleString(undefined, {
							weekday: 'long',
							day: 'numeric',
							month: 'long',
							hour: '2-digit',
							minute: '2-digit'
						})}
					{/if}
				</p>
				{#if openedSitting.note}
					<div class="dbecause" style="margin-bottom:14px;">“{openedSitting.note}”</div>
				{/if}

				{#if openedSitting.gone}
					<p class="sub" style="margin:0;">
						They have since regrouped their sessions, so this sitting no longer exists as
						one. Nothing was lost on their side — the listens are still theirs.
					</p>
				{:else if !openedSitting.listens.length}
					<p class="sub" style="margin:0;">
						Nothing in this sitting ran through a chain they share, so there is nothing to
						show. Sharing is per chain and off by default.
					</p>
				{:else}
					<div class="slist">
						{#each openedSitting.listens as l, i (i)}
							<div class="slrow">
								<CoverArt
									url={l.artwork_url}
									caaId={l.caa_id}
									releaseMbid={l.caa_release_mbid}
									seed={l.artist}
								/>
								<div style="flex:1;min-width:0;">
									<div class="sltitle">{l.title}</div>
									<div class="slsub">
										{l.artist}{l.album ? ` — ${l.album}` : ''}
										{#if l.chain_name}<span class="slchain">{l.chain_name}</span>{/if}
									</div>
								</div>
								<span class="sltime">
									{new Date(l.timestamp * 1000).toLocaleTimeString(undefined, {
										hour: '2-digit',
										minute: '2-digit'
									})}
								</span>
							</div>
						{/each}
					</div>
				{/if}

				<!--
					Stated, never a silent difference. A document with invisible
					holes misrepresents the evening and the reader could not tell.
				-->
				{#if openedSitting.omitted > 0}
					<p class="somitted">
						{openedSitting.omitted}
						{openedSitting.omitted === 1 ? 'listen is' : 'listens are'} not shown — they ran
						through chains they don't share.
					</p>
				{/if}
			</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (openedSitting = null)}>Close</button>
			</div>
		</div>
	</div>
{/if}

<!--
	Start a Shared Spool.

	Two fields the design draws are deliberately absent rather than drawn and
	ignored. Its **chain picker** would be a second way to decide what a sitting
	publishes, and there is exactly one — the per-chain shared switch — which
	this must not quietly acquire an override for. Its **length** ("1 hour",
	"until I end it") has nothing behind it: `spools` has no `ends_at`, so the
	control would set a limit nothing enforces. Both are cheap to add once
	they mean something; a control that does nothing is not.
-->
{#if showSpoolDialog}
	<div
		class="dialog-backdrop"
		on:click={() => (showSpoolDialog = false)}
		on:keydown={() => {}}
		role="presentation"
	>
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">Start a Shared Spool</div>
			<div class="dialog-body">
				<p style="margin:0 0 15px;text-wrap:pretty;">
					Every deck keeps recording its own plays. What yours captures lands in the others'
					inboxes to confirm — nothing writes straight into a history.
				</p>

				<!--
					The moment to say this is here, not after an empty inbox: a
					host who shares no chain captures nothing at all.
				-->
				{#if !mySharedChains}
					<div class="dwarn">
						You share no chains, so nothing you play would be passed on.
						<a href="/chains">Share one first</a>.
					</div>
				{/if}

				<div class="field" style="margin-bottom:14px;">
					<label for="sp-title">Call it something <span style="opacity:.6">(optional)</span></label>
					<input
						id="sp-title"
						class="input"
						bind:value={spoolTitle}
						placeholder="Sunday listening"
						maxlength="80"
					/>
				</div>

				<div class="field">
					<label for="sp-with">Spool with</label>
					<div class="picklist" id="sp-with">
						{#each spoolCandidates as p (p.username)}
							<label class="pick" class:on={spoolPicked[p.username]}>
								<input type="checkbox" bind:checked={spoolPicked[p.username]} />
								<span class="avatar small">{p.username.slice(0, 2).toUpperCase()}</span>
								<span class="pickname">
									{p.display_name || p.username}
									<span class="pickhandle">@{p.username}</span>
								</span>
							</label>
						{/each}
					</div>
					<div class="pickfoot">
						{spoolPickedCount > 1
							? `${spoolPickedCount} decks — every side confirms its own captures`
							: 'Pick one deck, or several for a group sitting'}
					</div>
				</div>
			</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (showSpoolDialog = false)}>Cancel</button>
				<button
					class="btn btn-primary"
					disabled={busy === 'spool' || spoolPickedCount === 0}
					on:click={startSpool}
				>
					{busy === 'spool' ? 'Sending…' : 'Send invitation'}
				</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.head {
		margin-bottom: 14px;
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 16px;
		flex-wrap: wrap;
	}
	.start-spool { display: inline-flex; align-items: center; gap: 8px; white-space: nowrap; }
	.headright { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; justify-content: flex-end; }
	.whoami { display: flex; align-items: center; gap: 9px; flex-wrap: wrap; }
	.myhandle {
		font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
		font-size: 12.5px;
		color: var(--td-muted-2);
	}
	.deckpill { text-decoration: none; white-space: nowrap; }
	.deckpill.open {
		background: color-mix(in srgb, var(--td-accent) 12%, transparent);
		border-color: color-mix(in srgb, var(--td-accent) 38%, transparent);
		color: var(--td-accent);
	}
	h1 { font-size: 30px; margin: 0 0 4px; }
	.sub { margin: 0 0 16px; font-size: 13.5px; color: var(--td-muted); max-width: 64ch; }

	/* `.seg-btn` is defined per page, not globally — Svelte scopes styles, so
	   matching the existing definition matters more than deduplicating it. */
	.seg {
		display: inline-flex;
		background: var(--td-sidebar);
		border-radius: 999px;
		padding: 4px;
		gap: 2px;
		margin-bottom: 20px;
	}
	.seg-btn {
		border: none;
		background: transparent;
		color: var(--td-nav-fg);
		font: 600 13px/1 'Figtree', system-ui, sans-serif;
		padding: 9px 18px;
		border-radius: 999px;
		cursor: pointer;
		display: flex;
		align-items: center;
		gap: 7px;
	}
	.seg-btn.on { background: var(--td-accent); color: #fff; }
	.count {
		background: var(--td-accent-hi);
		color: #fff;
		font-size: 11px;
		border-radius: 999px;
		padding: 1px 7px;
		font-variant-numeric: tabular-nums;
	}
	.seg-btn.on .count { background: rgba(255, 255, 255, 0.28); }

	.note { border-radius: 22px; padding: 20px; font-size: 13.5px; color: var(--td-muted); }
	.empty { border-radius: 22px; padding: 34px; text-align: center; }
	/* The centring belongs to the empty state, not to the class. `.esub` is
	   reused in left-aligned panels, and carrying `margin: 0 auto` into them
	   silently centred a paragraph nobody asked to be centred — visible only by
	   reading the rendered page. */
	.esub { font-size: 13.5px; color: var(--td-muted); max-width: 54ch; margin: 0; line-height: 1.5; }
	.empty .esub { margin-inline: auto; }

	.feed { display: flex; flex-direction: column; gap: 22px; }
	.daybar { display: flex; align-items: center; gap: 12px; margin-bottom: 10px; }
	.daylabel {
		font-size: 11px;
		letter-spacing: 0.14em;
		text-transform: uppercase;
		color: var(--td-muted-2);
	}
	.rule { flex: 1; height: 1px; background: var(--td-border); }
	.daycount { font-size: 12px; color: var(--td-muted-3); font-variant-numeric: tabular-nums; }

	.listrows {
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 22px;
		overflow: hidden;
	}
	.lrow {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 10px 16px;
		border-bottom: 1px solid var(--td-border);
	}
	.lrow:last-child { border-bottom: none; }
	.lmain { flex: 1; min-width: 0; }
	.ltop { display: flex; align-items: baseline; gap: 8px; min-width: 0; }
	.ltitle {
		font-size: 14px;
		font-weight: 600;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.lartist { font-size: 12.5px; color: var(--td-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.lsub { display: flex; align-items: center; gap: 10px; margin-top: 2px; min-width: 0; }
	.lwho { font-size: 12.5px; color: var(--td-muted-2); white-space: nowrap; }
	.lchain {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		font-size: 12px;
		color: var(--td-muted-3);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.ldots { display: flex; gap: 3.5px; flex: none; }
	.dot { width: 7px; height: 7px; border-radius: 999px; display: block; }
	.lago {
		font-size: 12px;
		color: var(--td-muted-3);
		width: 38px;
		text-align: right;
		flex: none;
		font-variant-numeric: tabular-nums;
	}
	.more { display: flex; justify-content: center; margin-top: 18px; }

	.grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(330px, 1fr)); gap: 20px; }
	.secbar { display: flex; align-items: baseline; gap: 10px; margin-bottom: 12px; }
	.sech { font-size: 20px; margin: 0; }
	.seccount { font-size: 12.5px; color: var(--td-muted-2); font-variant-numeric: tabular-nums; }

	.rows { border-radius: 22px; overflow: hidden; padding: 0; }
	.prow {
		display: flex;
		align-items: center;
		gap: 13px;
		padding: 14px 18px;
		border-bottom: 1px solid var(--td-border);
		flex-wrap: wrap;
	}
	.prow:last-child { border-bottom: none; }
	.pmain { flex: 1; min-width: 180px; }
	.pname { font-size: 14px; font-weight: 600; }
	.phandle {
		font: 500 11.5px ui-monospace, SFMono-Regular, Menlo, monospace;
		color: var(--td-muted-3);
		margin-top: 1px;
	}
	.pmeta { font-size: 12.5px; color: var(--td-muted-2); margin-top: 3px; }
	.pacts { display: flex; gap: 8px; flex-wrap: wrap; }
	.rowempty { padding: 26px; text-align: center; font-size: 13.5px; color: var(--td-muted-2); }

	.avatar {
		width: 38px;
		height: 38px;
		border-radius: 999px;
		background: var(--td-surface2);
		color: var(--td-muted);
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: 12.5px;
		font-weight: 700;
		flex: none;
	}
	.statepill {
		font-size: 12px;
		font-weight: 600;
		padding: 5px 12px;
		border-radius: 999px;
		background: var(--td-surface2);
		color: var(--td-muted-2);
		white-space: nowrap;
	}
	.statepill.on { background: var(--td-good-bg); color: var(--td-good-fg); }

	.footnote {
		margin: 18px 0 0;
		font-size: 12.5px;
		color: var(--td-muted-2);
		max-width: 62ch;
		line-height: 1.55;
	}
	/* A control that sits *inside* a sentence. A `.btn` here carries button
	   padding, which pushes the following comma away from the word it belongs
	   to — visible only by reading the rendered line, not the markup. */
	.secbar.spaced { margin-top: 26px; }
	.bulk { display: flex; gap: 10px; flex-wrap: wrap; margin-top: 12px; }
	.dbecause {
		font-size: 13px;
		color: var(--td-muted);
		font-style: italic;
		margin-top: 6px;
		max-width: 52ch;
	}
	.avatar.small { width: 32px; height: 32px; font-size: 11px; }

	/* The sitting. `--td-panel` is the *dark card* in both themes, so every
	   colour inside it is restated — the v0.70.8 rule: if you use --td-panel,
	   set the text colours too. */
	.spool {
		display: flex;
		align-items: center;
		gap: 18px;
		flex-wrap: wrap;
		background: var(--td-panel);
		color: #f4e7cf;
		border-radius: 22px;
		padding: 18px 20px;
		margin-bottom: 20px;
	}
	.spool .slabel {
		font-size: 10.5px;
		letter-spacing: 0.14em;
		text-transform: uppercase;
		color: #c9a678;
		margin-bottom: 4px;
	}
	.spool .stitle { font-size: 15.5px; font-weight: 600; color: #f4e7cf; }
	.spool .smembers { font-size: 12.5px; color: #a4967c; margin-top: 2px; }
	.spool .swarn { font-size: 12.5px; color: #e0b48c; margin-top: 7px; max-width: 56ch; }
	.spool .swarn a { color: #f0c8a4; text-decoration: underline; }
	.smain { flex: 1; min-width: 200px; }
	.sacts { display: flex; gap: 9px; flex-wrap: wrap; }
	.spulse { display: flex; gap: 4px; flex: none; }
	.spulse span {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: var(--td-accent-hi);
		animation: td-blink 1.4s ease-in-out infinite;
	}
	.spulse span:last-child { background: var(--td-accent-2); animation-delay: 0.4s; }
	.spool.invited .spulse span { animation: none; }
	/* An ended sitting still shows when the invitation was never answered, and
	   it must not read as one that is happening. No pulse, and a dashed edge —
	   the same "this is not live" cue the unowned genre cards use. */
	.spool.over {
		background: none;
		border: 1px dashed var(--td-muted-3);
		color: var(--td-text);
	}
	.spool.over .slabel { color: var(--td-muted); }
	.spool.over .stitle,
	.spool.over .ctime { color: var(--td-text); }
	.spool.over .smembers,
	.spool.over .csub { color: var(--td-muted); }
	/* `.swarn` is a warm cream meant to sit on the dark panel. On the ended
	   card there is no dark panel, so it has to be re-stated against the
	   page — the `--td-panel` trap, which is a text colour left to chance
	   reading correctly in exactly one theme. */
	.spool.over .swarn { color: var(--td-muted-2); }
	.spool.over .swarn a { color: var(--td-accent); }
	.spool.over .sav { background: var(--td-surface2); color: var(--td-text); }
	/* The clock and what the sitting has offered. Both were already in the
	   payload — `started_at` and `captured` — and drawn nowhere. */
	.spool .sclock { text-align: right; flex: none; }
	.spool .ctime {
		font-variant-numeric: tabular-nums;
		font-size: 21px;
		letter-spacing: .06em;
		color: #f4e7cf;
	}
	.spool .csub { font-size: 11.5px; color: #9a8c72; margin-top: 2px; }
	/* A date is not a clock face and should not be set like one. */
	.spool.over .ctime { font-size: 16px; letter-spacing: 0; }
	.note.said { display: flex; align-items: center; gap: 12px; justify-content: space-between; }
	.linkish {
		background: none;
		border: 0;
		padding: 0;
		font: inherit;
		color: var(--td-accent);
		cursor: pointer;
		text-decoration: underline;
		flex: none;
	}

	/* — a handed-over sitting — */
	.sitting-doc { width: min(560px, 100%); min-width: 0; }
	.shead { margin: 0 0 12px; font-size: 13px; color: var(--td-muted); }
	.slist {
		display: flex;
		flex-direction: column;
		max-height: 46vh;
		overflow-y: auto;
		border: 1px solid var(--td-border);
		border-radius: 14px;
	}
	.slrow {
		display: flex;
		align-items: center;
		gap: 11px;
		padding: 9px 11px;
		min-width: 0;
	}
	.slrow + .slrow { border-top: 1px solid var(--td-border); }
	.sltitle {
		font-size: 13.5px;
		font-weight: 600;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.slsub {
		font-size: 12px;
		color: var(--td-muted-2);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.slchain { color: var(--td-accent); margin-left: 7px; }
	.sltime {
		flex: none;
		font-size: 12px;
		color: var(--td-muted-3);
		font-variant-numeric: tabular-nums;
	}
	.somitted {
		margin: 12px 0 0;
		font-size: 12.5px;
		color: var(--td-text);
		background: color-mix(in srgb, var(--td-accent) 10%, transparent);
		border-left: 3px solid var(--td-accent);
		border-radius: 0 12px 12px 0;
		padding: 9px 12px;
	}

	/* — reel controls — */
	.reelctl {
		display: flex;
		gap: 10px;
		flex-wrap: wrap;
		align-items: center;
		margin-bottom: 16px;
	}
	.reelsearch { position: relative; flex: 1; min-width: 240px; max-width: 380px; }
	.reelsearch svg {
		position: absolute;
		left: 15px;
		top: 50%;
		transform: translateY(-50%);
		color: var(--td-muted-3);
		pointer-events: none;
	}
	.reelsearch .input { width: 100%; padding-left: 40px; }
	.deckchips { display: flex; gap: 6px; flex-wrap: wrap; }

	/* — reel row — */
	.ravatar {
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
		text-decoration: none;
	}
	.ravatar img { width: 100%; height: 100%; object-fit: cover; }
	.rhandle {
		font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
		font-size: 11px;
		color: var(--td-muted-3);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.lsub .lwho { text-decoration: none; color: var(--td-muted-2); }
	.lsub .lwho:hover { text-decoration: underline; }
	.rlove {
		border: none;
		background: transparent;
		color: var(--td-muted-3);
		cursor: pointer;
		padding: 5px;
		border-radius: 9px;
		display: flex;
		flex: none;
	}
	.rlove:hover { color: var(--td-accent); }
	.rlove.on { color: #b2622d; }
	@media (max-width: 640px) {
		/* The sleeve goes before the person does: at this width the row is
		   already carrying an avatar, and two circles crowd the title. */
		.lrow :global(.cover) { display: none; }
		.rhandle { display: none; }
	}

	/* — rolling now — */
	.livehead { display: flex; align-items: center; gap: 10px; margin-bottom: 12px; }
	.livedot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--td-accent-hi);
		animation: td-blink 1.6s ease-in-out infinite;
	}
	@keyframes td-blink { 0%, 100% { opacity: 1; } 50% { opacity: .25; } }
	.livegrid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
		gap: 14px;
		margin-bottom: 26px;
	}
	.livecard {
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 22px;
		padding: 16px 18px;
		min-width: 0;
	}
	.livetop { display: flex; align-items: center; gap: 12px; margin-bottom: 12px; min-width: 0; }
	.livename {
		font-size: 13.5px;
		font-weight: 600;
		color: var(--td-text);
		text-decoration: none;
	}
	.livename:hover { text-decoration: underline; }
	.livetitle {
		font-size: 15px;
		font-weight: 600;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.liveartist {
		font-size: 13px;
		color: var(--td-muted);
		margin-bottom: 12px;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.livebar {
		height: 6px;
		border-radius: 999px;
		background: var(--td-track);
		overflow: hidden;
		margin-bottom: 10px;
	}
	.livebar > div {
		height: 100%;
		border-radius: 999px;
		background: linear-gradient(90deg, var(--td-accent-2), var(--td-accent));
	}
	/* An extrapolated position is drawn as one. */
	.livebar.guess > div { opacity: .45; }
	.livefoot { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
	.livetime {
		margin-left: auto;
		font-size: 12px;
		color: var(--td-muted-3);
		font-variant-numeric: tabular-nums;
	}
	.vu { display: flex; align-items: flex-end; gap: 2.5px; height: 18px; flex: none; }
	.vu span {
		width: 3px;
		border-radius: 2px;
		background: var(--td-accent);
		height: 40%;
		animation: td-vu 1.1s ease-in-out infinite;
	}
	@keyframes td-vu {
		0%, 100% { height: 30%; }
		50% { height: 100%; }
	}
	@media (prefers-reduced-motion: reduce) {
		.livedot, .vu span { animation: none; }
		.vu span { height: 70%; }
	}

	/* — what your deck publishes — */
	.publish { border-radius: 24px; padding: 22px; margin-bottom: 26px; }
	.pubhead {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 16px;
		flex-wrap: wrap;
		margin-bottom: 16px;
	}
	.pubhead .tag { flex: none; }
	.pubhead .tag-on {
		background: var(--td-good-bg);
		color: var(--td-good-fg);
		border-color: transparent;
	}
	.pubgrid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
		gap: 8px;
	}
	.pubrow {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 11px 14px;
		border-radius: 16px;
		background: var(--td-surface2);
		min-width: 0;
	}
	.pubname {
		font-size: 13.5px;
		font-weight: 600;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.pubsub { font-size: 11.5px; color: var(--td-muted-3); }
	.pubseg { flex: none; }
	.pubseg .seg-btn { font-size: 12px; padding: 6px 12px; }
	.pubseg .seg-btn:disabled { opacity: .6; cursor: default; }
	@media (max-width: 520px) {
		.pubrow { flex-wrap: wrap; }
		.pubseg { margin-left: auto; }
	}

	/* — spool captures, one card per sitting — */
	.sitcard { border-radius: 22px; padding: 18px 20px; margin-bottom: 14px; }
	.sithead {
		display: flex;
		align-items: center;
		gap: 12px;
		flex-wrap: wrap;
		margin-bottom: 14px;
	}
	.sittitle { font-size: 15px; font-weight: 600; }
	.sitwhen { font-size: 12.5px; color: var(--td-muted-2); margin-top: 2px; }
	.sittracks { display: flex; flex-direction: column; gap: 5px; margin-bottom: 14px; }
	.sittrack {
		display: flex;
		align-items: center;
		gap: 11px;
		padding: 8px 11px;
		border-radius: 13px;
		background: var(--td-surface2);
		cursor: pointer;
		min-width: 0;
	}
	.sittrack.on { background: color-mix(in srgb, var(--td-accent) 11%, transparent); }
	.sittrack input { width: 16px; height: 16px; accent-color: var(--td-accent); flex: none; cursor: pointer; }
	.sitat {
		flex: none;
		font-size: 12px;
		color: var(--td-muted-3);
		font-variant-numeric: tabular-nums;
	}

	/* The avatar stack on the running banner — the people first, which is what
	   a sitting is. */
	.savatars { display: flex; flex: none; }
	.savatars .sav {
		width: 34px;
		height: 34px;
		border-radius: 50%;
		background: var(--td-panel2);
		color: #f4e7cf;
		display: flex;
		align-items: center;
		justify-content: center;
		font: 700 11px ui-monospace, SFMono-Regular, Menlo, monospace;
		border: 2px solid var(--td-panel);
	}
	.savatars .sav + .sav { margin-left: -11px; }

	/* — collective hours — */
	.hbars {
		display: flex;
		align-items: flex-end;
		gap: 5px;
		height: 148px;
		margin-bottom: 4px;
	}
	.hbcol {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: flex-end;
		height: 100%;
		gap: 5px;
	}
	.hbval {
		font-size: 11px; color: var(--td-muted-2);
		font-variant-numeric: tabular-nums; flex: none;
	}
	/* The bar's scale. Its height is whatever the column has left once the two
	   labels have taken theirs, so a percentage on the bar inside it means what
	   it says — which it did not while the bar was the flex item being shrunk. */
	.hbtrack { flex: 1 1 auto; min-height: 0; width: 100%; position: relative; }
	.hbar {
		position: absolute;
		left: 0; right: 0; bottom: 0;
		border-radius: 7px 7px 3px 3px;
		background: color-mix(in srgb, var(--td-accent) 42%, transparent);
		min-height: 4px;
	}
	/* The busiest day, so the shape has somewhere to land. */
	.hbar.top { background: var(--td-accent); }
	.hblabel {
		flex: none;
		font-size: 11px;
		color: var(--td-muted-3);
		white-space: nowrap;
		overflow: hidden;
	}

	/* — the start dialog — */
	/* Deliberately *not* `--td-bad-*`. That pair is the destructive red, and
	   this is a consequence worth knowing rather than a dangerous act — using
	   it here would blunt the one signal that means "careful". Body text stays
	   `--td-text` and the accent marks the edge, so it reads in both themes by
	   construction rather than by a colour pairing invented for one. */
	.dwarn {
		font-size: 12.5px;
		color: var(--td-text);
		background: color-mix(in srgb, var(--td-accent) 10%, transparent);
		border-left: 3px solid var(--td-accent);
		border-radius: 0 12px 12px 0;
		padding: 9px 12px;
		margin-bottom: 14px;
	}
	.dwarn a { color: var(--td-accent); text-decoration: underline; }
	.picklist {
		display: flex;
		flex-direction: column;
		gap: 4px;
		max-height: 232px;
		overflow-y: auto;
		border: 1px solid var(--td-border);
		border-radius: 14px;
		padding: 6px;
	}
	.pick {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 7px 9px;
		border-radius: 10px;
		cursor: pointer;
		font-size: 13.5px;
	}
	.pick:hover { background: var(--td-surface2); }
	.pick.on { background: rgba(198, 113, 57, .12); }
	.pick input { accent-color: var(--td-accent); width: 15px; height: 15px; flex: none; }
	.pickname { display: flex; flex-direction: column; line-height: 1.3; min-width: 0; }
	.pickhandle { font-size: 11.5px; color: var(--td-muted-2); }
	.pickfoot { font-size: 12px; color: var(--td-muted-2); margin-top: 7px; }

	@media (max-width: 560px) {
		.spool .sclock { text-align: left; }
	}
	@media (prefers-reduced-motion: reduce) {
		.spulse span { animation: none; }
	}

	.agrid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
		gap: 12px;
	}
	/* A button now, so it needs the reset a `div` did not: buttons inherit
	   neither the page font nor its colour, and centre their text. */
	.acard {
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 18px;
		padding: 14px 16px;
		width: 100%;
		text-align: left;
		font-family: inherit;
		color: var(--td-text);
		cursor: pointer;
		transition: border-color .12s ease, transform .12s ease;
	}
	.acard:hover { border-color: var(--td-muted-3); transform: translateY(-1px); }
	@media (prefers-reduced-motion: reduce) {
		.acard { transition: none; }
		.acard:hover { transform: none; }
	}
	.agenre {
		font-size: 12px;
		color: var(--td-muted-2);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		margin-top: 1px;
	}
	.rankedby { font-size: 12.5px; color: var(--td-muted-2); }
	.chiprow { display: flex; gap: 8px; flex-wrap: wrap; margin: 4px 0 10px; }
	.chip {
		border: 1.5px solid var(--td-border);
		background: transparent;
		color: var(--td-text);
		font-family: inherit;
		font-size: 13px;
		padding: 7px 15px;
		border-radius: 999px;
		cursor: pointer;
	}
	.chip:hover { border-color: var(--td-muted-3); }
	.chip.on {
		border-color: var(--td-accent);
		background: color-mix(in srgb, var(--td-accent) 12%, transparent);
	}
	.rangeseg { margin-bottom: 18px; }
	/* Which decks play them, as initials rather than a run of usernames. */
	.wchip {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 20px;
		height: 20px;
		border-radius: 50%;
		background: var(--td-surface2);
		color: var(--td-muted-2);
		font: 700 9.5px ui-monospace, SFMono-Regular, Menlo, monospace;
		margin-left: 3px;
	}
	.acard.both { border-color: color-mix(in srgb, var(--td-accent-2) 42%, transparent); }
	.atop { display: flex; align-items: center; gap: 10px; margin-bottom: 10px; min-width: 0; }
	.amono {
		width: 38px;
		height: 38px;
		border-radius: 12px;
		color: #f4e7cf;
		display: flex;
		align-items: center;
		justify-content: center;
		font: 700 11px ui-monospace, SFMono-Regular, Menlo, monospace;
		flex: none;
	}
	.aname {
		font-size: 14px;
		font-weight: 600;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.abar { height: 6px; border-radius: 999px; background: var(--td-track); overflow: hidden; }
	.abar > div {
		height: 100%;
		border-radius: 999px;
		background: linear-gradient(90deg, var(--td-accent-2), var(--td-accent));
	}
	.ameta {
		display: flex;
		align-items: center;
		gap: 8px;
		margin-top: 8px;
		font-size: 12px;
		color: var(--td-muted-2);
	}
	.awho {
		margin-left: auto;
		font-size: 11px;
		color: var(--td-muted-3);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.dgrid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
		gap: 14px;
		margin-bottom: 24px;
	}
	.dcard {
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 22px;
		padding: 18px 20px;
	}
	.dtop { display: flex; align-items: center; gap: 12px; margin-bottom: 12px; }
	.dbio { margin: 0 0 12px; font-size: 13px; color: var(--td-muted); }
	.dnote { font-size: 12px; color: var(--td-muted-3); margin-bottom: 12px; }
	.dacts { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
	.dchains { font-size: 12px; color: var(--td-muted-3); }
	.chips { display: flex; flex-wrap: wrap; gap: 6px; margin-bottom: 8px; }
	.chip {
		font-size: 12px;
		padding: 4px 10px;
		border-radius: 999px;
		background: var(--td-good-bg);
		color: var(--td-good-fg);
	}
	.chip.neutral { background: var(--td-surface2); color: var(--td-muted); }
	.remote {
		background: var(--td-surface2);
		border: 1px dashed color-mix(in srgb, var(--td-accent) 36%, transparent);
		border-radius: 22px;
		padding: 20px 22px;
	}

	.ihero {
		display: flex;
		align-items: center;
		gap: 16px;
		flex-wrap: wrap;
		margin-bottom: 18px;
	}
	.seg.small { margin-bottom: 0; }
	.seg.small .seg-btn { padding: 7px 14px; font-size: 12.5px; }
	.tiles {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
		gap: 12px;
		margin-bottom: 22px;
	}
	.tiles.small { grid-template-columns: repeat(auto-fit, minmax(130px, 1fr)); }
	.tile {
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 18px;
		padding: 16px 18px;
	}
	.tlabel {
		font-size: 10.5px;
		letter-spacing: 0.12em;
		text-transform: uppercase;
		color: var(--td-muted-2);
		margin-bottom: 6px;
	}
	.tvalue { font-family: 'Caprasimo', serif; font-size: 28px; line-height: 1; }
	.tvalue.small { font-size: 20px; font-family: inherit; font-weight: 600; }
	.tsub { font-size: 12px; color: var(--td-muted-2); margin-top: 6px; }
	.icols {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
		gap: 16px;
		margin-bottom: 26px;
	}
	.icard { border-radius: 22px; padding: 20px; }
	.irow { margin-bottom: 12px; }
	.itop {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 12px;
		margin-bottom: 5px;
		font-size: 13.5px;
		font-weight: 600;
	}
	.iplays { font-size: 12px; color: var(--td-muted-3); font-variant-numeric: tabular-nums; }
	.opshead { display: flex; align-items: center; gap: 12px; margin: 26px 0 14px; }
	.urirow {
		display: flex;
		align-items: center;
		gap: 12px;
		flex-wrap: wrap;
		background: var(--td-surface2);
		border-radius: 12px;
		padding: 9px 12px;
	}
	.urilabel {
		font-size: 10.5px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--td-muted-2);
		flex: none;
	}
	.urirow code {
		font: 500 11.5px ui-monospace, SFMono-Regular, Menlo, monospace;
		overflow-wrap: anywhere;
	}

	.linkish {
		border: none;
		background: none;
		padding: 0;
		font: inherit;
		font-weight: 600;
		color: var(--td-accent);
		cursor: pointer;
	}
	.linkish:hover { color: var(--td-accent-hi); }
</style>
