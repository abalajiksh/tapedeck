<script lang="ts">
	// One sitting, after the fact.
	//
	// A Shared Spool used to be a transient: `active_spool` was the only reader
	// of the table, so closing one made the evening disappear from every screen.
	// This is the page it always had the data for.
	//
	// **Membership is what admits you, not the patch.** You were in the room,
	// and un-patching afterwards has no standing to un-happen an evening you
	// attended. What that does *not* widen is what the timeline shows: the
	// per-chain sharing switch still decides that, in the server's SQL, and
	// `withheld` says how much it kept off.
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { api, humanError } from '$lib/api';
	import type { SpoolDetail, SpoolMember, SpoolPlay, PatchPeer } from '$lib/api';
	import CoverArt from '$lib/components/CoverArt.svelte';

	let sitting: SpoolDetail | null = null;
	let loading = true;
	let error = '';
	let notice = '';
	let busy = '';

	// The route parameter. Read off the pathname rather than `$page.params`
	// so it does not depend on generated route types being in step.
	$: uuid = decodeURIComponent($page.url.pathname.split('/').filter(Boolean).pop() ?? '');

	onMount(load);

	async function load() {
		loading = true;
		try {
			sitting = await api.getSpoolDetail(uuid);
			error = '';
		} catch (e) {
			error = humanError(e);
		} finally {
			loading = false;
		}
	}

	// ── Who was there ────────────────────────────────────────────────────
	//
	// `present_from` is the interesting column. NULL means the whole sitting,
	// which is the common case rather than a missing value — somebody added an
	// hour in was almost always in the room from the start.
	const label = (m: SpoolMember) => m.display_name || m.username;

	function span(m: SpoolMember, startedAt: number) {
		if (m.state === 'invited') return 'never answered';
		if (m.state === 'declined') return 'said no';
		const from = m.present_from && m.present_from > startedAt ? clock(m.present_from) : null;
		const until = m.present_until ? clock(m.present_until) : null;
		if (from && until) return `${from} – ${until}`;
		if (from) return `from ${from}`;
		if (until) return `left at ${until}`;
		return 'the whole evening';
	}

	const clock = (ts: number) =>
		new Date(ts * 1000).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });

	function dateLine(s: SpoolDetail) {
		const d = new Date(s.started_at * 1000);
		const day = d.toLocaleDateString(undefined, {
			weekday: 'long',
			day: 'numeric',
			month: 'long',
			year: 'numeric'
		});
		if (!s.ended_at) return `${day} · started ${clock(s.started_at)} · still running`;
		return `${day} · ${clock(s.started_at)} – ${clock(s.ended_at)} · ${ran(s)}`;
	}

	function ran(s: SpoolDetail) {
		if (!s.ended_at) return '';
		const m = Math.max(0, Math.round((s.ended_at - s.started_at) / 60));
		if (m < 60) return `${m} min`;
		const h = Math.floor(m / 60);
		const rest = m % 60;
		return rest ? `${h}h ${rest}m` : `${h}h`;
	}

	// ── The timeline ─────────────────────────────────────────────────────
	//
	// One row per **play**, not per listen — a joint listen is N rows sharing a
	// sitting id, so the naive read shows an evening at twice its length as
	// soon as somebody accepts their half. The server folds that; this draws
	// each row's standing.
	const STANDING: Record<string, string> = {
		mine: 'yours',
		accepted: 'in your history',
		declined: 'declined',
		pending: 'waiting on you',
		theirs: 'not offered'
	};

	function who(p: SpoolPlay) {
		return p.from_display_name || p.from_username;
	}

	$: pending = (sitting?.plays ?? []).filter((p) => p.standing === 'pending');

	async function settle(ids: number[], accept: boolean) {
		if (!ids.length) return;
		busy = 'settle';
		try {
			const r = await api.settleCaptures(ids, accept);
			notice = accept
				? r.forwarded
					? `${r.settled} added to your history — ${r.forwarded} also sent on to your services.`
					: `${r.settled} added to your history. Nothing was forwarded on.`
				: `${r.settled} declined.`;
			await load();
		} catch (e) {
			error = humanError(e);
		} finally {
			busy = '';
		}
	}

	// ── Naming it, and writing about it ──────────────────────────────────
	//
	// The host's, and after the fact on purpose: a sitting is named at the
	// start, which is the moment you know least about the evening.
	let editing = false;
	let draftTitle = '';
	let draftNotes = '';
	let draftGuests = '';

	function openEdit() {
		if (!sitting) return;
		draftTitle = sitting.title ?? '';
		draftNotes = sitting.notes ?? '';
		draftGuests = sitting.guests.join(', ');
		editing = true;
	}

	async function saveEdit() {
		if (!sitting) return;
		busy = 'edit';
		try {
			await api.editSpool(sitting.uuid, {
				title: draftTitle.trim() || null,
				notes: draftNotes.trim() || null,
				guests: draftGuests
					.split(',')
					.map((g) => g.trim())
					.filter(Boolean)
			});
			editing = false;
			await load();
		} catch (e) {
			error = humanError(e);
		} finally {
			busy = '';
		}
	}

	// ── Adding somebody who was there ────────────────────────────────────
	//
	// The "from when" question is answered by the person adding rather than
	// inferred from a clock that means something else. Whole evening is the
	// default because it is the common case by a distance.
	let adding = false;
	let candidates: PatchPeer[] = [];
	let picked: Record<string, boolean> = {};
	let arrivedNow = false;

	async function openAdd() {
		adding = true;
		try {
			const d = await api.getPatchDecks();
			const here = new Set((sitting?.members ?? []).map((m) => m.username));
			candidates = [...d.following, ...d.followers].filter(
				(p, i, all) =>
					p.state === 'accepted' &&
					!here.has(p.username) &&
					all.findIndex((q) => q.username === p.username) === i
			);
		} catch (e) {
			error = humanError(e);
		}
	}

	async function saveAdd() {
		if (!sitting) return;
		const names = Object.keys(picked).filter((u) => picked[u]);
		if (!names.length) return;
		busy = 'add';
		try {
			// `present_from` absent means the whole evening. A real mid-evening
			// arrival says so — and "now" is the honest reading of that,
			// because the host is adding them as they walk in.
			const r = await api.addSpoolMembers(sitting.uuid, {
				with: names,
				present_from: arrivedNow ? Math.floor(Date.now() / 1000) : null
			});
			notice = `${r.added} invited. Nothing reaches their deck until they say yes.`;
			adding = false;
			picked = {};
			arrivedNow = false;
			await load();
		} catch (e) {
			error = humanError(e);
		} finally {
			busy = '';
		}
	}

	// ── Correcting your own presence ─────────────────────────────────────
	//
	// Yours to set, because you are the one who knows. Everything it decides is
	// about your own inbox.
	async function iWasHereAllAlong() {
		if (!sitting) return;
		busy = 'presence';
		try {
			const r = await api.setSpoolPresence(sitting.uuid, { present_from: null });
			notice = r.offered
				? `${r.offered} more ${r.offered === 1 ? 'play' : 'plays'} waiting on you.`
				: 'Nothing else to offer — you already had all of it.';
			await load();
		} catch (e) {
			error = humanError(e);
		} finally {
			busy = '';
		}
	}

	$: me = (sitting?.members ?? []).find((m) => m.user_id === sitting?.viewer_id);
	$: arrivedLate = !!me?.present_from && !!sitting && me.present_from > sitting.started_at;
