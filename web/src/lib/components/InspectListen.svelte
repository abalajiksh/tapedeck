<script lang="ts">
	import { api, type Scrobble, type ListenAnnotations } from '$lib/api';
	import { createEventDispatcher, onMount } from 'svelte';

	export let scrobble: Scrobble;

	const dispatch = createEventDispatcher<{ close: void; updated: Scrobble }>();

	// Liner notes for this listen and for the things it's *of*. Read-only: the
	// entities behind track/album/artist are only created when you write or love
	// something, so inspecting a row costs nothing.
	let ann: ListenAnnotations | null = null;
	let loveBusy = false;

	onMount(async () => {
		try { ann = await api.listenAnnotations(scrobble.id); } catch { /* notes are optional */ }
	});

	const noteTargets = ['track', 'album', 'artist'] as const;

	/** Open the notes page on a target, resolving (and creating) its entity. */
	async function writeAbout(kind: (typeof noteTargets)[number]) {
		const existing = ann?.[kind]?.entity_id;
		if (existing) {
			window.location.href = `/notes?entity=${existing}`;
			return;
		}
		try {
			const r = await api.noteFor({ from_scrobble: scrobble.id, kind });
			const id = r.target.entity?.id;
			window.location.href = id ? `/notes?entity=${id}` : '/notes';
		} catch {
			window.location.href = '/notes';
		}
	}

	async function toggleLove(kind: (typeof noteTargets)[number]) {
		if (loveBusy) return;
		loveBusy = true;
		try {
			const current = ann?.[kind]?.loved ?? false;
			await api.setLove({ from_scrobble: scrobble.id, kind }, !current);
			ann = await api.listenAnnotations(scrobble.id);
		} catch { /* leave the state as it was */ }
		loveBusy = false;
	}

	// Everything the row carries, including the fields the UI never renders
	// (source_id, delivered_sinks, session/device ids…). The list endpoint
	// already returns the whole stored row, so nothing here is a second guess
	// at what the database holds — it *is* what it holds.
	$: pretty = JSON.stringify(scrobble, null, 2);

	// MBIDs are the thing you actually want to click through on when a listen
	// looks mismatched.
	$: links = [
		['Recording', scrobble.mbid_recording, 'recording'],
		['Release', scrobble.mbid_release, 'release'],
		['Cover art release', scrobble.caa_release_mbid, 'release']
	].filter(([, id]) => !!id) as [string, string, string][];

	// ── Fixing MBIDs after the fact ──
	// The cover is what makes a bad release MBID visible (one track showing a
	// compilation's art), and this dialog is where you land when you go looking.
	// Saving a release also clears the resolved artwork so the right cover is
	// fetched — see `update_scrobble` in db.rs.
	let editing = false;
	let saving = false;
	let saveMsg = '';
	let editRec = '';
	let editRel = '';

	function startEdit() {
		editRec = scrobble.mbid_recording ?? '';
		editRel = scrobble.mbid_release ?? '';
		saveMsg = '';
		editing = true;
	}

	/** MusicBrainz ids are UUIDs; anything else is a paste gone wrong. */
	const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
	$: recValid = !editRec.trim() || UUID.test(editRec.trim());
	$: relValid = !editRel.trim() || UUID.test(editRel.trim());

	async function saveMbids() {
		if (!recValid || !relValid) return;
		saving = true;
		saveMsg = '';
		const patch: Record<string, string> = {};
		if (editRec.trim() && editRec.trim() !== scrobble.mbid_recording) patch.mbid_recording = editRec.trim();
		if (editRel.trim() && editRel.trim() !== scrobble.mbid_release) patch.mbid_release = editRel.trim();
		if (!Object.keys(patch).length) {
			editing = false;
			saving = false;
			return;
		}
		try {
			const updated = await api.patchScrobble(scrobble.id, patch);
			scrobble = updated;
			dispatch('updated', updated);
			saveMsg = patch.mbid_release
				? 'Saved. The cover will re-resolve in the background.'
				: 'Saved.';
			editing = false;
		} catch (e) {
			saveMsg = 'Failed: ' + String(e).replace(/^API \d+: /, '');
		}
		saving = false;
	}

	let copied = false;
	async function copy() {
		try {
			await navigator.clipboard.writeText(pretty);
			copied = true;
			setTimeout(() => (copied = false), 1800);
		} catch { /* clipboard blocked — the JSON is selectable anyway */ }
	}
</script>

