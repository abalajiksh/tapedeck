<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { api } from '$lib/api';
	import type { Note, NoteTarget, NoteRevision, NotePeriod, Entity, LovedRow, Visibility } from '$lib/api';
	import { formatDateTime, timeAgo } from '$lib/utils';
	import type { NoteQuery } from '$lib/api';
	import ShortcodeLegend from '$lib/components/ShortcodeLegend.svelte';
	import ToTop from '$lib/components/ToTop.svelte';
	import LyricPicker from '$lib/components/LyricPicker.svelte';
	import LyricAnnotate from '$lib/components/LyricAnnotate.svelte';
	import type { PinnedLine } from '$lib/api';

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
		{ value: 'listen', label: 'Listens' },
		{ value: 'period', label: 'Periods' },
		// A **pin** — a note whose target *is* a lyric line, rather than prose
		// that quotes one. Its own filter because it is its own kind of writing:
		// a footnote on somebody else's words.
		{ value: 'line', label: 'Lines' }
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

	// ── Annotate lines ──
	//
	// What the open note is *about*, which is what a picked line is written
	// against — the same context the server resolves `[[line:12]]` with. Held
	// separately from `targetLabel` because that one is a sentence for a
	// heading and this has to be three fields.
	let ctxArtist = '';
	let ctxAlbum: string | null = null;
	let ctxTrack: string | null = null;
	/** The panel is off by default: most notes are prose about a record, and a
	 *  lyric column that is always there would take a third of the editor from
	 *  everyone to serve the notes that quote. Remembered per device. */
	let annotating = false;
	let editorEl: HTMLTextAreaElement | null = null;

	// ── The two modes ──
	//
	// The design's own split, and it is not decoration: **Write & quote** is
	// prose that *mentions* a line, and **Annotate lines** is the footnote form
	// — a note whose target *is* the line. They are different kinds of writing
	// with different targets, and folding them into one screen is what made the
	// first pass ship pinning inside the reader, which is not where the design
	// puts it.
	let mode: 'quote' | 'pins' = 'quote';
	/** A line the reader sent us to pin. Consumed once, so re-rendering does
	 *  not keep re-opening an editor the writer closed. */
	let pinFocus: number | null = null;
	/** Every line pinned anywhere, for the sidebar. */
	let pinned: PinnedLine[] = [];

	function setMode(m: 'quote' | 'pins') {
		mode = m;
		if (m === 'quote') pinFocus = null;
		try {
			localStorage.setItem('td-notes-mode', m);
		} catch {
			/* not worth failing the page over */
		}
	}

	async function loadPinned() {
		try {
			pinned = (await api.pinnedLines()).lines;
		} catch {
			// A sidebar that fails to load is a sidebar with nothing in it, not
			// a page that fails to render.
			pinned = [];
		}
	}

	/** A pin somewhere other than the record in front of you — the design's
	 *  "· other record", which is what stops the sidebar reading as a list of
	 *  this record's pins when most of it is not. */
	function elsewhere(p: PinnedLine): boolean {
		const here = (ctxTrack ?? '').trim().toLowerCase();
		const t = p.line.title.trim().toLowerCase();
		if (here) return t !== here;
		return !albumTracks.some((x) => x.trim().toLowerCase() === t);
	}

	/** The tracks of the record being annotated, so `elsewhere` can tell a pin
	 *  on it from a pin on something else. Empty for a track note, which uses
	 *  its own title instead. */
	let albumTracks: string[] = [];

	async function loadAlbumTracks() {
		albumTracks = [];
		if (!ctxAlbum || !ctxArtist) return;
		try {
			albumTracks = ((await api.lyrics(ctxArtist, ctxAlbum, ctxAlbum)).tracklist ?? []).map(
				(t) => t.title
			);
		} catch {
			// Only used to label the sidebar; not worth surfacing.
		}
	}

	function openPinnedLine(p: PinnedLine) {
		// Its own track, which is what the coordinate is read against.
		openTarget({ kind: 'track', name: p.line.title, artist: p.line.artist }).then(() => {
			pinFocus = p.line.line;
			setMode('pins');
		});
	}

	/** Whether there is anything to read against at all. An artist note has no
	 *  track, so the toggle is not offered rather than opening onto a panel
	 *  that can only say so. */
	$: canAnnotate = !!ctxArtist && (!!ctxTrack || !!ctxAlbum);
	$: void ctxArtist, ctxAlbum, loadAlbumTracks();
	// A subject with nothing to read against cannot be annotated, so the mode
	// falls back rather than showing an empty panel — an artist note is prose.
	$: if (!canAnnotate && mode === 'pins') mode = 'quote';

	function toggleAnnotate() {
		annotating = !annotating;
		try {
			localStorage.setItem('td-notes-annotate', annotating ? '1' : '0');
		} catch {
			/* not worth failing the page over */
		}
	}

	/** Drop a shortcode where the cursor is.
	 *
	 *  At the cursor rather than appended, because the whole point is that a
	 *  quote sits *inside* a sentence — the design's "drop it at the cursor".
	 *  Spaced only where it needs to be, so clicking twice does not run two
	 *  references together. */
	function insertCode(code: string) {
		const el = editorEl;
		if (!el) {
			body = body ? `${body} ${code}` : code;
			dirty = true;
			return;
		}
		const start = el.selectionStart ?? body.length;
		const end = el.selectionEnd ?? start;
		const before = body.slice(0, start);
		const after = body.slice(end);
		const lead = before && !/\s$/.test(before) ? ' ' : '';
		const tail = after && !/^\s/.test(after) ? ' ' : '';
		body = `${before}${lead}${code}${tail}${after}`;
		dirty = true;
		const caret = start + lead.length + code.length;
		// After a paint, or the textarea still holds the old value and the
		// caret lands in the wrong place.
		requestAnimationFrame(() => {
			el.focus();
			el.setSelectionRange(caret, caret);
		});
	}

	/** Swap one reference for a whole run of them — a ⇧-range extending the
	 *  reference the plain click already dropped.
	 *
	 *  Falls back to inserting when the anchor's text is no longer where it was
	 *  put: the writer may have typed over it or undone it, and silently
	 *  rewriting a body somewhere else would be worse than one extra
	 *  reference. */
	function replaceCode(was: string, now: string) {
		const at = was ? body.lastIndexOf(was) : -1;
		if (at < 0) {
			insertCode(now);
			return;
		}
		body = body.slice(0, at) + now + body.slice(at + was.length);
		dirty = true;
		const caret = at + now.length;
		requestAnimationFrame(() => {
			editorEl?.focus();
			editorEl?.setSelectionRange(caret, caret);
		});
	}

	/** How many distinct lines the body quotes, for the badge the design puts
	 *  under the editor. Derived from the text rather than from the saved
	 *  note's count, so it moves as you type. */
	$: linesQuoted = new Set(
		[...body.matchAll(/\[\[(?:line|lyric|quote|blockline):\s*([^\]]+?)\s*\]\]/g)].map((m) =>
			m[1].toLowerCase()
		)
	).size;

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
		if (note.period) return { label: periodLabel(note.period), kind: note.period.unit };
		if (note.line) {
			return {
				label: note.line.text ?? `${note.line.title} — line ${note.line.line}`,
				kind: `line ${note.line.line} · ${note.line.title}`
			};
		}
		return { label: 'Untitled', kind: '' };
	}

	/** "2026-08" → "August 2026". A week keeps its ISO key: turning it into a
	 *  date range here would be a second implementation of `period_of`'s label,
	 *  free to disagree with the one Reports shows. */
	function periodLabel(p: NotePeriod): string {
		if (p.unit === 'month') {
			const [y, m] = p.key.split('-');
			const name = new Date(Number(y), Number(m) - 1, 1).toLocaleDateString('en-GB', {
				month: 'long'
			});
			return `${name} ${y}`;
		}
		return p.key;
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
			ctxArtist = '';
			ctxAlbum = null;
			ctxTrack = null;
			if (entity) {
				({ label: targetLabel, kind: targetKind } = entityLabel(entity));
				target = { entity_id: entity.id };
				// What a `[[line:12]]` in this note will be read against — the
				// same three fields `note_context` builds server-side. An
				// artist note has neither, which is why the panel is not
				// offered on one.
				ctxArtist = entity.artist_name || entity.name;
				if (entity.kind === 'recording') ctxTrack = entity.name;
				else if (entity.kind === 'release') ctxAlbum = entity.name;
				else ctxArtist = entity.name;
			} else if (listen) {
				targetLabel = `${listen.title} — ${listen.artist}`;
				targetKind = `listen · ${formatDateTime(listen.timestamp)}`;
				target = { scrobble_id: listen.id };
				ctxArtist = listen.artist;
				ctxTrack = listen.title;
				ctxAlbum = listen.album ?? null;
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

	/** A pin was written or removed: the note in front of us gathers these as
	 *  footnotes, so it has to be re-read — otherwise the footnote you just
	 *  wrote is missing from the page you are looking at. */
	async function pinsChanged() {
		await loadPinned();
		if (target) {
			try {
				open = (await api.noteFor(target)).note;
			} catch {
				// The pin saved; only the footnote list is stale.
			}
		}
		await load();
	}

	function openNote(note: Note) {
		if (note.entity) return openTarget({ entity_id: note.entity.id });
		if (note.listen) return openTarget({ scrobble_id: note.listen.id });
		// A period note is edited where it was written — the chapter it is about
		// is the context that makes it worth reading, and this page has none of
		// it. Same reasoning as a listen note opening against its listen.
		if (note.period) {
			const at = note.period.unit;
			window.location.href = `/reports?unit=${at}`;
		}
		// A pin is read where the line is. Editing it in a box with no lyric
		// round it would be editing a footnote with the page torn out — the
		// same reasoning that sends a period note back to its chapter.
		if (note.line) {
			const q = new URLSearchParams({
				artist: note.line.artist,
				title: note.line.title,
				line: String(note.line.line)
			});
			window.location.href = `/lyrics?${q}`;
		}
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

	/** Arrive here from the deck's Quote button: open the note on that track and
	 *  drop the reference in, so quoting a line while it plays is one click.
	 *
	 *  A one-shot on mount rather than part of the reactive `?entity=` sync,
	 *  because it names a track by *name* — the entity may not exist yet, and
	 *  `openTarget` is what creates it. Re-running it on every URL change would
	 *  re-insert the reference each time the router touched the query. */
	async function openFromQuery(p: URLSearchParams) {
		const name = p.get('name');
		const artist = p.get('artist');
		const kind = p.get('kind');
		if (!name || (kind !== 'track' && kind !== 'album' && kind !== 'artist')) return false;
		await openTarget({ kind, name, ...(kind === 'artist' ? {} : { artist: artist ?? '' }) });
		const pin = Number(p.get('pin'));
		if (Number.isFinite(pin) && pin > 0) {
			pinFocus = pin;
			mode = 'pins';
		}
		const quote = p.get('quote');
		if (quote) {
			// Appended rather than inserted at the cursor: nothing is focused
			// yet on a page that has only just opened, so there is no cursor to
			// respect. A line of its own, because it arrives before the sentence
			// it will end up inside.
			body = body ? `${body}\n\n${quote} ` : `${quote} `;
			dirty = true;
			annotating = true;
			requestAnimationFrame(() => {
				editorEl?.focus();
				editorEl?.setSelectionRange(body.length, body.length);
			});
		}
		return true;
	}

	onMount(async () => {
		try {
			annotating = localStorage.getItem('td-notes-annotate') === '1';
			const m = localStorage.getItem('td-notes-mode');
			if (m === 'pins' || m === 'quote') mode = m;
		} catch {
			// A browser that refuses storage still gets an editor.
		}
		await load();
		await loadPinned();
		await openFromQuery(new URLSearchParams(window.location.search));
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
	<div style="display:flex;gap:8px;align-items:center;flex-wrap:wrap;">
		{#if canAnnotate}
			<!-- The design's two modes. Only offered where there is something to
			     annotate: an artist note is prose, and a tab that switches to a
			     panel whose only job is to say so is worse than no tab. -->
			<div class="modes" role="tablist" aria-label="Writing mode">
				<button class="mode" class:on={mode === 'quote'} role="tab"
					aria-selected={mode === 'quote'} on:click={() => setMode('quote')}>Write &amp; quote</button>
				<button class="mode" class:on={mode === 'pins'} role="tab"
					aria-selected={mode === 'pins'} on:click={() => setMode('pins')}>Annotate lines</button>
			</div>
		{/if}
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
		{#if mode === 'pins'}
			<!-- The design's "Lines you've written about". **Across everything**
			     rather than this record, which is what makes it a way back into
			     writing you had forgotten rather than a second copy of the margin
			     already on screen. -->
			<div class="panel" style="border-radius:22px;padding:20px;margin-bottom:16px;">
				<h3 class="h-display" style="font-size:17px;margin:0 0 4px;">Lines you've written about</h3>
				<p style="margin:0 0 12px;font-size:11.5px;color:var(--td-muted-3);line-height:1.45;">
					Across the whole shelf, newest first.
				</p>
				{#if !pinned.length}
					<p style="font-size:13px;color:var(--td-muted);margin:0;">
						None yet. Hover a line on the right and pin a note to it.
					</p>
				{:else}
					<div style="display:flex;flex-direction:column;gap:5px;">
						{#each pinned as p (p.note_id)}
							<button class="pinrow" on:click={() => openPinnedLine(p)}>
								<div class="pinco">{p.line.title} · {p.line.line}</div>
								<div class="pinwords">{p.line.text ?? '—'}</div>
								<div class="pinmeta">
									{timeAgo(p.updated_at)}{elsewhere(p) ? ' · other record' : ''}
								</div>
							</button>
						{/each}
					</div>
				{/if}
				<div style="height:1px;background:var(--td-border);margin:14px 0;"></div>
				<p style="margin:0;font-size:11.5px;color:var(--td-muted-2);line-height:1.5;">
					A pin is a note whose target <em>is</em> the line. It shows here, in the
					reader's sidebar, and as a footnote on the record's own note.
				</p>
			</div>
		{:else}
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
								{timeAgo(n.updated_at)}{n.lines_quoted ? ` · ${n.lines_quoted} line${n.lines_quoted > 1 ? 's' : ''} quoted` : ''}{n.revisions ? ` · ${n.revisions} earlier version${n.revisions > 1 ? 's' : ''}` : ''}{n.visibility !== 'private' ? ` · ${n.visibility}` : ''}
							</div>
						</button>
					{/each}
				</div>
			{/if}
		</div>
		{/if}

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
					{#if canAnnotate}
						<!-- Not offered where there is nothing to read against: an
						     artist note has no track, so a toggle would open onto
						     a panel whose only job would be to say so. -->
						<button class="btn btn-secondary" class:on={annotating} on:click={toggleAnnotate}>
							{annotating ? 'Hide lyrics' : 'Annotate lines'}
						</button>
					{/if}
					{#if target.entity_id}
						<button class="btn btn-secondary" style="color:{loved ? '#b2622d' : 'inherit'};" on:click={toggleLove}>
							{loved ? '♥ Loved' : '♡ Love'}
						</button>
					{/if}
					<button class="btn btn-secondary" on:click={closeEditor}>Close</button>
				</div>
			</div>

			{#if mode === 'pins'}
				<!-- The footnote form. The pin *is* the note, so this is not a
				     second editor for the same thing — it writes a different
				     target, and the prose editor is one tab away. -->
				<LyricAnnotate
					artist={ctxArtist}
					track={ctxTrack}
					album={ctxAlbum}
					focus={pinFocus}
					on:changed={pinsChanged}
					on:prose={() => setMode('quote')}
				/>
			{:else if previewing}
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
				<div class="writecols" class:split={annotating && canAnnotate}>
					<div style="min-width:0;">
						<textarea
							class="input note-input"
							bind:this={editorEl}
							bind:value={body}
							on:input={() => (dirty = true)}
							placeholder="Bought this on a whim in 2011. [[track:3]] is the one that got me — nothing else on it sounds like [[artist:Boards of Canada]]."
						></textarea>
						{#if linesQuoted}
							<p class="quoted">{linesQuoted} line{linesQuoted === 1 ? '' : 's'} quoted</p>
						{/if}
						<div style="margin-top:10px;"><ShortcodeLegend /></div>
					</div>
					{#if annotating && canAnnotate}
						<LyricPicker
							artist={ctxArtist}
							track={ctxTrack}
							album={ctxAlbum}
							{body}
							on:insert={(e) => insertCode(e.detail)}
							on:replace={(e) => replaceCode(e.detail.was, e.detail.now)}
						/>
					{/if}
				</div>
			{/if}

			{#if mode === 'quote'}
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

			<!-- **Pins roll up into the record's own note as footnotes** — the
			     design's third rendering, and the one that could ever federate:
			     nothing of the provider's text is written into the prose, so a
			     reader elsewhere sees your words and a stub. -->
			{#if open?.pins?.length}
				<div class="fnotes">
					<div class="fnhead">Pinned lines on this record</div>
					{#each open.pins as pin, i (pin.note_id)}
						<div class="fn">
							<span class="fnmark">{i + 1}</span>
							<div style="min-width:0;">
								<div class="fntext">{pin.excerpt}</div>
								<div class="fnline">
									<span class="fnco">{pin.line.title} · line {pin.line.line}</span>
									{#if pin.line.text}
										<span class="fnwords">{pin.line.text}</span>
									{:else}
										<span class="fngone">no words held for that line any more</span>
									{/if}
								</div>
							</div>
						</div>
					{/each}
				</div>
			{/if}
			{/if}

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
	/* The editor and the lyrics beside it. Splits only when the panel is open,
	   so a note that quotes nothing keeps the full width — and stacks under
	   1100 rather than squeezing both, since a lyric in a 200px column is
	   unreadable and so is prose. */
	.writecols { display: grid; grid-template-columns: minmax(0, 1fr); gap: 14px; }
	.writecols.split { grid-template-columns: minmax(0, 1fr) 320px; align-items: start; }
	@media (max-width: 1100px) {
		.writecols.split { grid-template-columns: minmax(0, 1fr); }
	}
	.quoted {
		display: inline-block;
		font-size: 11.5px;
		color: var(--td-good-fg);
		background: var(--td-good-bg);
		padding: 3px 10px;
		border-radius: 999px;
		margin: 8px 0 0;
	}
	.btn.on { border-color: var(--td-accent); color: var(--td-accent); }

	/* The two writing modes. Declared here rather than borrowed, for the reason
	   `.seg-btn` is repeated on every page that uses it: Svelte scopes styles,
	   so a class defined elsewhere renders as bare text and nothing errors. */
	.modes {
		display: inline-flex;
		gap: 4px;
		padding: 4px;
		border-radius: 999px;
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
	}
	.mode {
		font-family: 'Caprasimo', serif;
		font-size: 12.5px;
		padding: 6px 15px;
		border-radius: 999px;
		border: none;
		background: transparent;
		color: var(--td-muted-2);
		cursor: pointer;
	}
	.mode.on { background: var(--td-accent); color: #fbf5ea; }

	/* "Lines you've written about" */
	.pinrow {
		display: block;
		width: 100%;
		text-align: left;
		padding: 9px 11px;
		border-radius: 14px;
		border: 1px solid transparent;
		background: none;
		cursor: pointer;
		font-family: inherit;
	}
	.pinrow:hover { background: var(--td-surface2); border-color: var(--td-border); }
	.pinco { font: 500 10px ui-monospace, Menlo, monospace; color: var(--td-accent); }
	.pinwords {
		font-size: 12.5px;
		line-height: 1.4;
		margin-top: 2px;
		color: var(--td-text);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.pinmeta { font-size: 10.5px; color: var(--td-muted-3); margin-top: 3px; }

	/* Footnotes under the record's own note. */
	.fnotes {
		margin-top: 18px;
		padding-top: 16px;
		border-top: 1px solid var(--td-border);
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.fnhead {
		font: 600 9.5px 'Figtree', system-ui, sans-serif;
		letter-spacing: 0.16em;
		text-transform: uppercase;
		color: var(--td-muted-3);
	}
	.fn { display: grid; grid-template-columns: 22px minmax(0, 1fr); gap: 10px; align-items: start; }
	.fnmark {
		display: grid;
		place-items: center;
		width: 20px;
		height: 20px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--td-accent) 18%, transparent);
		color: var(--td-accent);
		font: 600 10.5px 'Figtree', system-ui, sans-serif;
	}
	.fntext { font-size: 13px; line-height: 1.55; color: var(--td-text); }
	.fnline { margin-top: 3px; display: flex; gap: 8px; flex-wrap: wrap; align-items: baseline; }
	.fnco {
		font: 500 10px ui-monospace, Menlo, monospace;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--td-muted-3);
	}
	.fnwords { font-size: 12px; color: var(--td-muted-2); font-style: italic; }
	.fngone { font-size: 12px; color: var(--td-muted-3); }

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
