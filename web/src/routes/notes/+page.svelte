<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { api } from '$lib/api';
	import type { Note, NoteTarget, NoteRevision, Entity, LovedRow, Visibility } from '$lib/api';
	import { formatDateTime, timeAgo } from '$lib/utils';
	import type { NoteQuery } from '$lib/api';
	import ShortcodeLegend from '$lib/components/ShortcodeLegend.svelte';
	import ToTop from '$lib/components/ToTop.svelte';

	/**
	 * Liner notes. A note is a living document about one thing — a track, a
	 * record, an artist, or one particular playing of something — kept in
	 * Markdown and rendered by the server, which is also what resolves the
	 * [[track:…]] shortcodes.
	 */

	let notes: Note[] = [];
	let total = 0;
	let loves: LovedRow[] = [];
	let loading = true;
	let error = '';

	// ── Finding a note again ──
	//
	// A notes collection is a thing that grows: at twenty a flat list is fine and
	// at two hundred it is a wall, and the whole point of writing these is being
	// able to come back to them. Filtering is server-side (see
	// `Database::search_notes`) rather than over an array here, because rendering
	// one note costs four more queries and narrowing first is the difference
	// between instant and the slowest screen in the app.
	let q = '';
	let kind: NoteQuery['kind'] = '';
	let sort: NonNullable<NoteQuery['sort']> = 'updated';
	let searchTimer: ReturnType<typeof setTimeout> | undefined;

	const KINDS: { value: NoteQuery['kind']; label: string }[] = [
		{ value: '', label: 'All' },
		{ value: 'recording', label: 'Tracks' },
		{ value: 'release', label: 'Records' },
		{ value: 'artist', label: 'Artists' },
		{ value: 'listen', label: 'Listens' }
	];

	/** Debounced, so typing a word is one request rather than one per letter. */
	function onSearch() {
		clearTimeout(searchTimer);
		searchTimer = setTimeout(load, 220);
	}

	function setKind(k: NoteQuery['kind']) {
		kind = k;
		load();
	}

	function clearFilters() {
		q = '';
		kind = '';
		load();
	}

	$: filtering = q.trim().length > 0 || kind !== '';

	// ── Loved ──
	//
	// This rendered the whole set flat, so at a few hundred loves the panel ran
	// several times the height of everything beside it and the one you wanted
	// was somewhere in the middle of it. It is a digest now: eight rows, a box
	// to find the rest, and "See all" expanding into a list that scrolls
	// *inside* the panel — the same handoff the Statistics top lists use, so
	// the page never has to be left to answer "did I love this?".
	//
	// Filtering is client-side, unlike the notes search above. That is not an
	// inconsistency: a note costs four more queries to render so narrowing has
	// to happen in SQL, while loves are a few hundred rows already in hand.
	// Same call `/loved` makes.
	let loveQ = '';
	let lovesOpen = false;
	const LOVE_PREVIEW = 8;

	$: lovesFound = (() => {
		const needle = loveQ.trim().toLowerCase();
		if (!needle) return loves;
		return loves.filter((l) =>
			`${l.entity.name} ${l.entity.artist_name ?? ''}`.toLowerCase().includes(needle)
		);
	})();
	/** Searching implies expanded — hiding matches behind "See all" would make
	 *  the search look broken for anything past the eighth hit. */
	$: lovesListing = lovesOpen || loveQ.trim() ? lovesFound : lovesFound.slice(0, LOVE_PREVIEW);
	$: lovesScrolls = lovesListing.length > LOVE_PREVIEW;

	// ── Editor ──
	let open: Note | null = null;            // the note being edited, if it exists
	let target: NoteTarget | null = null;    // what it's about
	let targetLabel = '';
	let targetKind = '';
	let body = '';
	let visibility: Visibility = 'private';
	let loved = false;
	let saving = false;
	let dirty = false;

	let previewHtml = '';
	let previewing = false;
	let previewBusy = false;

	let revisions: NoteRevision[] = [];
	let showRevisions = false;
	let confirmDelete = false;

	// ── New-note form ──
	let composing = false;
	let newKind: 'track' | 'album' | 'artist' = 'album';
	let newName = '';
	let newArtist = '';

	const clean = (e: unknown) => String(e).replace(/^API \d+: /, '');

	function labelFor(note: Note): { label: string; kind: string } {
		if (note.entity) return entityLabel(note.entity);
		if (note.listen) {
			return {
				label: `${note.listen.title} — ${note.listen.artist}`,
				kind: `listen · ${formatDateTime(note.listen.timestamp)}`
			};
		}
		return { label: 'Untitled', kind: '' };
	}

	// Takes only what it reads, so it works for both a full `Entity` and the
	// narrower one a loves listing carries.
	function entityLabel(e: Pick<Entity, 'kind' | 'name' | 'artist_name'>): { label: string; kind: string } {
		const kind = e.kind === 'recording' ? 'track' : e.kind === 'release' ? 'album' : 'artist';
		return { label: e.artist_name ? `${e.name} — ${e.artist_name}` : e.name, kind };
	}

	async function load() {
		try {
			const [n, l] = await Promise.all([api.getNotes({ q, kind, sort }), api.getLoves()]);
			notes = n.notes;
			total = n.total;
			loves = l.loves;
		} catch (e) {
			error = clean(e);
		}
		loading = false;
	}

	/** Open the editor on a target, pulling in the note already there (if any). */
	async function openTarget(t: NoteTarget) {
		error = '';
		previewing = false;
		showRevisions = false;
		revisions = [];
		try {
			const r = await api.noteFor(t);
			open = r.note;
			loved = r.loved;
			body = r.note?.body_md ?? '';
			visibility = r.note?.visibility ?? 'private';
			dirty = false;
			const entity = r.target.entity ?? r.note?.entity ?? null;
			const listen = r.target.listen ?? r.note?.listen ?? null;
			if (entity) {
				({ label: targetLabel, kind: targetKind } = entityLabel(entity));
				target = { entity_id: entity.id };
			} else if (listen) {
				targetLabel = `${listen.title} — ${listen.artist}`;
				targetKind = `listen · ${formatDateTime(listen.timestamp)}`;
				target = { scrobble_id: listen.id };
			} else {
				target = t;
			}
			composing = false;
			// Keep the address bar honest, so an open note can be linked to and
			// survives a reload. `target` is already set, so the reactive sync
			// above sees no change and doesn't re-fetch.
			const canonical = target.entity_id
				? `/notes?entity=${target.entity_id}`
				: target.scrobble_id ? `/notes?listen=${target.scrobble_id}` : '/notes';
			if ($page.url.pathname + $page.url.search !== canonical) {
				goto(canonical, { replaceState: true, noScroll: true, keepFocus: true });
			}
		} catch (e) {
			error = clean(e);
		}
	}

	function openNote(note: Note) {
		openTarget(note.entity ? { entity_id: note.entity.id } : { scrobble_id: note.listen!.id });
	}

	function closeEditor() {
		open = null;
		target = null;
		body = '';
		previewing = false;
		dirty = false;
		goto('/notes', { replaceState: true, noScroll: true, keepFocus: true });
	}

	async function save() {
		if (!target || !body.trim()) return;
		saving = true;
		error = '';
		try {
			const saved = await api.saveNote({ ...target, body_md: body, visibility });
			open = saved;
			dirty = false;
			if (previewing) previewHtml = saved.body_html ?? '';
			await load();
		} catch (e) {
			error = clean(e);
		}
		saving = false;
	}

	async function togglePreview() {
		previewing = !previewing;
		if (!previewing || !target) return;
		previewBusy = true;
		try {
			previewHtml = (await api.previewNote({ ...target, body_md: body })).body_html;
		} catch (e) {
			error = clean(e);
			previewing = false;
		}
		previewBusy = false;
	}

	async function toggleLove() {
		if (!target?.entity_id) return;
		try {
			const r = await api.setLove({ entity_id: target.entity_id }, !loved);
			loved = r.loved;
			loves = (await api.getLoves()).loves;
		} catch (e) {
			error = clean(e);
		}
	}

	async function loadRevisions() {
		showRevisions = !showRevisions;
		if (!showRevisions || !open) return;
		try {
			revisions = (await api.noteRevisions(open.id)).revisions;
		} catch (e) {
			error = clean(e);
		}
	}

	/** Bring an old version back into the editor. Saving it archives the current
	 *  body in turn, so nothing is lost either way. */
	function restore(rev: NoteRevision) {
		body = rev.body_md;
		dirty = true;
		showRevisions = false;
	}

	async function doDelete() {
		if (!open) return;
		try {
			await api.deleteNote(open.id);
			confirmDelete = false;
			closeEditor();
			await load();
		} catch (e) {
			error = clean(e);
		}
	}

	function startNew() {
		composing = true;
		open = null;
		target = null;
		body = '';
		newName = '';
		newArtist = '';
	}

	async function createFromForm() {
		if (!newName.trim()) return;
		await openTarget({
			kind: newKind,
			name: newName.trim(),
			...(newKind === 'artist' ? {} : { artist: newArtist.trim() })
		});
	}

	// `?entity=` / `?listen=` drives which note is open. Following it reactively
	// rather than only on mount matters because a link *inside* a rendered note
	// points back at this same page — the router swaps the query without
	// remounting, so a mount-only read would leave the old note on screen.
	let ready = false;
	$: if (ready) syncFromUrl($page.url.searchParams.get('entity'), $page.url.searchParams.get('listen'));

	async function syncFromUrl(entity: string | null, listen: string | null) {
		if (entity && target?.entity_id !== Number(entity)) await openTarget({ entity_id: Number(entity) });
		else if (listen && target?.scrobble_id !== Number(listen)) await openTarget({ scrobble_id: Number(listen) });
	}

	onMount(async () => {
		await load();
		ready = true;
	});
