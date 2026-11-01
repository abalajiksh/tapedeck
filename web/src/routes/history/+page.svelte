<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount, onDestroy } from 'svelte';
	import { syncUrl } from '$lib/url';
	import { api, lyricKey } from '$lib/api';
	import type { Scrobble, SignalChain , ChainVariant } from '$lib/api';
	import ScrobbleRow from '$lib/components/ScrobbleRow.svelte';
	import InspectListen from '$lib/components/InspectListen.svelte';
	import { loadLoves, lovedKeys, isLoved, toggleLove } from '$lib/loves';
	import SessionCard from '$lib/components/SessionCard.svelte';
	import ToTop from '$lib/components/ToTop.svelte';
	import Pager from '$lib/components/Pager.svelte';
	import type { Session } from '$lib/api';

	let scrobbles: Scrobble[] = [];
	let chains: SignalChain[] = [];
	/** Chain variants, so a sitting can name which setup it was. */
	let variants: ChainVariant[] = [];
	// Keyed by id for the row glyph — a listen carries a chain_id, not a chain.
	$: chainsById = Object.fromEntries(chains.map((c) => [c.id, { name: c.name, icon: c.icon }]));
	let filter = '';
	let loading = true;
	// Numbered pages kept in the URL, so a reload or Back lands where you were.
	// shortcut: offset paging drifts by however many listens arrive while you
	// read page 2+; keyset on (timestamp, id) if that ever bites.
	let page = 1;
	let total = 0;
	const PAGE = 50;
	let jumpDate = '';

	// ── Sessions ──
	// A calendar day is an arbitrary boundary for listening; a session is the
	// thing you actually did. Grouping is *stored*, not recomputed here, because
	// merges and splits are decisions no heuristic can re-derive.
	let groupBy: 'day' | 'session' = 'day';
	let sessions: Session[] = [];
	let sessionsLoading = false;
	let sessPage = 1;
	let sessTotal = 0;
	const SESS_PAGE = 30;
	let mergeMode = false;
	let mergePick = new Set<number>();
	let merging = false;
	$: mergeCount = mergePick.size;

	async function loadSessions() {
		sessionsLoading = true;
		try {
			const res = await api.getSessions(SESS_PAGE, (sessPage - 1) * SESS_PAGE);
			sessions = res.sessions;
			sessTotal = res.total;
		} catch (e) { console.warn('sessions', e); }
		sessionsLoading = false;
	}

	function setGroupBy(g: 'day' | 'session') {
		groupBy = g;
		if (g === 'session' && !sessions.length) loadSessions();
		if (g === 'day') exitMerge();
		syncPlace();
	}

	/** Write the view, scope and page into the address bar. */
	function syncPlace() {
		const q = new URLSearchParams();
		if (scope.artist) q.set('artist', scope.artist);
		if (scope.album) q.set('album', scope.album);
		if (scope.track) q.set('track', scope.track);
		if (groupBy === 'session') {
			q.set('view', 'session');
			if (sessPage > 1) q.set('page', String(sessPage));
		} else if (page > 1) q.set('page', String(page));
		const qs = q.toString();
		syncUrl(`/history${qs ? '?' + qs : ''}`);
	}

	// Selections and merge picks are sets of ids, so they survive a page turn.
	function goPage(p: number) {
		page = p;
		syncPlace();
		void load();
		window.scrollTo({ top: 0 });
	}
	function goSessPage(p: number) {
		sessPage = p;
		syncPlace();
		void loadSessions();
		window.scrollTo({ top: 0 });
	}

	/** The page holding the last listen on or before that day: count what's
	 *  newer under the same filters, and that count *is* the offset. */
	async function jumpTo() {
		if (!jumpDate) return;
		const end = new Date(jumpDate + 'T00:00:00');
		end.setDate(end.getDate() + 1);
		try {
			const newer = (await api.getScrobbles({ ...filters(), limit: 1, after: Math.floor(end.getTime() / 1000) })).total;
			goPage(Math.floor(newer / PAGE) + 1);
		} catch (e) { console.warn('jump', e); }
	}

	function toggleMergePick(sess: Session) {
		if (mergePick.has(sess.id)) mergePick.delete(sess.id);
		else mergePick.add(sess.id);
		mergePick = mergePick;
	}
	function exitMerge() { mergeMode = false; mergePick = new Set(); }

	async function applyMerge() {
		if (mergePick.size < 2) return;
		merging = true;
		try {
			await api.mergeSessions([...mergePick]);
			exitMerge();
			await loadSessions();
		} catch (e) { alert('Merge failed: ' + String(e).replace(/^API \d+: /, '')); }
		merging = false;
	}

	// Search results link here with ?artist= / ?album= / ?track=, so a result
	// opens the actual listens rather than a dead end.
	let scope: { artist?: string; album?: string; track?: string } = {};
	$: scopeLabel = scope.track ?? scope.album ?? scope.artist ?? '';

	function clearScope() {
		scope = {};
		load(true);
	}

	// row menu + dialogs
	let menuFor: Scrobble | null = null;
	// Anchored to the ⋯ button; opens upward in the lower half so it stays on screen.
	let menuPos = '';
	function openMenu(d: { scrobble: Scrobble; rect: DOMRect }) {
		const r = d.rect;
		const v = r.bottom > window.innerHeight / 2 ? `bottom:${window.innerHeight - r.top}px` : `top:${r.bottom}px`;
		menuPos = `${v};right:${window.innerWidth - r.right}px;`;
		menuFor = d.scrobble;
	}
	let inspecting: Scrobble | null = null;
	let editing: Scrobble | null = null;
	let edit = { title: '', artist: '', album: '', mbid_recording: '', signal_chain_id: 0, chain_variant_id: 0, format_type: '' };
	let confirmDelete: Scrobble | null = null;
	let saving = false;

	// bulk selection
	let selectMode = false;
	let selected = new Set<number>();
	let bulkChain = '';
	let bulkCompany = '';
	// null = everything. The third state is load-bearing: an unmarked listen is
	// unmarked, not proven solitary, so "everything" cannot be spelled as
	// "solo + social".
	let socialFilter: boolean | null = null;
	/** Which setup of that chain. A variant belongs to exactly one chain, so
	 *  this resets whenever the chain does. */
	let bulkVariant = '';
	$: selectedCount = selected.size;

	function toggleSelect(s: Scrobble) {
		if (selected.has(s.id)) selected.delete(s.id);
		else selected.add(s.id);
		selected = selected; // trigger reactivity
	}
	function exitSelect() {
		selectMode = false;
		selected = new Set();
		bulkChain = '';
		bulkVariant = '';
		bulkCompany = '';
		shiftAmount = '';
		shiftDir = -1;
		timeNotice = '';
	}

	/**
	 * Mark the selected listens as heard in company.
	 *
	 * Its own action rather than a field on the chain Apply, and that is not
	 * fussiness: the chain picker's empty option *means* "clear the chain", so
	 * one Apply covering both fields could not tell "leave the company alone"
	 * from "unmark it". That is the same absent-versus-null distinction the
	 * endpoint refuses to guess at, appearing in the UI.
	 */
	async function applyBulkCompany(clear = false) {
		if (!selected.size) return;
		const who = clear ? null : bulkCompany.trim();
		if (!clear && !who) return;
		saving = true;
		try {
			await api.bulkSetCompany([...selected], who);
			scrobbles = scrobbles.map((s) =>
				selected.has(s.id) ? { ...s, company: who } : s
			);
			exitSelect();
		} catch (e) {
			alert('Could not save that: ' + e);
		}
		saving = false;
	}

	// ── Moving listens in time ───────────────────────────────────────────
	//
	// **An offset over a selection, not an absolute time**, and that is the
	// shape of the actual failure rather than a limitation. A timestamp is
	// almost never wrong on its own: a clock was out by an hour, an evening was
	// reconstructed from times laid out by arithmetic, or a capture arrived
	// carrying somebody else's clock. Every one of those moves a *run* by a
	// *constant*, and a constant cannot scramble an ordering the way a set of
	// hand-typed times can.
	//
	// Where the play has not been accepted out of a sitting yet, the capture's
	// own re-time is the cheaper place to do it — nothing is in a history
	// there, so none of the consequences below apply.
	let shiftAmount = '';
	/** -1 earlier, +1 later. Two buttons rather than a minus sign somebody has
	 *  to remember to type, and a direction the control states out loud. */
	let shiftDir: -1 | 1 = -1;
	let timeNotice = '';

	/** "1h30", "90m", "45", "2h" — minutes unless it says otherwise. */
	function parseShift(raw: string): number | null {
		const t = raw.trim().toLowerCase();
		if (!t) return null;
		const m = t.match(/^(?:(\d+)\s*h)?\s*(?:(\d+)\s*m?)?$/);
		if (!m || (!m[1] && !m[2])) return null;
		const secs = (Number(m[1] ?? 0) * 60 + Number(m[2] ?? 0)) * 60;
		return secs > 0 ? secs : null;
	}
	$: shiftSecs = parseShift(shiftAmount);

	async function applyShift() {
		if (!selected.size || !shiftSecs) return;
		const offset = shiftSecs * shiftDir;
		saving = true;
		try {
			const r = await api.shiftListenTimes([...selected], offset);
			// Everything it could not reach is said out loud. A correction
			// somebody believes went further than it did is worse than one they
			// know the edges of.
			const bits = [`${r.moved} moved`];
			if (r.moved < selected.size)
				bits.push(
					`${selected.size - r.moved} left alone — moving them would have landed on another play of the same track`
				);
			if (r.already_forwarded)
				bits.push(
					`${r.already_forwarded} had already gone to Last.fm or ListenBrainz, and neither can be edited — their copy keeps the old time`
				);
			if (r.grouping_stale)
				bits.push('sessions no longer match the gap they were grouped by — rebuild when you want them regrouped');
			timeNotice = bits.join('. ') + '.';
			// A full reload rather than patching the rows in place: moving a
			// listen can reorder the list and change which day it falls under,
			// so the page's own grouping has to be rebuilt from the answer.
			selected = new Set();
			await load();
		} catch (e) {
			timeNotice = 'Could not move those: ' + e;
		}
		saving = false;
	}

	async function applyBulkChain() {
		if (!selected.size) return;
		const cid = bulkChain ? Number(bulkChain) : null;
		const vid = cid && bulkVariant ? Number(bulkVariant) : null;
		saving = true;
		try {
			await api.bulkSetChain([...selected], cid, vid);
			const chain = cid; // scrobbles carry chain_id
			scrobbles = scrobbles.map((s) =>
				selected.has(s.id) ? { ...s, chain_id: chain, chain_variant_id: vid } : s
			);
			exitSelect();
		} catch (e) { alert('Bulk update failed: ' + e); }
		saving = false;
	}

	/** A variant belongs to exactly one chain, so each picker offers only that
	 *  chain's — and clears itself when the chain changes underneath it. */
	$: bulkVariants = variants.filter((v) => v.chain_id === Number(bulkChain));
	$: if (bulkChain !== undefined && !bulkVariants.some((v) => String(v.id) === bulkVariant))
		bulkVariant = '';
	$: editVariants = variants.filter((v) => v.chain_id === edit.signal_chain_id);
	$: if (!editVariants.some((v) => v.id === edit.chain_variant_id)) edit.chain_variant_id = 0;

	$: groups = groupByDay(scrobbles);

	function dayLabel(ts: number): string {
		const d = new Date(ts * 1000);
		const today = new Date(); today.setHours(0, 0, 0, 0);
		const yst = new Date(today); yst.setDate(yst.getDate() - 1);
		const day = new Date(d); day.setHours(0, 0, 0, 0);
		if (day.getTime() === today.getTime()) return 'Today';
		if (day.getTime() === yst.getTime()) return 'Yesterday';
		return d.toLocaleDateString('en-GB', { weekday: 'short', day: 'numeric', month: 'short' });
	}

	function groupByDay(list: Scrobble[]): { label: string; items: Scrobble[] }[] {
		const out: { label: string; items: Scrobble[] }[] = [];
		for (const s of list) {
			const label = dayLabel(s.timestamp);
			const last = out[out.length - 1];
			if (last && last.label === label) last.items.push(s);
			else out.push({ label, items: [s] });
		}
		return out;
	}

	function filters() {
		return {
			// The typed filter is an artist filter; a scope from search can
			// narrow by album or track instead.
			artist: filter || scope.artist || undefined,
			album: scope.album,
			track: scope.track,
			social: socialFilter ?? undefined
		};
	}

	/** `reset` is a filter change: back to page 1, since the old page number
	 *  means nothing under a different filter. */
	async function load(reset = false) {
		if (reset) { page = 1; syncPlace(); }
		loading = true;
		try {
			const res = await api.getScrobbles({ ...filters(), limit: PAGE, offset: (page - 1) * PAGE });
			scrobbles = res.scrobbles;
			total = res.total;
			// Past the end (a stale URL, or deletes): land on the last real page.
			if (!scrobbles.length && total && page > 1) {
				loading = false;
				return goPage(Math.ceil(total / PAGE));
			}
		} catch (e) { console.warn('history', e); }
		loading = false;
	}

	// Decks this person is patched with, so a sitting can be handed over. Loaded
	// once for the page rather than per card, and a failure simply hides the
	// button — being unable to reach the Patch endpoints is not a reason for the
	// History page to show an error.
	let peers: { username: string; display_name: string | null }[] = [];
	async function loadPeers() {
		try {
			const d = await api.getPatchDecks();
			const out = new Map<string, { username: string; display_name: string | null }>();
			for (const p of [...d.following, ...d.followers]) {
				if (p.state === 'accepted')
					out.set(p.username, { username: p.username, display_name: p.display_name });
			}
			peers = [...out.values()].sort((a, b) =>
				(a.display_name || a.username).localeCompare(b.display_name || b.username)
			);
		} catch {
			/* no button, which is the right outcome when there is nobody */
		}
	}

	function setSocial(v: boolean | null) {
		socialFilter = v;
		void load(true);
	}

	let filterTimer: ReturnType<typeof setTimeout>;
	function onFilter() {
		clearTimeout(filterTimer);
		filterTimer = setTimeout(() => load(true), 300);
	}

	// ── Keeping up with what's playing ──
	// This page used to load once and never again, so leaving it open through an
	// album meant watching a list that had stopped being the history. Only page
	// 1 follows along: on an older page new listens would shuffle rows under
	// the reader, so it stays put and the total catches up on the next turn.
	const REFRESH_MS = 20_000;
	let refreshTimer: ReturnType<typeof setInterval> | null = null;
	const onWake = () => { if (!document.hidden) refreshHead(); };

	/** Refreshing under an open dialog or a half-finished bulk edit moves the
	 *  thing being acted on. None of these last long, and the next tick catches
	 *  up the moment they're closed. */
	$: refreshPaused =
		selectMode || mergeMode || !!editing || !!confirmDelete || !!inspecting || !!menuFor;

	async function refreshHead() {
		if (refreshPaused || loading || (typeof document !== 'undefined' && document.hidden)) return;
		if (groupBy === 'day' ? page !== 1 : sessPage !== 1) return;
		try {
			const res = await api.getScrobbles({ ...filters(), limit: PAGE, offset: 0 });
			const known = new Set(scrobbles.map((s) => s.id));
			if (!res.scrobbles.some((s) => !known.has(s.id))) return;
			// Replaced, not merged: page 1 is a fixed window, and the server's
			// order is the one groupByDay needs.
			scrobbles = res.scrobbles;
			total = res.total;
			if (groupBy === 'session') loadSessions();
		} catch {
			// Same call the deck poll makes: a failed refresh leaves what's on
			// screen alone rather than flickering it, and the next tick corrects.
		}
	}

	/** Open the lyric reader for a listen.
	 *
	 *  Offered on every listen rather than only where lyrics are held: knowing
	 *  which have them was, at first, a lookup per row — so the item was offered
	 *  on every listen and the reader's own empty state explained itself.
	 *
	 *  It is gated now, for **one** request per page: `/api/v1/lyrics/keys`
	 *  returns the whole set at once and a row's key is derived here, which is
	 *  exactly the shape `/loves/keys` and `loveKey` already have. The standing
	 *  trap comes with it — `lyricKey` must stay in step with the server's fold
	 *  or the item quietly stops appearing on rows that do have words, and
	 *  nothing errors.
	 *
	 *  A failed fetch leaves the set empty, and an empty set means *offer it
	 *  everywhere* rather than nowhere: the reader explains itself, and a menu
	 *  item missing because a request failed is the worse of the two. */
	let lyricKeys = new Set<string>();

	async function loadLyricKeys() {
		try {
			lyricKeys = new Set((await api.lyricKeys()).keys);
		} catch {
			lyricKeys = new Set();
		}
	}

	function hasWords(s: Scrobble): boolean {
		return lyricKeys.size === 0 || lyricKeys.has(lyricKey(s.artist, s.title));
	}

	function readWords(s: Scrobble) {
		menuFor = null;
		const q = new URLSearchParams({ artist: s.artist, title: s.title });
		if (s.album) q.set('album', s.album);
		goto(`/lyrics?${q}`);
	}

	function openEdit(s: Scrobble) {
		menuFor = null;
		editing = s;
		edit = {
			title: s.title, artist: s.artist, album: s.album ?? '',
			mbid_recording: s.mbid_recording ?? '',
			signal_chain_id: s.chain_id ?? 0, chain_variant_id: s.chain_variant_id ?? 0,
			format_type: s.format_type ?? ''
		};
	}

	async function saveEdit() {
		if (!editing) return;
		saving = true;
		try {
			const changes: Record<string, unknown> = {
				title: edit.title, artist: edit.artist, album: edit.album,
				mbid_recording: edit.mbid_recording, format_type: edit.format_type
			};
			if (edit.signal_chain_id) {
				changes.signal_chain_id = edit.signal_chain_id;
				// Sent alongside, never on its own: the server rewrites the setup
				// whenever the chain is set, so leaving it out here is how you say
				// "no particular setup" rather than "leave it".
				if (edit.chain_variant_id) changes.chain_variant_id = edit.chain_variant_id;
			}
			const updated = await api.patchScrobble(editing.id, changes);
			scrobbles = scrobbles.map((x) => (x.id === updated.id ? updated : x));
			editing = null;
		} catch (e) { alert('Failed to save: ' + e); }
		saving = false;
	}

	async function doDelete(s: Scrobble) {
		try {
			await api.deleteScrobble(s.id);
			scrobbles = scrobbles.filter((x) => x.id !== s.id);
			total -= 1;
		} catch (e) { alert('Failed to delete: ' + e); }
		confirmDelete = null;
		menuFor = null;
	}

	onMount(async () => {
		const q = new URLSearchParams(window.location.search);
		scope = {
			artist: q.get('artist') ?? undefined,
			album: q.get('album') ?? undefined,
			track: q.get('track') ?? undefined
		};
		const p = Math.max(1, Number(q.get('page')) || 1);
		if (q.get('view') === 'session') {
			groupBy = 'session';
			sessPage = p;
			void loadSessions();
		} else page = p;
		loadLoves();
		// One request for the whole set, the way loves are loaded. Not awaited:
		// the menu item is offered everywhere until it arrives, which is the
		// safe direction — see `hasWords`.
		void loadLyricKeys();
		try { chains = (await api.getChains()).chains; } catch { /* optional */ }
		// Optional in the same way: an instance with no variants simply shows no
		// variant picker on a session card.
		try { variants = (await api.getVariants()).variants; } catch { /* optional */ }
		void loadPeers();
		await load();

		refreshTimer = setInterval(refreshHead, REFRESH_MS);
		// Coming back to the tab is the moment the list is most obviously stale,
		// and the interval skipped its ticks while it was hidden anyway — a
		// backgrounded tab must not poll for an hour.
		document.addEventListener('visibilitychange', onWake);
		window.addEventListener('focus', onWake);
	});

	// Not a `return` out of `onMount` above: that only registers a teardown when
	// the callback is synchronous, and this one is `async` — Svelte gets a
	// Promise and ignores it, so the interval would outlive the page.
	onDestroy(() => {
		if (refreshTimer) clearInterval(refreshTimer);
		if (typeof document === 'undefined') return;
		document.removeEventListener('visibilitychange', onWake);
		window.removeEventListener('focus', onWake);
	});
