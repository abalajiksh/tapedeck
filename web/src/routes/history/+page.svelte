<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { api } from '$lib/api';
	import type { Scrobble, SignalChain , ChainVariant } from '$lib/api';
	import ScrobbleRow from '$lib/components/ScrobbleRow.svelte';
	import InspectListen from '$lib/components/InspectListen.svelte';
	import { loadLoves, lovedKeys, isLoved, toggleLove } from '$lib/loves';
	import SessionCard from '$lib/components/SessionCard.svelte';
	import ToTop from '$lib/components/ToTop.svelte';
	import type { Session } from '$lib/api';

	let scrobbles: Scrobble[] = [];
	let chains: SignalChain[] = [];
	/** Chain variants, so a sitting can name which setup it was. */
	let variants: ChainVariant[] = [];
	// Keyed by id for the row glyph — a listen carries a chain_id, not a chain.
	$: chainsById = Object.fromEntries(chains.map((c) => [c.id, { name: c.name, icon: c.icon }]));
	let filter = '';
	let loading = true;
	let offset = 0;
	let hasMore = true;
	const PAGE = 50;

	// ── Sessions ──
	// A calendar day is an arbitrary boundary for listening; a session is the
	// thing you actually did. Grouping is *stored*, not recomputed here, because
	// merges and splits are decisions no heuristic can re-derive.
	let groupBy: 'day' | 'session' = 'day';
	let sessions: Session[] = [];
	let sessionsLoading = false;
	let mergeMode = false;
	let mergePick = new Set<number>();
	let merging = false;
	$: mergeCount = mergePick.size;

	async function loadSessions() {
		sessionsLoading = true;
		try {
			sessions = (await api.getSessions(60)).sessions;
		} catch (e) { console.warn('sessions', e); }
		sessionsLoading = false;
	}

	function setGroupBy(g: 'day' | 'session') {
		groupBy = g;
		if (g === 'session' && !sessions.length) loadSessions();
		if (g === 'day') exitMerge();
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
		history.replaceState(null, '', '/history');
		load(true);
	}

	// row menu + dialogs
	let menuFor: Scrobble | null = null;
	let inspecting: Scrobble | null = null;
	let editing: Scrobble | null = null;
	let edit = { title: '', artist: '', album: '', mbid_recording: '', signal_chain_id: 0, format_type: '' };
	let confirmDelete: Scrobble | null = null;
	let saving = false;

	// bulk selection
	let selectMode = false;
	let selected = new Set<number>();
	let bulkChain = '';
	$: selectedCount = selected.size;

	function toggleSelect(s: Scrobble) {
		if (selected.has(s.id)) selected.delete(s.id);
		else selected.add(s.id);
		selected = selected; // trigger reactivity
	}
	function exitSelect() { selectMode = false; selected = new Set(); bulkChain = ''; }

	async function applyBulkChain() {
		if (!selected.size) return;
		const cid = bulkChain ? Number(bulkChain) : null;
		saving = true;
		try {
			await api.bulkSetChain([...selected], cid);
			const chain = cid; // scrobbles carry chain_id
			scrobbles = scrobbles.map((s) => (selected.has(s.id) ? { ...s, chain_id: chain } : s));
			exitSelect();
		} catch (e) { alert('Bulk update failed: ' + e); }
		saving = false;
	}

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

	async function load(reset = false) {
		if (reset) { offset = 0; scrobbles = []; hasMore = true; }
		loading = true;
		try {
			const res = await api.getScrobbles({
				limit: PAGE,
				offset,
				// The typed filter is an artist filter; a scope from search can
				// narrow by album or track instead.
				artist: filter || scope.artist || undefined,
				album: scope.album,
				track: scope.track
			});
			scrobbles = reset ? res.scrobbles : [...scrobbles, ...res.scrobbles];
			offset += res.scrobbles.length;
			hasMore = res.scrobbles.length === PAGE;
		} catch (e) { console.warn('history', e); }
		loading = false;
	}

	let filterTimer: ReturnType<typeof setTimeout>;
	function onFilter() {
		clearTimeout(filterTimer);
		filterTimer = setTimeout(() => load(true), 300);
	}

	// ── Keeping up with what's playing ──
	// This page used to load once and never again, so leaving it open through an
	// album meant watching a list that had stopped being the history. It pulls
	// the newest page back and merges, rather than reloading: a reload would
	// throw away everything paged in and put the reader back at the top.
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
		try {
			const res = await api.getScrobbles({
				limit: PAGE,
				offset: 0,
				artist: filter || scope.artist || undefined,
				album: scope.album,
				track: scope.track
			});
			const known = new Set(scrobbles.map((s) => s.id));
			const fresh = res.scrobbles.filter((s) => !known.has(s.id));
			if (!fresh.length) return;
			// Sorted rather than prepended: a listen added by hand carries its own
			// timestamp and can legitimately land in the middle of what's already
			// loaded, and groupByDay walks the list in order — an out-of-order row
			// would split its day into two headings.
			scrobbles = [...fresh, ...scrobbles].sort(
				(a, b) => b.timestamp - a.timestamp || b.id - a.id
			);
			// `offset` is a row offset into the server's own ordering, so rows that
			// appeared above the window shift it. Without this, "load more" would
			// re-serve the last `fresh.length` rows it already gave us.
			offset += fresh.length;
			if (groupBy === 'session') loadSessions();
		} catch {
			// Same call the deck poll makes: a failed refresh leaves what's on
			// screen alone rather than flickering it, and the next tick corrects.
		}
	}

	function openEdit(s: Scrobble) {
		menuFor = null;
		editing = s;
		edit = {
			title: s.title, artist: s.artist, album: s.album ?? '',
			mbid_recording: s.mbid_recording ?? '',
			signal_chain_id: 0, format_type: s.format_type ?? ''
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
			if (edit.signal_chain_id) changes.signal_chain_id = edit.signal_chain_id;
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
		loadLoves();
		try { chains = (await api.getChains()).chains; } catch { /* optional */ }
		// Optional in the same way: an instance with no variants simply shows no
		// variant picker on a session card.
		try { variants = (await api.getVariants()).variants; } catch { /* optional */ }
		await load(true);

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
			<input class="input" style="width:200px;" bind:value={filter} on:input={onFilter} placeholder="Filter by artist…">
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
		<button class="btn btn-primary" on:click={applyBulkChain} disabled={!selectedCount || saving}>{saving ? 'Applying…' : 'Apply'}</button>
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
				selectable={mergeMode}
				selected={mergePick.has(sess.id)}
				on:toggle={(e) => toggleMergePick(e.detail)}
				on:changed={loadSessions}
			/>
		{/each}
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
					on:menu={(e) => (menuFor = e.detail)} on:toggle={(e) => toggleSelect(e.detail)} />
			{/each}
		</div>
	{/each}
	{#if hasMore}
		<div style="display:flex;justify-content:center;margin-top:22px;">
			<button class="btn btn-secondary" on:click={() => load(false)} disabled={loading}>{loading ? 'Loading…' : 'Load older tapes'}</button>
		</div>
	{/if}
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
{#if menuFor}
	<div on:click={() => (menuFor = null)} on:keydown={() => {}} role="presentation" style="position:fixed;inset:0;z-index:65;"></div>
	<div style="position:fixed;z-index:66;right:40px;top:50%;transform:translateY(-50%);background:var(--td-surface);border:1px solid var(--td-border);border-radius:14px;box-shadow:0 12px 32px rgba(46,43,37,.2);padding:6px;min-width:180px;display:flex;flex-direction:column;">
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
