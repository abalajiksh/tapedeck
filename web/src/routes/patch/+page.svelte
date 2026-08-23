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
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { PatchPeer, ReelListen } from '$lib/api';
	import CoverArt from '$lib/components/CoverArt.svelte';
	import ChainIcon from '$lib/components/ChainIcon.svelte';
	import { syncUrl } from '$lib/url';

	type Tab = 'reel' | 'decks' | 'inbox';
	let tab: Tab = 'reel';

	let decks: PatchPeer[] = [];
	let following: PatchPeer[] = [];
	let followers: PatchPeer[] = [];
	let listens: ReelListen[] = [];
	let next: number | null = null;
	let loading = true;
	let reelLoading = false;
	let busy: string | null = null;
	let error = '';

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
		if (t === 'decks' || t === 'inbox') tab = t;
		await Promise.all([loadDecks(), loadReel()]);
		loading = false;
	});

	function go(t: Tab) {
		tab = t;
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
	<button class="seg-btn" class:on={tab === 'reel'} on:click={() => go('reel')}>The Reel</button>
	<button class="seg-btn" class:on={tab === 'decks'} on:click={() => go('decks')}>Decks</button>
	<button class="seg-btn" class:on={tab === 'inbox'} on:click={() => go('inbox')}>
		Inbox{#if pending.length}<span class="count">{pending.length}</span>{/if}
	</button>
</div>

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
	<div class="secbar">
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
	.esub { font-size: 13.5px; color: var(--td-muted); max-width: 54ch; margin: 0 auto; line-height: 1.5; }

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