</script>

<svelte:head><title>History — Tapedeck</title></svelte:head>

<div style="display:flex;align-items:flex-end;justify-content:space-between;gap:16px;margin-bottom:24px;flex-wrap:wrap;">
	<div>
		<h1 class="h-display" style="font-size:38px;">History</h1>
		<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;">Every side you've ever pressed play on.</p>
	</div>
	<div style="display:flex;gap:10px;align-items:center;flex-wrap:wrap;">
		<div style="display:inline-flex;background:var(--td-sidebar);border-radius:999px;padding:3px;gap:2px;">
			<button class="seg-btn" class:on={groupBy === 'day'} on:click={() => setGroupBy('day')}>By day</button>
			<button class="seg-btn" class:on={groupBy === 'session'} on:click={() => setGroupBy('session')}>By session</button>
		</div>
		{#if groupBy === 'session'}
			{#if mergeMode}
				<button class="btn btn-secondary" on:click={exitMerge}>Done</button>
			{:else}
				<button class="btn btn-secondary" on:click={() => (mergeMode = true)}>Merge…</button>
			{/if}
		{:else if selectMode}
			<button class="btn btn-secondary" on:click={exitSelect}>Done</button>
		{:else}
			<button class="btn btn-secondary" on:click={() => (selectMode = true)}>Select</button>
		{/if}
		{#if groupBy === 'day'}
			<!--
				Three states, not two. "Everything" is not "alone plus in
				company": a listen nobody marked is *unmarked*, and most of any
				history predates the marking existing — so collapsing this to a
				pair would quietly claim every old listen was solitary.
			-->
			<div style="display:inline-flex;background:var(--td-sidebar);border-radius:999px;padding:3px;gap:2px;">
				<button class="seg-btn" class:on={socialFilter === null} on:click={() => setSocial(null)}>All</button>
				<button class="seg-btn" class:on={socialFilter === true} on:click={() => setSocial(true)}>With others</button>
				<button class="seg-btn" class:on={socialFilter === false} on:click={() => setSocial(false)}>Unmarked</button>
			</div>
			<input class="input" style="width:200px;" bind:value={filter} on:input={onFilter} placeholder="Filter by artist…">
			<input class="input" style="width:auto;" type="date" bind:value={jumpDate} on:change={jumpTo} aria-label="Jump to a date">
		{/if}
	</div>
</div>

{#if scopeLabel}
	<div class="panel" style="border-radius:14px;padding:10px 16px;margin-bottom:16px;display:flex;align-items:center;gap:10px;flex-wrap:wrap;">
		<span style="font-size:13px;color:var(--td-muted);">
			Showing {scope.track ? 'listens of' : scope.album ? 'the album' : 'everything by'}
		</span>
		<span style="font-weight:600;font-size:14px;">{scopeLabel}</span>
		<span style="flex:1;"></span>
		<button class="btn btn-ghost" style="font-size:12.5px;" on:click={clearScope}>Clear</button>
	</div>
{/if}

{#if selectMode}
	<div class="panel" style="position:sticky;top:0;z-index:30;border-radius:14px;padding:12px 16px;margin-bottom:16px;display:flex;align-items:center;gap:12px;flex-wrap:wrap;background:var(--td-surface2);">
		<span style="font-weight:600;font-size:14px;">{selectedCount} selected</span>
		<span style="flex:1;"></span>
		<span style="font-size:13px;color:var(--td-muted);">Set chain:</span>
		<select class="input" style="width:auto;min-width:170px;" bind:value={bulkChain}>
			<option value="">— clear chain —</option>
			{#each chains as c}<option value={c.id}>{c.name}</option>{/each}
		</select>
		{#if bulkVariants.length}
			<select class="input" style="width:auto;min-width:150px;" bind:value={bulkVariant}
				aria-label="Which setup of that chain">
				<option value="">— as the chain is —</option>
				{#each bulkVariants as v}<option value={v.id}>{v.name}</option>{/each}
			</select>
		{/if}
		<button class="btn btn-primary" on:click={applyBulkChain} disabled={!selectedCount || saving}>{saving ? 'Applying…' : 'Apply'}</button>
		<div style="flex-basis:100%;height:0;"></div>
		<span style="font-size:13px;color:var(--td-muted);">Heard with:</span>
		<input
			class="input"
			style="width:auto;min-width:200px;flex:1;"
			bind:value={bulkCompany}
			maxlength="200"
			placeholder="Kim, and whoever else was there"
			on:keydown={(e) => e.key === 'Enter' && applyBulkCompany()}
		/>
		<button class="btn btn-primary" on:click={() => applyBulkCompany()} disabled={!selectedCount || saving || !bulkCompany.trim()}>
			Mark
		</button>
		<button class="btn btn-secondary" on:click={() => applyBulkCompany(true)} disabled={!selectedCount || saving}>
			Unmark
		</button>
		<div style="flex-basis:100%;height:0;"></div>
		<span style="font-size:13px;color:var(--td-muted);">Move in time:</span>
		<div style="display:flex;background:var(--td-sidebar);border-radius:999px;padding:4px;gap:2px;">
			<button class="seg-btn" class:on={shiftDir === -1} on:click={() => (shiftDir = -1)}>Earlier</button>
			<button class="seg-btn" class:on={shiftDir === 1} on:click={() => (shiftDir = 1)}>Later</button>
		</div>
		<input
			class="input"
			style="width:auto;min-width:110px;"
			bind:value={shiftAmount}
			placeholder="1h30"
			aria-label="How far to move them"
			on:keydown={(e) => e.key === 'Enter' && applyShift()}
		/>
		<button class="btn btn-primary" on:click={applyShift} disabled={!selectedCount || saving || !shiftSecs}>
			Move
		</button>
		{#if timeNotice}
			<div style="flex-basis:100%;font-size:12.5px;color:var(--td-muted);line-height:1.45;">{timeNotice}</div>
		{/if}
	</div>
{/if}

{#if groupBy === 'session'}
	{#if mergeMode}
		<div class="panel" style="position:sticky;top:0;z-index:30;border-radius:14px;padding:12px 16px;margin-bottom:16px;display:flex;align-items:center;gap:12px;flex-wrap:wrap;background:var(--td-surface2);">
			<span style="font-weight:600;font-size:14px;">{mergeCount} selected</span>
			<span style="font-size:12.5px;color:var(--td-muted-2);">
				They fold into the earliest one, keeping its identity.
			</span>
			<span style="flex:1;"></span>
			<button class="btn btn-primary" on:click={applyMerge} disabled={mergeCount < 2 || merging}>
				{merging ? 'Merging…' : 'Merge selected'}
			</button>
		</div>
	{/if}

	{#if sessionsLoading && !sessions.length}
		<div style="display:flex;justify-content:center;padding:60px;"><div class="spin" style="width:22px;height:22px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
	{:else if sessions.length}
		{#each sessions as sess (sess.id)}
			<SessionCard
				session={sess}
				{chains}
				{variants}
				{peers}
				selectable={mergeMode}
				selected={mergePick.has(sess.id)}
				on:toggle={(e) => toggleMergePick(e.detail)}
				on:changed={loadSessions}
			/>
		{/each}
		<Pager page={sessPage} size={SESS_PAGE} total={sessTotal} noun="sessions" busy={sessionsLoading} on:go={(e) => goSessPage(e.detail)} />
	{:else}
		<div class="panel" style="border-radius:22px;padding:44px;text-align:center;color:var(--td-muted);">
			No sessions yet — they're grouped from your listens as they arrive.
		</div>
	{/if}
{:else if groups.length}
	{#each groups as g}
		<div style="display:flex;align-items:center;gap:12px;margin:22px 0 10px;">
			<span style="font-size:12px;letter-spacing:.12em;text-transform:uppercase;color:var(--td-muted-2);font-weight:600;">{g.label}</span>
			<span style="flex:1;height:1px;background:var(--td-border);"></span>
			<span style="font-size:12px;color:var(--td-muted-3);">{g.items.length} plays</span>
		</div>
		<div class="panel" style="border-radius:22px;overflow:hidden;">
			{#each g.items as s (s.id)}
				<ScrobbleRow scrobble={s} chains={chainsById} showMenu selectable={selectMode} selected={selected.has(s.id)}
					on:menu={(e) => openMenu(e.detail)} on:toggle={(e) => toggleSelect(e.detail)} />
			{/each}
		</div>
	{/each}
	<Pager {page} size={PAGE} {total} busy={loading} on:go={(e) => goPage(e.detail)} />
{:else if loading}
	<div style="display:flex;justify-content:center;padding:60px;"><div class="spin" style="width:22px;height:22px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
{:else if filter}
	<div class="panel" style="border-radius:22px;padding:44px;text-align:center;color:var(--td-muted);">No listens matching “{filter}”.</div>
{:else}
	<div class="panel" style="border-radius:22px;padding:48px 44px;text-align:center;">
		<div class="h-display" style="font-size:20px;margin-bottom:6px;">The tape is blank</div>
		<div style="color:var(--td-muted);font-size:14px;max-width:340px;margin:0 auto 18px;">No listens recorded yet — import a backup or start scrobbling from your player.</div>
		<a class="btn btn-primary" href="/import" style="text-decoration:none;">Import listens</a>
	</div>
{/if}

<!-- Row menu -->
<svelte:window on:scroll={() => (menuFor = null)} on:resize={() => (menuFor = null)} />
{#if menuFor}
	<div on:click={() => (menuFor = null)} on:keydown={() => {}} role="presentation" style="position:fixed;inset:0;z-index:65;"></div>
	<div style="position:fixed;z-index:66;{menuPos}background:var(--td-surface);border:1px solid var(--td-border);border-radius:14px;box-shadow:0 12px 32px rgba(46,43,37,.2);padding:6px;min-width:180px;display:flex;flex-direction:column;">
		<!-- The bare heart on the row means the track; album and artist are
		     explicit here, where there's room to say which. -->
		{#each [{ k: 'album', label: 'album' }, { k: 'artist', label: 'artist' }] as t}
			{@const isOn = !!menuFor && isLoved($lovedKeys, menuFor, t.k as 'album' | 'artist')}
			<!-- A listen with no album has no album to love. Previously implied by
			     `loveKey` returning null; said outright now. -->
			{#if t.k === 'artist' || menuFor?.album}
				<button class="menu-item" on:click={() => { if (menuFor) toggleLove(menuFor, t.k as 'album' | 'artist'); menuFor = null; }}>
					<svg viewBox="0 0 24 24" width="16" fill={isOn ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1-1.1a5.5 5.5 0 0 0-7.8 7.8l1.1 1L12 21l7.7-7.6 1.1-1a5.5 5.5 0 0 0 0-7.8z"/></svg>
					{isOn ? `Un-love ${t.label}` : `Love ${t.label}`}
				</button>
			{/if}
		{/each}
		<!-- The reader was reachable from a resolved [[line:…]] chip and nowhere
		     else — and to write one you need a line number, which you get by
		     opening the reader. A closed loop, which is the shipped-switched-off
		     pattern with an extra step. This is the way in. -->
		{#if menuFor && hasWords(menuFor)}
			<button class="menu-item" on:click={() => menuFor && readWords(menuFor)}>
				<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6h10M4 12h16M4 18h7"/></svg>Read the words</button>
		{/if}
		<button class="menu-item" on:click={() => menuFor && openEdit(menuFor)}>
			<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/></svg>Edit details</button>
		<button class="menu-item" on:click={() => { inspecting = menuFor; menuFor = null; }}>
			<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M9 8l-5 4 5 4"/><path d="M15 8l5 4-5 4"/></svg>Inspect listen</button>
		<button class="menu-item danger" on:click={() => { confirmDelete = menuFor; menuFor = null; }}>
			<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16"/><path d="M10 11v6M14 11v6"/><path d="M6 7l1 13h10l1-13"/><path d="M9 7V4h6v3"/></svg>Delete listen</button>
	</div>
{/if}

{#if inspecting}
	<InspectListen scrobble={inspecting} on:close={() => (inspecting = null)} />
{/if}

<ToTop />

<!-- Edit dialog -->
{#if editing}
	<div class="dialog-backdrop" on:click={() => (editing = null)} on:keydown={() => {}} role="presentation">
		<div class="dialog" style="width:min(540px,100%);" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">Edit listen</div>
			<div class="dialog-body">
				<div style="display:flex;flex-direction:column;gap:13px;">
					<div style="display:flex;gap:12px;flex-wrap:wrap;">
						<div class="field" style="flex:1;min-width:200px;"><label for="et">Title</label><input id="et" class="input" bind:value={edit.title}></div>
						<div class="field" style="flex:1;min-width:200px;"><label for="ea">Artist</label><input id="ea" class="input" bind:value={edit.artist}></div>
					</div>
					<div class="field"><label for="eal">Album</label><input id="eal" class="input" bind:value={edit.album}></div>
					<div class="field"><label for="em">Recording MBID</label><input id="em" class="input" bind:value={edit.mbid_recording} placeholder="e.g. b9ad642e-b012-…"></div>
					<div style="display:flex;gap:12px;flex-wrap:wrap;">
						<div class="field" style="flex:1;min-width:180px;"><label for="ec">Signal chain</label>
							<select id="ec" class="input" bind:value={edit.signal_chain_id}>
								<option value={0}>— unchanged —</option>
								{#each chains as c}<option value={c.id}>{c.name}</option>{/each}
							</select>
						</div>
						{#if editVariants.length}
							<!-- Only appears once the chosen chain has setups. Two
							     cartridges on one turntable are the same path with one
							     step swapped, and this is what makes the hours land on
							     the one that was actually fitted. -->
							<div class="field" style="flex:1;min-width:180px;"><label for="ev">Setup</label>
								<select id="ev" class="input" bind:value={edit.chain_variant_id}>
									<option value={0}>— as the chain is —</option>
									{#each editVariants as v}<option value={v.id}>{v.name}</option>{/each}
								</select>
							</div>
						{/if}
						<div class="field" style="flex:1;min-width:180px;"><label for="ef">Format label</label><input id="ef" class="input" bind:value={edit.format_type} placeholder="e.g. flac, dsd, aac"></div>
					</div>
				</div>
			</div>
			<div class="dialog-actions" style="justify-content:space-between;">
				<button class="btn btn-ghost" style="color:#b2622d;" on:click={() => { confirmDelete = editing; editing = null; }}>Delete</button>
				<div style="display:flex;gap:8px;">
					<button class="btn btn-secondary" on:click={() => (editing = null)}>Cancel</button>
					<button class="btn btn-primary" on:click={saveEdit} disabled={saving}>{saving ? 'Saving…' : 'Save changes'}</button>
				</div>
			</div>
		</div>
	</div>
{/if}

<!-- Confirm delete -->
{#if confirmDelete}
	<div class="dialog-backdrop" on:click={() => (confirmDelete = null)} on:keydown={() => {}} role="presentation">
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">Delete this listen?</div>
			<div class="dialog-body" style="color:var(--td-nav-fg);">“{confirmDelete.title}” by {confirmDelete.artist} will be permanently removed.</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (confirmDelete = null)}>Cancel</button>
				<button class="btn btn-primary" style="background:#b2622d;" on:click={() => confirmDelete && doDelete(confirmDelete)}>Delete</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.spin { animation: td-spin 0.7s linear infinite; }
	.menu-item {
		display: flex; align-items: center; gap: 10px; width: 100%;
		border: none; background: transparent; cursor: pointer; text-align: left;
		padding: 9px 12px; border-radius: 10px; font-size: 13.5px; color: var(--td-text);
	}
	.menu-item:hover { background: var(--td-nav-hover); }
	.menu-item.danger { color: #b2622d; }
	.seg-btn {
		border: none; background: transparent; cursor: pointer; color: var(--td-nav-fg);
		font-size: 12.5px; padding: 6px 14px; border-radius: 999px; font-family: inherit;
	}
	.seg-btn.on { background: var(--td-accent); color: #fbf5ea; }
</style>
