<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { Scrobble, SignalChain } from '$lib/api';
	import ScrobbleRow from '$lib/components/ScrobbleRow.svelte';

	let scrobbles: Scrobble[] = [];
	let chains: SignalChain[] = [];
	let filter = '';
	let loading = true;
	let offset = 0;
	let hasMore = true;
	const PAGE = 50;

	// row menu + dialogs
	let menuFor: Scrobble | null = null;
	let editing: Scrobble | null = null;
	let edit = { title: '', artist: '', album: '', mbid_recording: '', signal_chain_id: 0, format_type: '' };
	let confirmDelete: Scrobble | null = null;
	let saving = false;

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
			const res = await api.getScrobbles({ limit: PAGE, offset, artist: filter || undefined });
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
		try { chains = (await api.getChains()).chains; } catch { /* optional */ }
		load(true);
	});
</script>

<svelte:head><title>History — Tapedeck</title></svelte:head>

<div style="display:flex;align-items:flex-end;justify-content:space-between;gap:16px;margin-bottom:24px;flex-wrap:wrap;">
	<div>
		<h1 class="h-display" style="font-size:38px;">History</h1>
		<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;">Every side you've ever pressed play on.</p>
	</div>
	<input class="input" style="width:230px;" bind:value={filter} on:input={onFilter} placeholder="Filter by artist…">
</div>

{#if groups.length}
	{#each groups as g}
		<div style="display:flex;align-items:center;gap:12px;margin:22px 0 10px;">
			<span style="font-size:12px;letter-spacing:.12em;text-transform:uppercase;color:var(--td-muted-2);font-weight:600;">{g.label}</span>
			<span style="flex:1;height:1px;background:var(--td-border);"></span>
			<span style="font-size:12px;color:var(--td-muted-3);">{g.items.length} plays</span>
		</div>
		<div class="panel" style="border-radius:22px;overflow:hidden;">
			{#each g.items as s (s.id)}
				<ScrobbleRow scrobble={s} showMenu on:menu={(e) => (menuFor = e.detail)} />
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
		<button class="menu-item" on:click={() => menuFor && openEdit(menuFor)}>
			<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/></svg>Edit details</button>
		<button class="menu-item danger" on:click={() => { confirmDelete = menuFor; menuFor = null; }}>
			<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16"/><path d="M10 11v6M14 11v6"/><path d="M6 7l1 13h10l1-13"/><path d="M9 7V4h6v3"/></svg>Delete listen</button>
	</div>
{/if}

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
</style>
