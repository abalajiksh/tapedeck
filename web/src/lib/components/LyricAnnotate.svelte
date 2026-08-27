<script lang="ts">
	/**
	 * *Annotate lines* — artboard 1d of `temp/Lyrics Final.dc.html`, and the
	 * place a **pin** is actually written.
	 *
	 * The design splits lyrics three ways and this is the third: the reader is
	 * for reading, *Write & quote* is for prose that mentions a line, and this is
	 * for the footnote form — a note whose target *is* the line. Both of the
	 * others hand off here rather than growing their own editor, which is why
	 * the reader's "Pin a note" is a link.
	 *
	 * The margin is the whole idea: a pin sits beside the line it is about,
	 * joined to it, rather than under a heading somewhere else. Hence the
	 * connector dot and rule — they are what make the card belong to that line.
	 *
	 * The design's **ES** toggle is absent because LRCLIB returns no
	 * translation, and its per-line "played 41 times" sits on the header
	 * instead: it is the *track's* count, and on a line it reads as a count of
	 * that line.
	 */
	import { createEventDispatcher } from 'svelte';
	import { api } from '$lib/api';
	import type { LyricDoc, LyricLine } from '$lib/api';

	export let artist = '';
	/** The track being annotated. A record's note picks one from its tracklist. */
	export let track: string | null = null;
	export let album: string | null = null;
	/** Open on this line, when something sent us here pointing at one. */
	export let focus: number | null = null;

	const dispatch = createEventDispatcher<{ changed: void; prose: void }>();

	let picked = '';
	let doc: LyricDoc | null = null;
	let tracks: string[] = [];
	let loading = false;
	let error = '';
	let editing: number | null = null;
	let draft = '';
	let saving = false;

	const clean = (e: unknown) => String(e).replace(/^API \d+: /, '');

	async function loadTracks() {
		error = '';
		if (track) {
			tracks = [track];
			picked = track;
			return;
		}
		if (!album || !artist) {
			tracks = [];
			return;
		}
		try {
			const seed = await api.lyrics(artist, album, album);
			tracks = (seed.tracklist ?? []).map((t) => t.title);
			if (!picked && tracks.length) picked = tracks[0];
		} catch (e) {
			error = clean(e);
		}
	}

	async function loadDoc() {
		if (!artist || !picked) {
			doc = null;
			return;
		}
		loading = true;
		error = '';
		try {
			doc = await api.lyrics(artist, picked, album ?? undefined);
		} catch (e) {
			error = clean(e);
			doc = null;
		}
		loading = false;
	}

	$: void artist, track, album, loadTracks();
	$: void picked, loadDoc();

	// Open the editor on the line we were sent to, once, after the lyric is in.
	let opened = false;
	$: if (focus != null && doc && !opened) {
		opened = true;
		editing = focus;
		draft = doc.lines.find((l) => l.n === focus)?.pin?.body_md ?? '';
	}

	function startPin(l: LyricLine) {
		editing = l.n;
		draft = l.pin?.body_md ?? '';
	}

	async function savePin() {
		if (editing == null || !doc || !draft.trim()) return;
		saving = true;
		error = '';
		try {
			await api.saveNote({
				artist: doc.artist,
				name: doc.title,
				line: editing,
				body_md: draft,
				visibility: 'private'
			});
			editing = null;
			draft = '';
			await loadDoc();
			// The record's own note gathers these as footnotes, so it has to be
			// re-read — otherwise the footnote you just wrote is missing from
			// the page you are looking at.
			dispatch('changed');
		} catch (e) {
			error = clean(e);
		}
		saving = false;
	}

	async function removePin(l: LyricLine) {
		if (!l.pin) return;
		try {
			await api.deleteNote(l.pin.note_id);
			await loadDoc();
			dispatch('changed');
		} catch (e) {
			error = clean(e);
		}
	}

	function mmss(s: number): string {
		const m = Math.floor(s / 60);
		return `${m}:${String(Math.floor(s - m * 60)).padStart(2, '0')}`;
	}

	function readerHref(): string {
		const q = new URLSearchParams({ artist, title: picked });
		if (album) q.set('album', album);
		return `/lyrics?${q}`;
	}

	$: lines = doc?.lines ?? [];
	$: pinCount = lines.filter((l) => l.pin).length;
