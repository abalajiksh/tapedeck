<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { api } from '$lib/api';
	import type { Note, NoteTarget, NoteRevision, Entity, Visibility } from '$lib/api';
	import { formatDateTime, timeAgo } from '$lib/utils';

	/**
	 * Liner notes. A note is a living document about one thing — a track, a
	 * record, an artist, or one particular playing of something — kept in
	 * Markdown and rendered by the server, which is also what resolves the
	 * [[track:…]] shortcodes.
	 */

	let notes: Note[] = [];
	let loves: { entity: Entity; loved_at: number }[] = [];
	let loading = true;
	let error = '';

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

	function entityLabel(e: Entity): { label: string; kind: string } {
		const kind = e.kind === 'recording' ? 'track' : e.kind === 'release' ? 'album' : 'artist';
		return { label: e.artist_name ? `${e.name} — ${e.artist_name}` : e.name, kind };
	}

	async function load() {
		try {
			const [n, l] = await Promise.all([api.getNotes(), api.getLoves()]);
			notes = n.notes;
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
			What you think about a record, kept with it. Markdown, and
			<code class="chip">[[track:Xtal]]</code> to point at other music.
		</p>
	</div>
	<button class="btn btn-primary" on:click={startNew}>+ New note</button>
</div>

{#if error}<div class="panel" style="border-radius:16px;padding:14px 18px;margin-bottom:16px;font-size:13.5px;color:#b2622d;">{error}</div>{/if}

<div class="notes-layout">
	<!-- Everything written, newest edit first -->
	<div>
		<div class="panel" style="border-radius:22px;padding:20px;margin-bottom:16px;">
			<h3 class="h-display" style="font-size:17px;margin:0 0 12px;">Your notes</h3>
			{#if loading}
				<div style="display:flex;justify-content:center;padding:14px;"><div class="spin" style="width:18px;height:18px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
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

		{#if loves.length}
			<div class="panel" style="border-radius:22px;padding:20px;">
				<h3 class="h-display" style="font-size:17px;margin:0 0 12px;">Loved</h3>
				<div style="display:flex;flex-direction:column;gap:4px;">
					{#each loves as l (l.entity.id)}
						{@const e = entityLabel(l.entity)}
						<button class="note-item" on:click={() => openTarget({ entity_id: l.entity.id })}>
							<div style="display:flex;align-items:baseline;gap:8px;">
								<span style="color:#b2622d;">♥</span>
								<span style="font-size:13.5px;">{e.label}</span>
								<span style="font-size:11px;color:var(--td-muted-3);">{e.kind}</span>
							</div>
						</button>
					{/each}
				</div>
			</div>
		{/if}
	</div>

	<!-- Editor -->
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
				<p style="font-size:11.5px;color:var(--td-muted-3);margin:8px 0 0;">
					Markdown. <code class="chip">[[track:Xtal]]</code>,
					<code class="chip">[[track:3]]</code> for the third track of this record,
					<code class="chip">[[album:…]]</code>, <code class="chip">[[artist:…]]</code>.
					A reference that matches nothing stays plain text.
				</p>
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
				<p style="margin:0;font-size:12.5px;color:var(--td-muted-3);">
					Notes can also be written straight from a listen — open one in History and choose
					“Liner note”.
				</p>
			</div>
		{/if}
	</div>
</div>

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

	.chip { background: var(--td-sidebar); padding: 2px 6px; border-radius: 6px; font-size: 12px; }
	.seg-btn { border: 1px solid var(--td-border); background: transparent; cursor: pointer; color: var(--td-nav-fg); font-size: 13px; padding: 7px 16px; border-radius: 999px; font-family: inherit; }
	.seg-btn.on { background: var(--td-accent); border-color: var(--td-accent); color: #fbf5ea; }
	.spin { animation: td-spin 0.7s linear infinite; }
</style>