</script>

<svelte:head><title>Liner notes — Tapedeck</title></svelte:head>

<div style="display:flex;align-items:flex-start;justify-content:space-between;gap:16px;flex-wrap:wrap;margin-bottom:24px;">
	<div>
		<h1 class="h-display" style="font-size:38px;margin-bottom:6px;">Liner notes</h1>
		<p style="margin:0;color:var(--td-muted);font-size:15px;">
			What you think about a record, kept with it.
		</p>
	</div>
	<div style="display:flex;gap:8px;align-items:center;">
		<a class="btn btn-secondary" href="/settings#data" title="Export every note as Markdown">
			Export
		</a>
		<button class="btn btn-primary" on:click={startNew}>+ New note</button>
	</div>
</div>

{#if error}<div class="panel" style="border-radius:16px;padding:14px 18px;margin-bottom:16px;font-size:13.5px;color:#b2622d;">{error}</div>{/if}

<div class="notes-layout">
	<!-- Everything written, newest edit first -->
	<div>
		<div class="panel" style="border-radius:22px;padding:20px;margin-bottom:16px;">
			<div style="display:flex;align-items:baseline;justify-content:space-between;gap:8px;margin-bottom:10px;">
				<h3 class="h-display" style="font-size:17px;margin:0;">Your notes</h3>
				<span style="font-size:11.5px;color:var(--td-muted-3);">
					{filtering ? `${notes.length} of ${total}` : `${total || notes.length}`}
				</span>
			</div>

			<div class="search">
				<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor"
					stroke-width="1.6" stroke-linecap="round" aria-hidden="true">
					<circle cx="7" cy="7" r="4.5" /><path d="m10.5 10.5 3 3" />
				</svg>
				<input class="input" type="search" bind:value={q} on:input={onSearch}
					placeholder="Search your notes" aria-label="Search your notes" />
			</div>

			<div class="filters">
				{#each KINDS as k}
					<button class="fchip" class:on={kind === k.value} on:click={() => setKind(k.value)}>
						{k.label}
					</button>
				{/each}
				<span style="flex:1;"></span>
				<select class="input sortsel" bind:value={sort} on:change={load} aria-label="Sort notes">
					<option value="updated">Recently edited</option>
					<option value="created">Recently written</option>
					<option value="title">A–Z</option>
				</select>
			</div>

			{#if loading}
				<div style="display:flex;justify-content:center;padding:14px;"><div class="spin" style="width:18px;height:18px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
			{:else if !notes.length && filtering}
				<p style="font-size:13px;color:var(--td-muted);margin:0;">
					Nothing matches that.
					<button class="linkish" on:click={clearFilters}>Clear the search</button>
				</p>
			{:else if !notes.length}
				<p style="font-size:13px;color:var(--td-muted);margin:0;">
					Nothing yet. Write one here, or open any listen in History and choose “Liner note”.
				</p>
			{:else}
				<div style="display:flex;flex-direction:column;gap:6px;">
					{#each notes as n (n.id)}
						{@const l = labelFor(n)}
						<button class="note-item" class:on={open?.id === n.id} on:click={() => openNote(n)}>
							<div style="display:flex;align-items:baseline;gap:8px;">
								<span style="font-size:13.5px;font-weight:600;">{l.label}</span>
								<span style="font-size:11px;color:var(--td-muted-3);">{l.kind}</span>
							</div>
							<div style="font-size:12px;color:var(--td-muted-2);margin-top:2px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">{n.excerpt}</div>
							<div style="font-size:11px;color:var(--td-muted-3);margin-top:3px;">
								{timeAgo(n.updated_at)}{n.revisions ? ` · ${n.revisions} earlier version${n.revisions > 1 ? 's' : ''}` : ''}{n.visibility !== 'private' ? ` · ${n.visibility}` : ''}
							</div>
						</button>
					{/each}
				</div>
			{/if}
		</div>

	</div>

	<!-- Editor, and under it whatever the column has room for. -->
	<div class="rightcol">
	<div class="panel" style="border-radius:22px;padding:24px;">
		{#if composing && !target}
			<h3 class="h-display" style="font-size:18px;margin:0 0 4px;">What is this about?</h3>
			<p style="font-size:12.5px;color:var(--td-muted);margin:0 0 16px;">
				Type the name as you'd recognise it. If you've played it, the note links up with your
				history automatically.
			</p>
			<div style="display:flex;gap:6px;margin-bottom:14px;">
				{#each ['track', 'album', 'artist'] as const as k}
					<button class="seg-btn" class:on={newKind === k} on:click={() => (newKind = k)}>{k}</button>
				{/each}
			</div>
			<div style="display:flex;gap:12px;flex-wrap:wrap;">
				<div class="field" style="flex:2;min-width:200px;">
					<label for="n-name">{newKind === 'artist' ? 'Artist' : newKind === 'album' ? 'Album' : 'Track'}</label>
					<input id="n-name" class="input" bind:value={newName} placeholder={newKind === 'artist' ? 'Aphex Twin' : 'Selected Ambient Works 85-92'}>
				</div>
				{#if newKind !== 'artist'}
					<div class="field" style="flex:1;min-width:160px;">
						<label for="n-artist">Artist</label>
						<input id="n-artist" class="input" bind:value={newArtist} placeholder="Aphex Twin">
					</div>
				{/if}
			</div>
			<div style="display:flex;gap:10px;margin-top:16px;">
				<button class="btn btn-primary" on:click={createFromForm} disabled={!newName.trim()}>Start writing</button>
				<button class="btn btn-secondary" on:click={() => (composing = false)}>Cancel</button>
			</div>
		{:else if target}
			<div style="display:flex;align-items:flex-start;justify-content:space-between;gap:12px;flex-wrap:wrap;margin-bottom:14px;">
				<div style="min-width:0;">
					<h3 class="h-display" style="font-size:18px;margin:0 0 2px;">{targetLabel}</h3>
					<div style="font-size:11.5px;color:var(--td-muted-3);text-transform:uppercase;letter-spacing:.08em;">{targetKind}</div>
				</div>
				<div style="display:flex;gap:8px;align-items:center;">
					{#if target.entity_id}
						<button class="btn btn-secondary" style="color:{loved ? '#b2622d' : 'inherit'};" on:click={toggleLove}>
							{loved ? '♥ Loved' : '♡ Love'}
						</button>
					{/if}
					<button class="btn btn-secondary" on:click={closeEditor}>Close</button>
				</div>
			</div>

			{#if previewing}
				<div class="note-body">
					{#if previewBusy}
						<p style="color:var(--td-muted);">Rendering…</p>
					{:else}
						<!-- Rendered and sanitised server-side; this is the only place a
						     note becomes HTML. -->
						{@html previewHtml}
					{/if}
				</div>
			{:else}
				<textarea
					class="input note-input"
					bind:value={body}
					on:input={() => (dirty = true)}
					placeholder="Bought this on a whim in 2011. [[track:3]] is the one that got me — nothing else on it sounds like [[artist:Boards of Canada]]."
				></textarea>
				<div style="margin-top:10px;"><ShortcodeLegend /></div>
			{/if}

			<div style="display:flex;align-items:center;gap:10px;flex-wrap:wrap;margin-top:16px;">
				<button class="btn btn-primary" on:click={save} disabled={saving || !body.trim()}>
					{saving ? '…' : dirty || !open ? 'Save' : 'Saved'}
				</button>
				<button class="btn btn-secondary" on:click={togglePreview} disabled={!body.trim()}>
					{previewing ? 'Edit' : 'Preview'}
				</button>
				<select class="input" style="width:auto;padding:8px 12px;" bind:value={visibility}>
					<option value="private">Private</option>
					<option value="patched">Patched-in</option>
					<option value="public">Public</option>
				</select>
				<span style="flex:1;"></span>
				{#if open}
					{#if open.revisions}
						<button class="btn btn-ghost" style="font-size:13px;" on:click={loadRevisions}>
							{showRevisions ? 'Hide' : `${open.revisions} earlier version${open.revisions > 1 ? 's' : ''}`}
						</button>
					{/if}
					<button class="btn btn-ghost" style="color:#b2622d;font-size:13px;" on:click={() => (confirmDelete = true)}>Delete</button>
				{/if}
			</div>

			<p style="font-size:11.5px;color:var(--td-muted-3);margin:10px 0 0;">
				Visibility is stored for later — patches and federation aren't built yet, so for now
				every note is yours alone whatever this says.
			</p>

			{#if showRevisions}
				<div style="margin-top:16px;padding-top:16px;border-top:1px solid var(--td-border);">
					{#if !revisions.length}
						<p style="font-size:13px;color:var(--td-muted);margin:0;">No earlier versions.</p>
					{:else}
						<div style="display:flex;flex-direction:column;gap:8px;">
							{#each revisions as r (r.id)}
								<div style="display:flex;align-items:center;gap:12px;padding:10px 14px;border:1px solid var(--td-border);border-radius:14px;background:var(--td-surface2);">
									<div style="flex:1;min-width:0;">
										<div style="font-size:12.5px;color:var(--td-muted-2);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">{r.excerpt}</div>
										<div style="font-size:11px;color:var(--td-muted-3);margin-top:2px;">{formatDateTime(r.created_at)}</div>
									</div>
									<button class="btn btn-secondary" style="font-size:12.5px;padding:6px 14px;" on:click={() => restore(r)}>Restore</button>
								</div>
							{/each}
						</div>
					{/if}
				</div>
			{/if}
		{:else}
			<div style="text-align:center;padding:40px 12px;color:var(--td-muted);">
				<div style="font-size:34px;margin-bottom:10px;">✍</div>
				<p style="margin:0 0 4px;font-size:14px;">Pick a note on the left, or start a new one.</p>
				<p style="margin:0 0 20px;font-size:12.5px;color:var(--td-muted-3);">
					Notes can also be written straight from a listen — open one in History and choose
					“Liner note”.
				</p>
				<div style="text-align:left;max-width:520px;margin:0 auto;">
					<ShortcodeLegend open />
				</div>
			</div>
		{/if}
	</div>

	<!-- Loved lives here rather than under the notes list: this column ended
	     early and left a column of empty page beside a list running off the
	     bottom of it. When a note is open the editor takes the space above and
	     this drops below it, which is the right order — in view when you are
	     looking for something, out of the way when you are writing. -->
	{#if loves.length}
		<div class="panel" style="border-radius:22px;padding:20px;">
			<div style="display:flex;align-items:baseline;justify-content:space-between;gap:8px;margin-bottom:10px;">
				<h3 class="h-display" style="font-size:17px;margin:0;">Loved</h3>
				<a class="linkish" style="font-size:11.5px;" href="/loved">Open the shelf →</a>
			</div>

			<div class="search">
				<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor"
					stroke-width="1.6" stroke-linecap="round" aria-hidden="true">
					<circle cx="7" cy="7" r="4.5" /><path d="m10.5 10.5 3 3" />
				</svg>
				<input class="input" type="search" bind:value={loveQ}
					placeholder="Find something you loved" aria-label="Search loved" />
			</div>

			{#if !lovesFound.length}
				<p style="font-size:13px;color:var(--td-muted);margin:0;">
					Nothing loved matches that.
					<button class="linkish" on:click={() => (loveQ = '')}>Clear</button>
				</p>
			{:else}
				<div class="loves-list" class:scrolls={lovesScrolls}>
					{#each lovesListing as l (l.entity.id)}
						{@const e = entityLabel(l.entity)}
						<button class="note-item" on:click={() => openTarget({ entity_id: l.entity.id })}>
							<div style="display:flex;align-items:baseline;gap:8px;">
								<span style="color:#b2622d;">♥</span>
								<span style="font-size:13.5px;flex:1;min-width:0;">{e.label}</span>
								<span style="font-size:11px;color:var(--td-muted-3);flex:none;">{e.kind}</span>
							</div>
						</button>
					{/each}
				</div>

				<div style="margin-top:10px;font-size:11.5px;color:var(--td-muted-3);">
					{#if loveQ.trim()}
						{lovesFound.length} of {loves.length}
					{:else if lovesOpen}
						All {loves.length}.
						<button class="linkish" on:click={() => (lovesOpen = false)}>Show fewer</button>
					{:else if loves.length > LOVE_PREVIEW}
						{LOVE_PREVIEW} of {loves.length}.
						<button class="linkish" on:click={() => (lovesOpen = true)}>See all</button>
					{:else}
						{loves.length} loved
					{/if}
				</div>
			{/if}
		</div>
	{/if}
	</div>
</div>

<ToTop />

{#if confirmDelete}
	<div class="dialog-backdrop" on:click={() => (confirmDelete = false)} on:keydown={() => {}} role="presentation">
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">Delete this note?</div>
			<div class="dialog-body" style="color:var(--td-nav-fg);">
				The note and every earlier version of it go. This can't be undone.
			</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (confirmDelete = false)}>Cancel</button>
				<button class="btn btn-primary" style="background:#b2622d;" on:click={doDelete}>Delete</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.notes-layout {
		display: grid;
		grid-template-columns: minmax(0, 320px) minmax(0, 1fr);
		gap: 18px;
		align-items: start;
	}
	@media (max-width: 860px) {
		.notes-layout { grid-template-columns: minmax(0, 1fr); }
	}

	/* The editor and Loved stack in the second column. */
	.rightcol {
		display: flex;
		flex-direction: column;
		gap: 16px;
		min-width: 0;
	}

	/* Past the preview the list scrolls inside its own panel rather than
	   pushing the page down — the same containment the Statistics top lists
	   use when "See all" is open. */
	.loves-list {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.loves-list.scrolls {
		max-height: 340px;
		overflow-y: auto;
		padding-right: 4px;
	}

	.note-item {
		display: block;
		width: 100%;
		text-align: left;
		border: 1px solid transparent;
		background: transparent;
		font: inherit;
		cursor: pointer;
		padding: 9px 12px;
		border-radius: 14px;
		color: var(--td-text);
	}
	.note-item:hover { background: var(--td-nav-hover); }
	.note-item.on { background: var(--td-surface2); border-color: var(--td-border); }

	.note-input {
		width: 100%;
		min-height: 260px;
		resize: vertical;
		line-height: 1.6;
		font-family: inherit;
	}

	.search { position: relative; margin-bottom: 9px; }
	.search svg {
		position: absolute;
		left: 12px;
		top: 50%;
		transform: translateY(-50%);
		color: var(--td-muted-3);
		pointer-events: none;
	}
	.search :global(input) { padding-left: 32px; }

	.filters { display: flex; gap: 5px; align-items: center; flex-wrap: wrap; margin-bottom: 12px; }

	/* `.chip` on this page is the inline code style, so the filters get their own
	   class rather than borrowing a name that already means something else. */
	.fchip {
		border: 1px solid var(--td-border);
		background: transparent;
		cursor: pointer;
		color: var(--td-muted-2);
		font: inherit;
		font-size: 12px;
		padding: 4px 11px;
		border-radius: 999px;
	}
	.fchip:hover { color: var(--td-text); }
	.fchip.on { background: var(--td-accent); border-color: var(--td-accent); color: #fbf5ea; }

	.sortsel { width: auto; padding: 4px 8px; font-size: 12px; border-radius: 999px; }

	.linkish {
		border: none;
		background: none;
		padding: 0;
		font: inherit;
		color: var(--td-accent);
		cursor: pointer;
		text-decoration: underline;
	}

	.seg-btn { border: 1px solid var(--td-border); background: transparent; cursor: pointer; color: var(--td-nav-fg); font-size: 13px; padding: 7px 16px; border-radius: 999px; font-family: inherit; }
	.seg-btn.on { background: var(--td-accent); border-color: var(--td-accent); color: #fbf5ea; }
	.spin { animation: td-spin 0.7s linear infinite; }
</style>