<div class="dialog-backdrop" on:click={() => dispatch('close')} on:keydown={() => {}} role="presentation">
	<div class="dialog" style="width:min(680px,100%);" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
		<div class="dialog-title">Inspect listen</div>
		<div class="dialog-body">
			<p style="margin:0 0 12px;font-size:12.5px;color:var(--td-muted);">
				The stored row exactly as Tapedeck holds it — every column, including the ones no screen shows.
			</p>

			{#if ann}
				<div class="notes-block">
					<div class="notes-head">Liner notes</div>
					<div class="notes-rows">
						{#each noteTargets as kind}
							{#if ann[kind]}
								<div class="notes-row">
									<span class="notes-kind">{kind}</span>
									<span class="notes-name">{ann[kind].name}</span>
									<button class="btn btn-ghost notes-btn" style="color:{ann[kind].loved ? '#b2622d' : ''};"
										on:click={() => toggleLove(kind)} disabled={loveBusy}>
										{ann[kind].loved ? '♥' : '♡'}
									</button>
									<button class="btn btn-secondary notes-btn" on:click={() => writeAbout(kind)}>
										{ann[kind].note_id ? 'Read note' : 'Write note'}
									</button>
								</div>
							{/if}
						{/each}
						<div class="notes-row">
							<span class="notes-kind">this playing</span>
							<span class="notes-name" style="color:var(--td-muted-2);">a note about this listen alone</span>
							<a class="btn btn-secondary notes-btn" style="text-decoration:none;" href={`/notes?listen=${scrobble.id}`}>
								{ann.listen_note_id ? 'Read note' : 'Write note'}
							</a>
						</div>
					</div>
				</div>
			{/if}

			<div class="notes-block">
				<div class="notes-head" style="display:flex;align-items:center;justify-content:space-between;gap:10px;">
					<span>MusicBrainz ids</span>
					{#if !editing}
						<button class="btn btn-ghost notes-btn" on:click={startEdit}>Fix ids</button>
					{/if}
				</div>

				{#if editing}
					<div style="display:flex;flex-direction:column;gap:10px;">
						<div class="field">
							<label for="mbrec">Recording</label>
							<input id="mbrec" class="input" bind:value={editRec} spellcheck="false"
								placeholder="00000000-0000-0000-0000-000000000000">
							{#if !recValid}<div class="warn">That isn't a MusicBrainz id.</div>{/if}
						</div>
						<div class="field">
							<label for="mbrel">Release</label>
							<input id="mbrel" class="input" bind:value={editRel} spellcheck="false"
								placeholder="00000000-0000-0000-0000-000000000000">
							{#if !relValid}<div class="warn">That isn't a MusicBrainz id.</div>{/if}
						</div>
						<p style="margin:0;font-size:11.5px;color:var(--td-muted-3);">
							Changing the release clears this listen's cached cover so the right one is fetched.
							To fix a whole album at once, use Settings → Maintenance → Metadata Sanitiser.
						</p>
						<div style="display:flex;gap:8px;">
							<button class="btn btn-secondary notes-btn" on:click={() => (editing = false)}>Cancel</button>
							<button class="btn btn-primary notes-btn" on:click={saveMbids} disabled={saving || !recValid || !relValid}>
								{saving ? 'Saving…' : 'Save'}
							</button>
						</div>
					</div>
				{:else if links.length}
					<div style="display:flex;flex-wrap:wrap;gap:8px;">
						{#each links as [label, id, kind]}
							<a
								class="tag tag-outline"
								style="text-decoration:none;"
								href={`https://musicbrainz.org/${kind}/${id}`}
								target="_blank"
								rel="noopener"
							>{label} on MusicBrainz →</a>
						{/each}
					</div>
				{:else}
					<p style="margin:0;font-size:12.5px;color:var(--td-muted-2);">
						No ids on this listen. Pasting the right ones is what makes it show the right cover
						and count towards the right record.
					</p>
				{/if}
				{#if saveMsg}<p style="margin:10px 0 0;font-size:12.5px;color:var(--td-good-fg);">{saveMsg}</p>{/if}
			</div>

			<pre class="inspect-json">{pretty}</pre>
		</div>
		<div class="dialog-actions">
			<button class="btn btn-secondary" on:click={copy}>{copied ? '✓ Copied' : 'Copy JSON'}</button>
			<button class="btn btn-primary" on:click={() => dispatch('close')}>Close</button>
		</div>
	</div>
</div>

<style>
	.notes-block {
		margin-bottom: 14px;
		padding: 12px 14px;
		border: 1px solid var(--td-border);
		border-radius: 16px;
		background: var(--td-surface2);
	}
	.notes-head {
		font-size: 10.5px;
		letter-spacing: .1em;
		text-transform: uppercase;
		color: var(--td-muted-3);
		margin-bottom: 8px;
	}
	.notes-rows { display: flex; flex-direction: column; gap: 6px; }
	.notes-row { display: flex; align-items: center; gap: 10px; }
	.notes-kind {
		width: 78px;
		flex: none;
		font-size: 11px;
		color: var(--td-muted-3);
		text-transform: uppercase;
		letter-spacing: .06em;
	}
	.notes-name {
		flex: 1;
		min-width: 0;
		font-size: 13px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.notes-btn { font-size: 12.5px; padding: 5px 12px; }
	.warn { font-size: 11.5px; color: #b2622d; margin-top: 4px; }

	.inspect-json {
		margin: 0;
		max-height: 46vh;
		overflow: auto;
		padding: 14px 16px;
		border-radius: 14px;
		background: var(--td-panel2);
		color: #a9c48a;
		font-size: 12.5px;
		line-height: 1.55;
		white-space: pre;
		tab-size: 2;
	}
</style>
