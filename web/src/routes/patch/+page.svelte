<script lang="ts">
	// Patch — Phase 6, Step 2.
	//
	// Three of the design's six tabs. Reel, Decks and Inbox are what patches and
	// the per-chain switch make answerable; Artists, Discover and Instance are
	// Step 3, along with Dubs and Shared Spools. They are deliberately absent
	// rather than stubbed — a tab that renders an empty state forever reads as a
	// broken feature, where a tab that is not there yet reads as a roadmap.
	//
	// Everything here is **local**: both ends of every patch are accounts on this
	// instance, so nothing is signed and nothing is delivered. The remote half
	// arrives with Step 3.
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
		SpoolCapture
	} from '$lib/api';
	import CoverArt from '$lib/components/CoverArt.svelte';
	import ChainIcon from '$lib/components/ChainIcon.svelte';
	import { syncUrl } from '$lib/url';

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
	let spoolPicked: Record<string, boolean> = {};
	let spoolTitle = '';
	let showSpoolDialog = false;

	// A sitting captures only from shared chains, so a host who publishes
	// nothing passes nothing on. Read from the chains endpoint rather than
	// inferred from an empty capture list, which would be indistinguishable
	// from "nothing has played yet".
	let mySharedChains = 0;

	$: pendingCaptures = captures.filter((c) => c.state === 'pending');
	$: openDubs = dubs.filter((d) => d.status === 'open');
	// One number on the tab. Three kinds of thing wait in that inbox and a
	// person does not care which until they open it.
	$: inboxCount = pending.length + pendingCaptures.length + openDubs.length;

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
			artists = (await api.getPatchArtists()).artists;
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
		await Promise.all([loadDecks(), loadReel(), loadSpool(), loadDubs()]);
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
	});

	let timer: ReturnType<typeof setInterval> | undefined;
	onDestroy(() => clearInterval(timer));

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
	<h2 class="h-display sech">Artists</h2>
	<p class="sub">
		Across the decks you've patched into, from the chains they share. The number beside a card
		is <strong>your own plays</strong> — the overlap is shown as which artists, never as a
		score.
	</p>
	{#if artists.length}
		<div class="agrid">
			{#each artists as a (a.name)}
				<div class="acard" class:both={a.mine > 0}>
					<div class="atop">
						<span class="amono">{a.name.slice(0, 2).toUpperCase()}</span>
						<span class="aname">{a.name}</span>
					</div>
					<div class="abar">
						<div style="width:{Math.min(100, (a.plays / (artists[0]?.plays || 1)) * 100)}%"></div>
					</div>
					<div class="ameta">
						<span>{a.plays} there{#if a.mine} · {a.mine} yours{/if}</span>
						<span class="awho">{(a.who ?? '').split(',').slice(0, 3).join(' ')}</span>
					</div>
				</div>
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
	{#if pendingCaptures.length}
		<div class="secbar">
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

<style>
	.head { margin-bottom: 14px; }
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
	@media (prefers-reduced-motion: reduce) {
		.spulse span { animation: none; }
	}

	.agrid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
		gap: 12px;
	}
	.acard {
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 18px;
		padding: 14px 16px;
	}
	.acard.both { border-color: color-mix(in srgb, var(--td-accent-2) 42%, transparent); }
	.atop { display: flex; align-items: center; gap: 10px; margin-bottom: 10px; min-width: 0; }
	.amono {
		width: 32px;
		height: 32px;
		border-radius: 10px;
		background: var(--td-surface2);
		color: var(--td-muted-2);
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
