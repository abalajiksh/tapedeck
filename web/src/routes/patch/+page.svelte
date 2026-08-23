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
		PatchPeer,
		ReelListen,
		PatchArtist,
		DiscoverDeck,
		InstancePulse,
		FederationOps,
		Dub,
		SpoolView,
		SpoolCapture,
		HandedSitting,
		HandedSittingDetail
	} from '$lib/api';
	import CoverArt from '$lib/components/CoverArt.svelte';
	import ChainIcon from '$lib/components/ChainIcon.svelte';
	import { syncUrl } from '$lib/url';
	import { openArtist } from '$lib/entities';

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
	let busy: string | null = null;
	let error = '';

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
	let isAdmin = false;

	// The sitting, and what it has offered. Polled while one is running, because
	// captures arrive from *someone else's* deck and nothing else on this page
	// would learn about them.
	let spool: SpoolView | null = null;
	let captures: SpoolCapture[] = [];
	let dubs: Dub[] = [];
	// Sittings somebody passed on. Loaded with the rest rather than on the tab,
	// because the inbox count on the tab strip has to include them.
	let sittings: HandedSitting[] = [];
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
	$: spoolClock = spool ? clock(spool.started_at, nowSec) : '';

	// A sitting captures only from shared chains, so a host who publishes
	// nothing passes nothing on. Read from the chains endpoint rather than
	// inferred from an empty capture list, which would be indistinguishable
	// from "nothing has played yet".
	let mySharedChains = 0;

	$: pendingCaptures = captures.filter((c) => c.state === 'pending');
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

	async function loadSpool() {
		try {
			const r = await api.getSpool();
			spool = r.spool;
			captures = r.captures;
		} catch {
			/* the tab shows its own empty state */
		}
	}

	async function loadSittings() {
		try {
			sittings = (await api.getSittings()).sittings;
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
		reelLoading = true;
		try {
			const page = await api.getReel({ limit: 40, before: more ? (next ?? undefined) : undefined });
			listens = more ? [...listens, ...page.listens] : page.listens;
			next = page.next;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not load the Reel';
		} finally {
			reelLoading = false;
		}
	}

	onMount(async () => {
		const t = new URLSearchParams(window.location.search).get('tab');
		if (t && (TABS as string[]).includes(t)) tab = t as Tab;
		if (tab === 'artists') void loadArtists();
		if (tab === 'discover') void loadDiscover();
		if (tab === 'instance') void loadInstance();
		await Promise.all([loadDecks(), loadReel(), loadSpool(), loadDubs(), loadSittings()]);
		try {
			mySharedChains = (await api.getChains()).chains.filter((c) => c.shared).length;
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
		}, 1000);
	});

	let timer: ReturnType<typeof setInterval> | undefined;
	let tick: ReturnType<typeof setInterval> | undefined;
	onDestroy(() => {
		clearInterval(timer);
		clearInterval(tick);
	});

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
		busy = 'spool';
		try {
			await api.joinSpool(spool.uuid, accept);
			await loadSpool();
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
			await api.settleCaptures(ids, accept);
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
	<div class="spool" class:invited={spool.my_state === 'invited'}>
		<div class="spulse"><span></span><span></span></div>
		<div class="smain">
			<div class="slabel">
				{spool.my_state === 'invited' ? 'You were asked into a Shared Spool' : 'Shared Spool running'}
			</div>
			<div class="stitle">{spool.title || 'A sitting'}</div>
			<div class="smembers">
				{spool.members
					.filter((m) => m.state === 'joined')
					.map((m) => m.display_name || m.username)
					.join(', ')}
				{#if spool.members.some((m) => m.state === 'invited')}
					· {spool.members.filter((m) => m.state === 'invited').length} invited
				{/if}
			</div>
			<!--
				A sitting does not bypass the sharing switch, so a host who
				publishes no chain captures nothing — and that has to be said
				here rather than discovered from an empty inbox.
			-->
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
				{spool.captured}
				{spool.captured === 1 ? 'play' : 'plays'} offered to you
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

{#if loading}
	<div class="panel note">Loading…</div>
{:else if tab === 'reel'}
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
								<CoverArt
									url={l.artwork_url}
									caaId={l.caa_id}
									releaseMbid={l.caa_release_mbid}
									seed={l.album || l.artist}
									size={38}
								/>
								<div class="lmain">
									<div class="ltop">
										<span class="ltitle">{l.title}</span>
										<span class="lartist">{l.artist}</span>
									</div>
									<div class="lsub">
										<span class="lwho">{l.display_name || l.username}</span>
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
			{#each [['Listens', pulse.listens.toLocaleString(), 'through shared chains'], ['Hours', pulse.hours.toFixed(1), 'where a length was known'], ['Shared chains', String(pulse.shared_chains), 'across all decks'], ['Publishing', `${pulse.publishing}/${pulse.decks}`, 'decks sharing anything']] as [label, value, sub]}
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
				<h3 class="h-display" style="font-size:18px;margin:0 0 4px;">Most-shared chains</h3>
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
							from @{h.from_username}{#if h.started_at} · {new Date(h.started_at * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'long' })}{/if}
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

	{#if pendingCaptures.length}
		<div class="secbar" style="margin-top:26px;">
			<h2 class="h-display sech">Spool handoffs</h2>
			<span class="seccount">{pendingCaptures.length}</span>
		</div>
		<p class="sub" style="margin-top:0;">
			Plays captured on someone else's deck while you were in a Shared Spool.
			<strong>Nothing enters your history until you accept it</strong> — and what you accept
			is stored only here, never forwarded on to Last.fm or ListenBrainz.
		</p>
		<div class="panel rows">
			{#each pendingCaptures as c (c.id)}
				<div class="prow">
					<span class="avatar small">{c.from_username.slice(0, 2).toUpperCase()}</span>
					<div class="pmain">
						<div class="pname">{c.title}</div>
						<div class="pmeta">{c.artist}{#if c.album} · {c.album}{/if}</div>
						<div class="phandle">from @{c.from_username} · {c.spool_title || 'a sitting'}</div>
					</div>
					<div class="pacts">
						<button class="btn btn-primary" disabled={busy === 'captures'}
							on:click={() => settleCaptures([c.id], true)}>Keep</button>
						<button class="btn btn-secondary" disabled={busy === 'captures'}
							on:click={() => settleCaptures([c.id], false)}>Skip</button>
					</div>
				</div>
			{/each}
		</div>
		<div class="bulk">
			<button class="btn btn-primary" disabled={busy === 'captures'}
				on:click={() => settleCaptures(pendingCaptures.map((c) => c.id), true)}>
				Keep all {pendingCaptures.length}
			</button>
			<button class="btn btn-secondary" disabled={busy === 'captures'}
				on:click={() => settleCaptures(pendingCaptures.map((c) => c.id), false)}>
				Decline the rest
			</button>
		</div>
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