</script>

<div class="panel ann">
	<div class="ahead">
		<div style="min-width:0;flex:1;">
			{#if tracks.length > 1}
				<select class="input atrack" bind:value={picked} aria-label="Which track">
					{#each tracks as t (t)}<option value={t}>{t}</option>{/each}
				</select>
			{:else}
				<h3 class="h-display attl">{picked || '—'}</h3>
			{/if}
			<div class="asub">
				{artist}{album ? ` · ${album}` : ''}
				{#if doc?.source}· {doc.source}{doc.timed ? ' · timed' : ''}{/if}
				{#if doc?.plays}· played {doc.plays} time{doc.plays === 1 ? '' : 's'}{/if}
			</div>
		</div>
		<a class="btn btn-secondary" href={readerHref()}>Open the reader ↗</a>
	</div>

	{#if error}<p class="amsg bad">{error}</p>{/if}

	<div class="abody">
		{#if loading}
			<p class="amsg">Reading…</p>
		{:else if !picked}
			<p class="amsg">Nothing to annotate — this note is not about a track or a record.</p>
		{:else if doc?.instrumental}
			<p class="amsg">No words — this track is instrumental.</p>
		{:else if !lines.length}
			<p class="amsg">
				Nothing held for this one yet. Lyrics are fetched a few tracks a minute,
				most-played first — or LRCLIB may simply not have it.
			</p>
		{:else}
			{#each lines as l (l.n)}
				{#if !l.text.trim()}
					<div class="abreak"></div>
				{:else}
					<div class="aline" class:on={editing === l.n || !!l.pin}>
						<span class="an">{doc?.timed && l.at != null ? mmss(l.at) : l.n}</span>
						<div class="atext">
							<div class="awords">{l.text}</div>
							{#if (l.quoted ?? 0) > 0}
								<div class="amarks">
									<span class="apill">quoted in {l.quoted} note{l.quoted === 1 ? '' : 's'}</span>
								</div>
							{/if}
						</div>
						<div class="amargin">
							{#if editing === l.n}
								<div class="acard editing">
									<!-- svelte-ignore a11y-autofocus -->
									<textarea class="input" bind:value={draft} rows="3" autofocus
										placeholder="The one admission on the whole side."></textarea>
									<div class="arow">
										<button class="btn btn-primary" on:click={savePin} disabled={saving || !draft.trim()}>
											{saving ? '…' : 'Pin it'}
										</button>
										<button class="btn btn-secondary" on:click={() => (editing = null)}>Cancel</button>
										<span style="flex:1;"></span>
										<span class="atiny">private</span>
									</div>
								</div>
							{:else if l.pin}
								<div class="acard">
									<div class="atext-pin">{l.pin.excerpt}</div>
									<div class="arow">
										<button class="btn btn-ghost atinybtn" on:click={() => startPin(l)}>Edit</button>
										<button class="btn btn-ghost atinybtn danger" on:click={() => removePin(l)}>Remove</button>
									</div>
								</div>
							{:else}
								<button class="aadd" on:click={() => startPin(l)} aria-label="Pin a note to line {l.n}">
									＋ pin a note here
								</button>
							{/if}
						</div>
					</div>
				{/if}
			{/each}
		{/if}
	</div>

	<div class="afoot">
		<span style="flex:1;">
			Pins roll up into the record's own note as footnotes.
			{#if pinCount}<strong>{pinCount}</strong> on this track.{/if}
		</span>
		<button class="btn btn-secondary" on:click={() => dispatch('prose')}>
			Write about the record instead
		</button>
	</div>
</div>

<style>
	.ann { border-radius: 22px; overflow: hidden; display: flex; flex-direction: column; }
	.ahead {
		display: flex;
		align-items: flex-start;
		gap: 12px;
		padding: 16px 22px;
		border-bottom: 1px solid var(--td-border);
		flex-wrap: wrap;
	}
	.attl { font-size: 17px; margin: 0; }
	.atrack { min-height: 34px; font-size: 14px; padding: 5px 10px; max-width: 100%; }
	.asub { font-size: 11.5px; color: var(--td-muted-2); margin-top: 3px; }
	.amsg { font-size: 13px; color: var(--td-muted); margin: 14px 22px; line-height: 1.55; }
	.amsg.bad { color: var(--td-bad-fg); }
	.abody { padding: 12px 16px 16px; }
	.abreak { height: 10px; }
	.aline {
		display: grid;
		grid-template-columns: 44px minmax(0, 1fr) 300px;
		gap: 16px;
		align-items: start;
		padding: 7px 10px;
		border-radius: 12px;
	}
	.aline.on { background: color-mix(in srgb, var(--td-accent) 8%, transparent); }
	.an {
		text-align: right;
		padding-top: 4px;
		color: var(--td-muted-3);
		font-size: 11px;
		font-variant-numeric: tabular-nums;
	}
	.atext { min-width: 0; }
	.awords { font-size: 16px; line-height: 1.45; }
	.amarks { display: flex; gap: 7px; margin-top: 6px; flex-wrap: wrap; }
	.apill {
		font-size: 11px;
		padding: 3px 10px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--td-accent) 18%, transparent);
		color: var(--td-accent);
	}
	.amargin { min-width: 0; position: relative; }
	/* The connector: a dot on the line's side of the gap and a rule across it.
	   This is what makes a card in the margin *belong* to the line beside it
	   rather than float next to it. */
	.acard { position: relative; border: 1px solid var(--td-border); border-radius: 16px; background: var(--td-surface2); padding: 11px 13px; }
	.acard::before {
		content: '';
		position: absolute;
		left: -19px;
		top: 16px;
		width: 19px;
		height: 1px;
		background: var(--td-muted-3);
		opacity: 0.5;
	}
	.acard::after {
		content: '';
		position: absolute;
		left: -24px;
		top: 13px;
		width: 7px;
		height: 7px;
		border-radius: 999px;
		background: var(--td-muted-3);
	}
	.acard.editing {
		border-color: var(--td-accent);
		background: var(--td-surface);
		box-shadow: 0 4px 14px color-mix(in srgb, var(--td-accent) 16%, transparent);
	}
	.acard.editing::before { background: var(--td-accent); opacity: 0.5; }
	.acard.editing::after { background: var(--td-accent); }
	.atext-pin { font-size: 12.5px; line-height: 1.55; color: var(--td-text); }
	.arow { display: flex; align-items: center; gap: 8px; margin-top: 9px; flex-wrap: wrap; }
	.atiny { font-size: 10.5px; color: var(--td-muted-3); }
	.atinybtn { font-size: 11.5px; padding: 3px 8px; }
	.danger { color: var(--td-bad-fg); }
	/* Faded until the line is hovered or the button is focused. **Not
	   `display: none`** — that takes it out of the tab order entirely, so the
	   only way to pin a line would be a mouse. `:focus` rather than
	   `:focus-visible`, because that heuristic depends on how focus was last
	   moved and a control invisible until it matches can be tabbed to and still
	   not appear. */
	.aadd {
		opacity: 0;
		pointer-events: none;
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: 11.5px;
		padding: 5px 12px;
		border-radius: 999px;
		border: 1px dashed var(--td-border);
		background: transparent;
		color: var(--td-muted-2);
		cursor: pointer;
		font-family: inherit;
	}
	.aline:hover .aadd,
	.aadd:focus { opacity: 1; pointer-events: auto; }
	.aadd:hover { color: var(--td-accent); border-color: var(--td-accent); }
	.afoot {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 14px 22px;
		border-top: 1px solid var(--td-border);
		background: var(--td-surface2);
		font-size: 12px;
		color: var(--td-muted-2);
		flex-wrap: wrap;
	}
	/* The margin drops under the words rather than squeezing them: a pin is
	   worth reading and a two-word column is not. */
	@media (max-width: 820px) {
		.aline { grid-template-columns: 34px minmax(0, 1fr); }
		.amargin { grid-column: 2; }
		.acard::before, .acard::after { display: none; }
	}
</style>