</script>

<svelte:head>
	<title>{sitting?.title || 'Sitting'} · Tapedeck</title>
</svelte:head>

<a class="back" href="/patch?tab=sittings">‹ Sittings</a>

{#if loading}
	<div class="panel empty"><div class="esub">Reading the evening…</div></div>
{:else if !sitting}
	<div class="panel empty">
		<div class="esub">{error || 'No sitting of yours with that id.'}</div>
	</div>
{:else}
	<header class="head">
		<div>
			<h1 class="h-display">{sitting.title || 'A sitting'}</h1>
			<p class="sub">{dateLine(sitting)}</p>
		</div>
		{#if sitting.hosting}
			<div class="acts">
				<button class="btn btn-secondary" on:click={openEdit}>Name it</button>
				<button class="btn btn-secondary" on:click={openAdd}>Add somebody</button>
			</div>
		{/if}
	</header>

	{#if notice}<div class="notice">{notice}</div>{/if}
	{#if error}<div class="notice bad">{error}</div>{/if}

	<section class="panel">
		<h2 class="sech2">Who was there</h2>
		<div class="people">
			{#each sitting.members as m (m.user_id)}
				<div class="person" class:absent={m.state === 'invited' || m.state === 'declined'}>
					<div class="pname">
						{label(m)}
						{#if m.user_id === sitting.host_user_id}<span class="tag">host</span>{/if}
						{#if m.user_id === sitting.viewer_id}<span class="tag">you</span>{/if}
					</div>
					<div class="pspan">{span(m, sitting.started_at)}</div>
				</div>
			{/each}
			{#each sitting.guests as g (g)}
				<div class="person guest">
					<div class="pname">{g}</div>
					<div class="pspan">no deck here</div>
				</div>
			{/each}
		</div>
		{#if arrivedLate}
			<p class="hint">
				You are down as arriving at {clock(me?.present_from ?? 0)}, so you were only
				offered what played after that.
				<button class="link" on:click={iWasHereAllAlong} disabled={busy === 'presence'}>
					I was here all along
				</button>
			</p>
		{/if}
	</section>

	<section class="panel">
		<div class="tlhead">
			<h2 class="sech2">What played</h2>
			{#if pending.length}
				<div class="acts">
					<button
						class="btn btn-primary"
						disabled={busy === 'settle'}
						on:click={() => settle(pending.map((p) => p.capture_id!).filter(Boolean), true)}
					>
						Keep {pending.length}
					</button>
					<button
						class="btn btn-secondary"
						disabled={busy === 'settle'}
						on:click={() => settle(pending.map((p) => p.capture_id!).filter(Boolean), false)}
					>
						Decline
					</button>
				</div>
			{/if}
		</div>

		{#if sitting.plays.length}
			<div class="tl">
				{#each sitting.plays as p (p.played_at + p.title)}
					<div class="play" class:pending={p.standing === 'pending'}>
						<span class="at">{clock(p.played_at)}</span>
						<CoverArt
							url={p.artwork_url}
							caaId={p.caa_id}
							releaseMbid={p.caa_release_mbid}
							seed={p.title}
							size={38}
						/>
						<div class="pmain">
							<div class="ptitle">{p.title}</div>
							<div class="partist">
								{p.artist}{#if p.album} — {p.album}{/if}
							</div>
						</div>
						<div class="pside">
							<span class="deck">{who(p)}</span>
							<span class="standing st-{p.standing}">{STANDING[p.standing]}</span>
						</div>
					</div>
				{/each}
			</div>
		{:else}
			<div class="esub">
				Nothing on the timeline. Either no deck in the room played anything, or what
				played ran through chains nobody shares.
			</div>
		{/if}

		{#if sitting.withheld}
			<!--
				Counted and stated rather than quietly dropped. A timeline with
				invisible holes misrepresents the evening and the reader could
				never tell — the rule the hand-over document already carries.
			-->
			<p class="withheld">
				{sitting.withheld}
				{sitting.withheld === 1 ? 'play is' : 'plays are'} not shown — they ran through chains
				their owner does not share.
			</p>
		{/if}
	</section>

	{#if sitting.notes}
		<section class="panel">
			<h2 class="sech2">Notes</h2>
			<p class="notes">{sitting.notes}</p>
		</section>
	{/if}
{/if}

{#if editing && sitting}
	<div class="scrim" role="presentation" on:click={() => (editing = false)}></div>
	<div class="dialog" role="dialog" aria-label="Name this sitting">
		<h3 class="h-display">Name this sitting</h3>
		<p class="sub">You name an evening for what it turned out to be.</p>
		<label class="fld">
			<span>Title</span>
			<input bind:value={draftTitle} placeholder="The long one" />
		</label>
		<label class="fld">
			<span>Notes</span>
			<textarea bind:value={draftNotes} rows="4" placeholder="Ran to 3am…"></textarea>
		</label>
		<label class="fld">
			<span>Anyone else in the room</span>
			<input bind:value={draftGuests} placeholder="Tobias, Marie" />
			<small
				>Names, comma separated — the people with no deck here. They are named in the
				company of every play from now on.</small
			>
		</label>
		<div class="dacts">
			<button class="btn btn-secondary" on:click={() => (editing = false)}>Close</button>
			<button class="btn btn-primary" disabled={busy === 'edit'} on:click={saveEdit}>
				Save
			</button>
		</div>
	</div>
{/if}

{#if adding && sitting}
	<div class="scrim" role="presentation" on:click={() => (adding = false)}></div>
	<div class="dialog" role="dialog" aria-label="Add somebody to this sitting">
		<h3 class="h-display">Add somebody</h3>
		<p class="sub">
			Nothing reaches their deck until they say yes, and then only plays through chains
			their owner shares.
		</p>
		{#if candidates.length}
			<div class="picks">
				{#each candidates as c (c.username)}
					<label class="pick">
						<input type="checkbox" bind:checked={picked[c.username]} />
						<span>{c.display_name || c.username}</span>
					</label>
				{/each}
			</div>
			<!--
				The "from when" question, and the default is the common case: a
				host adding somebody an hour in has usually just remembered the
				person sitting in front of them.
			-->
			<div class="seg">
				<button class="seg-btn" class:on={!arrivedNow} on:click={() => (arrivedNow = false)}>
					Here all along
				</button>
				<button class="seg-btn" class:on={arrivedNow} on:click={() => (arrivedNow = true)}>
					Just arrived
				</button>
			</div>
			<p class="hint">
				{arrivedNow
					? 'They will only be offered what plays from now on.'
					: 'They will be offered the whole evening.'}
			</p>
		{:else}
			<p class="hint">
				Everybody you are patched with is already in this sitting.
			</p>
		{/if}
		<div class="dacts">
			<button class="btn btn-secondary" on:click={() => (adding = false)}>Close</button>
			<button
				class="btn btn-primary"
				disabled={busy === 'add' || !candidates.length}
				on:click={saveAdd}
			>
				Add
			</button>
		</div>
	</div>
{/if}

<style>
	.back {
		display: inline-block;
		margin-bottom: 12px;
		font-size: 13px;
		color: var(--td-muted);
		text-decoration: none;
	}
	.back:hover {
		color: var(--td-text);
	}
	.head {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 16px;
		flex-wrap: wrap;
		margin-bottom: 16px;
	}
	.head h1 {
		margin: 0;
	}
	.sub {
		margin: 4px 0 0;
		color: var(--td-muted);
		font-size: 13px;
	}
	.acts {
		display: flex;
		gap: 8px;
	}
	.panel {
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 14px;
		padding: 18px;
		margin-bottom: 16px;
	}
	.sech2 {
		margin: 0 0 12px;
		font-size: 13px;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--td-muted);
	}
	.people {
		display: flex;
		flex-wrap: wrap;
		gap: 10px;
	}
	.person {
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
		border-radius: 10px;
		padding: 8px 12px;
		min-width: 150px;
	}
	.person.absent {
		opacity: 0.55;
	}
	.person.guest {
		border-style: dashed;
	}
	.pname {
		font-size: 14px;
		font-weight: 600;
	}
	.pspan {
		font-size: 12px;
		color: var(--td-muted);
		margin-top: 2px;
	}
	.tag {
		font-size: 10px;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--td-muted);
		border: 1px solid var(--td-border);
		border-radius: 5px;
		padding: 1px 4px;
		margin-left: 4px;
		vertical-align: middle;
	}
	.tlhead {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		flex-wrap: wrap;
	}
	.tl {
		display: flex;
		flex-direction: column;
	}
	.play {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 8px 0;
		border-top: 1px solid var(--td-border);
	}
	.play:first-child {
		border-top: none;
	}
	.at {
		font-variant-numeric: tabular-nums;
		font-size: 12px;
		color: var(--td-muted);
		min-width: 44px;
	}
	.pmain {
		flex: 1;
		min-width: 0;
	}
	.ptitle {
		font-size: 14px;
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.partist {
		font-size: 12px;
		color: var(--td-muted);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.pside {
		display: flex;
		align-items: center;
		gap: 10px;
		flex: none;
	}
	.deck {
		font-size: 12px;
		color: var(--td-muted);
	}
	.standing {
		font-size: 11px;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		border: 1px solid var(--td-border);
		border-radius: 6px;
		padding: 2px 6px;
		color: var(--td-muted);
	}
	.st-pending {
		border-color: var(--td-accent);
		color: var(--td-accent);
	}
	.st-mine,
	.st-accepted {
		border-color: var(--td-good-fg);
		color: var(--td-good-fg);
	}
	.withheld {
		margin: 12px 0 0;
		font-size: 12px;
		color: var(--td-muted);
	}
	.notes {
		margin: 0;
		white-space: pre-wrap;
		font-size: 14px;
		line-height: 1.55;
	}
	.hint {
		margin: 12px 0 0;
		font-size: 12px;
		color: var(--td-muted);
	}
	.link {
		background: none;
		border: none;
		padding: 0;
		font: inherit;
		color: var(--td-accent);
		cursor: pointer;
		text-decoration: underline;
	}
	.notice {
		background: var(--td-good-bg);
		color: var(--td-good-fg);
		border-radius: 10px;
		padding: 10px 14px;
		font-size: 13px;
		margin-bottom: 14px;
	}
	.notice.bad {
		background: var(--td-bad-bg);
		color: var(--td-bad-fg);
	}
	.esub {
		color: var(--td-muted);
		font-size: 13px;
	}
	.empty {
		text-align: center;
	}
	.scrim {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.5);
		z-index: 40;
	}
	.dialog {
		position: fixed;
		z-index: 41;
		top: 50%;
		left: 50%;
		transform: translate(-50%, -50%);
		width: min(460px, calc(100vw - 32px));
		max-height: calc(100vh - 64px);
		overflow: auto;
		background: var(--td-surface);
		color: var(--td-text);
		border: 1px solid var(--td-border);
		border-radius: 14px;
		padding: 20px;
	}
	.dialog h3 {
		margin: 0;
	}
	.fld {
		display: block;
		margin-top: 14px;
	}
	.fld > span {
		display: block;
		font-size: 12px;
		color: var(--td-muted);
		margin-bottom: 4px;
	}
	.fld input,
	.fld textarea {
		width: 100%;
		box-sizing: border-box;
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
		border-radius: 8px;
		padding: 8px 10px;
		color: var(--td-text);
		font: inherit;
	}
	.fld small {
		display: block;
		margin-top: 4px;
		font-size: 11px;
		color: var(--td-muted);
	}
	.picks {
		display: flex;
		flex-direction: column;
		gap: 6px;
		margin: 14px 0;
	}
	.pick {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 14px;
	}
	.seg {
		display: inline-flex;
		border: 1px solid var(--td-border);
		border-radius: 8px;
		overflow: hidden;
	}
	.seg-btn {
		background: none;
		border: none;
		padding: 6px 12px;
		font: inherit;
		font-size: 13px;
		color: var(--td-muted);
		cursor: pointer;
	}
	.seg-btn.on {
		background: var(--td-accent);
		color: #fff;
	}
	.dacts {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
		margin-top: 18px;
	}
	@media (max-width: 560px) {
		.pside {
			flex-direction: column;
			align-items: flex-end;
			gap: 2px;
		}
	}
</style>
