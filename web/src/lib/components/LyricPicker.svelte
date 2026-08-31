<script lang="ts">
	/**
	 * *Annotate lines* — the lyrics panel beside the note editor.
	 *
	 * **The gap this closes.** Before it there was no way to insert a
	 * `[[line:…]]` except by typing it, which means knowing the line number,
	 * which means opening the reader first — a closed loop with an extra step,
	 * and the largest of the things the design specified and v0.99.0 did not
	 * build.
	 *
	 * **It inserts the shortcode, never the words.** That is the first of the
	 * design's four rules and the whole reason the feature is worth having: a
	 * reference is an index, so correcting a fetched lyric later corrects every
	 * quote with it rather than orphaning them. Pasting the text would be a
	 * snapshot, and would also put the provider's words into a note that can be
	 * exported.
	 *
	 * Two departures from the design, both because it is a shelf design:
	 *
	 * * Its track picker lists a record's sides (`A3 · Long Way From The
	 *   Coast`). Most listening never came off a record, so the picker here is
	 *   the tracks of whatever the note is about — the record's, for a record;
	 *   the one track, for a track — and the coordinate written is the short
	 *   `[[line:12]]` unless a side is genuinely known.
	 * * Its **ES** translation toggle and **LRCLIB v3 ▾** version picker are
	 *   absent rather than deferred: LRCLIB returns the words and nothing else,
	 *   and `/api/get` returns one record with no version history. A control
	 *   over data that does not exist is worse than no control.
	 */
	import { createEventDispatcher } from 'svelte';
	import { api } from '$lib/api';
	import type { LyricDoc } from '$lib/api';
	import { gutter, cycleGutter, gutterLabel, gutterTitle } from '$lib/lyricgutter';

	/** What the note is about, which is what a picked line is written against. */
	export let artist = '';
	/** The track, when the note is about one. */
	export let track: string | null = null;
	/** The record, when it is about one — the picker then lists its tracks. */
	export let album: string | null = null;
	/** The current note body, so lines already referenced can be marked. */
	export let body = '';

	const dispatch = createEventDispatcher<{ insert: string; replace: { was: string; now: string } }>();

	let picked = '';
	let doc: LyricDoc | null = null;
	let tracks: string[] = [];
	let loading = false;
	let error = '';
	/** The anchor of a ⇧-range. Set by a plain click, consumed by a shift one. */
	let anchor: number | null = null;
	/** What that plain click actually wrote.
	 *
	 *  A ⇧-range **extends the reference already dropped** rather than adding a
	 *  second one — click 5 then ⇧-click 8 and you get 5 6 7 8, not 5 5 6 7 8.
	 *  Doing that means knowing the exact text the anchor put in, so the editor
	 *  can swap it for the whole run; if it is no longer there — the writer
	 *  typed over it, or undid it — the range is simply inserted and nothing is
	 *  quietly rewritten. */
	let anchorCode = '';

	const clean = (e: unknown) => String(e).replace(/^API \d+: /, '');

	/** Which tracks to offer. A note about a track has exactly one; a note about
	 *  a record has the ones that record has actually been played from, which is
	 *  the only tracklist this instance can be sure of. */
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
			// The reader's own payload carries the record's tracklist, so asking
			// for any one track of it gives the list — one request rather than a
			// second endpoint that would answer the same question differently.
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
		anchor = null;
		try {
			doc = await api.lyrics(artist, picked, album ?? undefined);
		} catch (e) {
			error = clean(e);
			doc = null;
		}
		loading = false;
	}

	// Re-runs when the subject changes, which is what makes the panel follow the
	// note being edited rather than whichever one it was opened on.
	$: void artist, track, album, loadTracks();
	$: void picked, loadDoc();

	/** The shortcode a line becomes.
	 *
	 *  The short form, because it is read against the thing the note is about —
	 *  which is what the note editor's own context already resolves. The long
	 *  `[[line:A:3:12]]` is for a record whose side is actually known, and the
	 *  panel writes it only when there is a side to name. */
	function code(n: number): string {
		return `[[line:${n}]]`;
	}

	/** Lines this note already refers to, so they can be marked. Scanned from
	 *  the body rather than tracked, so an edit by hand is seen too. */
	$: referenced = new Set(
		[...body.matchAll(/\[\[(?:line|lyric|quote|blockline):\s*(\d+)\s*\]\]/g)].map((m) =>
			Number(m[1])
		)
	);

	function pick(n: number, shift: boolean) {
		if (shift && anchor != null) {
			// A range is what makes quoting a couplet one gesture rather than
			// two references — the design's ⇧ rule.
			const [lo, hi] = anchor <= n ? [anchor, n] : [n, anchor];
			// A stanza break is a numbered line and quoting one quotes nothing,
			// so a range steps over it rather than dropping a reference that
			// can only render as prose.
			const codes = (doc?.lines ?? [])
				.filter((l) => l.n >= lo && l.n <= hi && l.text.trim())
				.map((l) => code(l.n))
				.join(' ');
			if (codes) dispatch('replace', { was: anchorCode, now: codes });
			anchor = null;
			anchorCode = '';
			return;
		}
		anchor = n;
		anchorCode = code(n);
		dispatch('insert', anchorCode);
	}

	function mmss(s: number): string {
		const m = Math.floor(s / 60);
		return `${m}:${String(Math.floor(s - m * 60)).padStart(2, '0')}`;
	}

	// ── Fixing the words ──
	//
	// The design puts this in this panel's footer, and it is the only place it
	// lives: the reader is a reading surface with no dialog on it. The rule is
	// the shelf's running times — **a value you supplied is never overwritten by
	// a lookup** — which is what settles the collision with *newest fetch wins*:
	// between two fetches the newer one wins, and between a fetch and a person,
	// the person does.
	let fixing = false;
	let fixPlain = '';
	let fixSynced = '';
	let fixNote = '';
	let saving = false;

	function startFix() {
		if (!doc) return;
		fixPlain = doc.plain ?? '';
		fixSynced = doc.synced ?? '';
		fixNote = '';
		fixing = true;
	}

	async function saveFix() {
		if (!doc) return;
		saving = true;
		error = '';
		try {
			const r = await api.fixLyrics({
				artist: doc.artist,
				title: doc.title,
				plain: fixPlain,
				synced: fixSynced
			});
			fixing = false;
			await loadDoc();
			// The one thing worth saying out loud: a reference is an index, so a
			// changed line count re-points every quote past the change.
			fixNote = r.renumbered
				? `Saved — but the lyric went from ${r.lines_before} lines to ${r.lines_after}, so every reference past the change now points at a different line.`
				: 'Saved. A later fetch will not overwrite this.';
		} catch (e) {
			error = clean(e);
		}
		saving = false;
	}

	// ── Pick a different match ───────────────────────────────────────────────
	//
	// The honest form of the design's `LRCLIB v3 ▾`. LRCLIB has no version
	// history; what it has is *other records* for the same song, and swapping
	// to one is how a wrong match gets fixed without retyping the words.
	//
	// Adopting one goes through the ordinary correction path, so it is marked
	// edited and survives every later fetch — and it reports the renumbering,
	// because a reference is an index and a different record moves every quote
	// past the change.
	let matching = false;
	let matches: Awaited<ReturnType<typeof api.lyricMatches>>['matches'] = [];
	let matchWhy = '';
	let searching = false;

	async function startMatches() {
		if (!doc) return;
		matching = true;
		searching = true;
		matches = [];
		matchWhy = '';
		error = '';
		try {
			const r = await api.lyricMatches(doc.artist, doc.title);
			matches = r.matches;
			// Told apart on purpose: "LRCLIB has nothing else for this" is an
			// answer, and "we could not ask" is not. Reporting the second as the
			// first would send somebody off to type the words out by hand.
			matchWhy = r.unavailable
				? (r.why ?? 'Could not reach LRCLIB.')
				: matches.length
					? ''
					: 'LRCLIB has nothing else for this track.';
		} catch (e) {
			error = clean(e);
			matching = false;
		}
		searching = false;
	}

	async function adopt(m: (typeof matches)[number]) {
		if (!doc) return;
		saving = true;
		error = '';
		try {
			const r = await api.fixLyrics({
				artist: doc.artist,
				title: doc.title,
				plain: m.plain ?? '',
				synced: m.synced_text ?? ''
			});
			matching = false;
			await loadDoc();
			fixNote = r.renumbered
				? `Swapped — but this record has ${r.lines_after} lines where the last had ${r.lines_before}, so every reference past the change now points at a different line.`
				: 'Swapped. A later fetch will not overwrite this.';
		} catch (e) {
			error = clean(e);
		}
		saving = false;
	}

	function readerHref(): string {
		const q = new URLSearchParams({ artist, title: picked });
		if (album) q.set('album', album);
		return `/lyrics?${q}`;
	}
</script>

<div class="panel lp">
	<div class="lphead">
		<div class="lptop">
			<h3 class="h-display lpttl">Lyrics</h3>
			{#if doc?.source}<span class="lpsrc">{doc.source}{doc.timed ? ' · timed' : ''}</span>{/if}
		</div>
		<div class="lppick">
			{#if tracks.length > 1}
				<select class="input" bind:value={picked} aria-label="Which track">
					{#each tracks as t (t)}<option value={t}>{t}</option>{/each}
				</select>
			{:else}
				<span class="lpone">{picked || '—'}</span>
			{/if}
			{#if picked}
				<a class="lpout" href={readerHref()} title="Open the reader" aria-label="Open the reader">↗</a>
			{/if}
		</div>
	</div>

	<div class="lpbody">
		{#if error}
			<p class="lpmsg">{error}</p>
		{:else if loading}
			<p class="lpmsg">Reading…</p>
		{:else if !picked}
			<p class="lpmsg">Nothing to read against — this note is not about a track or a record.</p>
		{:else if doc?.instrumental}
			<p class="lpmsg">No words — this track is instrumental.</p>
		{:else if !doc?.lines.length}
			<p class="lpmsg">
				Nothing held for this one yet. Lyrics are fetched a few tracks a minute,
				most-played first — or LRCLIB may simply not have it.
			</p>
		{:else}
			{#each doc.lines as l (l.n)}
				{#if !l.text.trim()}
					<div class="lpbreak"></div>
				{:else}
					<button
						class="lpline"
						class:on={referenced.has(l.n)}
						class:anchor={anchor === l.n}
						on:click={(e) => pick(l.n, e.shiftKey)}
						aria-label="Quote line {l.n}: {l.text}"
					>
						<span class="lpn" class:empty={$gutter === 'none'}>{gutterLabel($gutter, l.n, l.at, mmss)}</span>
						<span class="lptext">
							{l.text}
							{#if referenced.has(l.n)}<span class="lpin">in this note</span>{/if}
						</span>
						<span class="lpq">”</span>
					</button>
				{/if}
			{/each}
		{/if}
	</div>

	<div class="lpfoot">
		<span style="flex:1;">
			Click a line to drop it at the cursor. Hold <kbd>⇧</kbd> for a range.
			The <em>shortcode</em> goes in, never the words — correct the lyric later and
			every quote corrects with it.
		</span>
		{#if doc?.lines.length}
			<button class="lpfix" on:click={cycleGutter} title={gutterTitle($gutter)}>
				{$gutter === 'time' ? '0:00' : $gutter === 'number' ? '#' : '–'}
			</button>
			<button class="lpfix" on:click={startMatches}>Pick a different match</button>
			<button class="lpfix" on:click={startFix}>Fix the words</button>
		{/if}
	</div>
	{#if fixNote}<p class="lpnote">{fixNote}</p>{/if}
</div>

{#if fixing}
	<div class="scrim" role="presentation" on:click={() => (fixing = false)}></div>
	<div class="dialog" role="dialog" aria-label="Fix the words">
		<h3 class="h-display" style="font-size:18px;margin:0 0 4px;">Fix the words</h3>
		<p class="dlgsub">
			A correction is kept: a later lookup will never overwrite it. But every
			reference already written is an <em>index</em> — so adding or removing a
			line moves every quote past it onto different words.
		</p>
		<label class="fl" for="fx-plain">Plain</label>
		<textarea id="fx-plain" class="input" rows="10" bind:value={fixPlain}></textarea>
		<label class="fl" for="fx-synced">Timed — <code>[mm:ss.xx] words</code> per line</label>
		<textarea id="fx-synced" class="input" rows="6" bind:value={fixSynced}></textarea>
		<div class="dlgrow">
			<button class="btn btn-primary" on:click={saveFix} disabled={saving}>{saving ? '…' : 'Save the words'}</button>
			<button class="btn btn-secondary" on:click={() => (fixing = false)}>Close</button>
		</div>
	</div>
{/if}

{#if matching}
	<div class="scrim" role="presentation" on:click={() => (matching = false)}></div>
	<div class="dialog" role="dialog" aria-label="Pick a different match">
		<h3 class="h-display" style="font-size:18px;margin:0 0 4px;">Pick a different match</h3>
		<p class="dlgsub">
			LRCLIB keeps no version history — these are <em>other records</em> for the
			same song. Swapping to one is kept the same way a correction is: a later
			lookup will never overwrite it.
		</p>
		{#if searching}
			<p class="lpmsg">Asking LRCLIB…</p>
		{:else if matchWhy}
			<p class="lpmsg">{matchWhy}</p>
		{:else}
			<div class="mlist">
				{#each matches as m (m.id ?? `${m.artist}-${m.title}-${m.lines}`)}
					<button class="mrow" on:click={() => adopt(m)} disabled={saving}>
						<span class="mmain">
							<span class="mt">{m.title ?? doc?.title}</span>
							<span class="ma">{m.artist ?? doc?.artist}</span>
						</span>
						<span class="mmeta">
							<!-- Which of these matters most: a timed lyric follows the
							     deck and a plain one cannot. -->
							<span class="mpill" class:lit={m.synced}>{m.synced ? 'Timed' : 'Plain'}</span>
							<span class="mn">{m.lines} lines</span>
							{#if m.duration}<span class="mn">{mmss(m.duration)}</span>{/if}
						</span>
					</button>
				{/each}
			</div>
		{/if}
		<div class="dlgrow">
			<button class="btn btn-secondary" on:click={() => (matching = false)}>Close</button>
		</div>
	</div>
{/if}

<style>
	.mlist { display: flex; flex-direction: column; gap: 6px; max-height: 46vh; overflow: auto; margin: 4px 0 12px; }
	.mrow {
		display: flex; align-items: center; justify-content: space-between; gap: 12px;
		width: 100%; text-align: left; padding: 9px 11px; border-radius: 11px;
		border: 1px solid var(--td-border); background: var(--td-surface);
		color: var(--td-text); cursor: pointer;
	}
	.mrow:hover:not(:disabled) { border-color: var(--td-accent); }
	.mrow:disabled { opacity: 0.55; cursor: default; }
	.mmain { display: flex; flex-direction: column; gap: 1px; min-width: 0; }
	.mt { font-size: 13.5px; font-weight: 600; }
	.ma { font-size: 11.5px; color: var(--td-muted-2); }
	.mmeta { display: flex; align-items: center; gap: 7px; flex: none; }
	.mpill {
		font-size: 10px; font-weight: 700; letter-spacing: 0.05em; text-transform: uppercase;
		padding: 2px 7px; border-radius: 999px;
		border: 1px solid var(--td-border); color: var(--td-muted-3);
	}
	.mpill.lit { border-color: var(--td-accent); color: var(--td-accent); }
	.mn { font-size: 11px; color: var(--td-muted-3); font-variant-numeric: tabular-nums; }
	/* A hidden gutter keeps its column, or every line shifts when it is turned
	   off and the list reads as a different lyric. */
	.lpn.empty { visibility: hidden; }

	.lp {
		border-radius: 22px;
		overflow: hidden;
		display: flex;
		flex-direction: column;
		max-height: 620px;
	}
	.lphead { padding: 14px 16px 12px; border-bottom: 1px solid var(--td-border); }
	.lptop { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; }
	.lpttl { font-size: 15px; margin: 0; }
	.lpsrc { font-size: 10.5px; color: var(--td-muted-3); }
	.lppick { display: flex; align-items: center; gap: 6px; margin-top: 9px; }
	.lppick .input { min-height: 34px; font-size: 13px; padding: 5px 10px; flex: 1; min-width: 0; }
	.lpone {
		flex: 1;
		min-width: 0;
		font-size: 13px;
		color: var(--td-muted-2);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.lpout {
		width: 30px;
		height: 30px;
		flex: none;
		display: grid;
		place-items: center;
		border-radius: 999px;
		border: 1px solid var(--td-border);
		background: var(--td-surface2);
		color: var(--td-muted-2);
		text-decoration: none;
		font-size: 13px;
	}
	.lpout:hover { color: var(--td-accent); border-color: var(--td-accent); }
	.lpbody { overflow-y: auto; padding: 8px 8px 10px; flex: 1; min-height: 0; }
	.lpmsg { font-size: 12.5px; color: var(--td-muted); margin: 10px 8px; line-height: 1.5; }
	.lpbreak { height: 9px; }
	.lpline {
		display: grid;
		grid-template-columns: 30px minmax(0, 1fr) 16px;
		gap: 8px;
		align-items: start;
		width: 100%;
		text-align: left;
		padding: 6px 8px;
		border: none;
		border-radius: 11px;
		background: none;
		cursor: pointer;
		font-family: inherit;
		color: var(--td-text);
	}
	.lpline:hover { background: var(--td-surface2); }
	.lpline.on { background: color-mix(in srgb, var(--td-accent) 14%, transparent); }
	.lpline.anchor { box-shadow: inset 2px 0 0 var(--td-accent); }
	.lpn {
		font-size: 10.5px;
		color: var(--td-muted-3);
		text-align: right;
		padding-top: 3px;
		font-variant-numeric: tabular-nums;
	}
	.lptext { font-size: 13px; line-height: 1.45; min-width: 0; }
	.lpin {
		display: block;
		font-size: 10.5px;
		color: var(--td-accent);
		margin-top: 2px;
	}
	.lpq {
		display: grid;
		place-items: center;
		width: 16px;
		height: 16px;
		margin-top: 2px;
		border-radius: 999px;
		background: var(--td-accent);
		color: #fbf5ea;
		font-size: 10px;
		opacity: 0;
	}
	.lpline:hover .lpq,
	.lpline:focus-visible .lpq { opacity: 1; }
	.lpfoot {
		border-top: 1px solid var(--td-border);
		background: var(--td-surface2);
		padding: 10px 14px;
		font-size: 11.5px;
		color: var(--td-muted-2);
		line-height: 1.5;
		display: flex;
		align-items: center;
		gap: 9px;
		flex-wrap: wrap;
	}
	.lpfix {
		font-size: 12px;
		padding: 6px 13px;
		border-radius: 999px;
		border: 1px solid var(--td-border);
		background: var(--td-surface);
		color: var(--td-muted-2);
		cursor: pointer;
		white-space: nowrap;
		font-family: inherit;
	}
	.lpfix:hover { color: var(--td-accent); border-color: var(--td-accent); }
	.lpnote {
		margin: 0;
		padding: 10px 14px;
		font-size: 11.5px;
		color: var(--td-muted);
		line-height: 1.5;
		border-top: 1px solid var(--td-border);
	}
	.scrim { position: fixed; inset: 0; background: rgba(0, 0, 0, 0.45); z-index: 40; }
	.dialog {
		position: fixed;
		z-index: 41;
		top: 50%;
		left: 50%;
		transform: translate(-50%, -50%);
		width: min(680px, 92vw);
		max-height: 86vh;
		overflow: auto;
		background: var(--td-surface);
		/* Stated rather than inherited: a dialog that takes its text colour from
		   whatever it opened over reads correctly in exactly one theme, which is
		   the `.dialog` bug v0.76.4 found in app.css. */
		color: var(--td-text);
		border: 1px solid var(--td-border);
		border-radius: 22px;
		padding: 22px;
	}
	.dlgsub { font-size: 12.5px; color: var(--td-muted); margin: 0 0 14px; line-height: 1.55; }
	.dlgrow { display: flex; align-items: center; gap: 8px; margin-top: 14px; flex-wrap: wrap; }
	.fl { display: block; font-size: 11.5px; color: var(--td-muted-2); margin: 12px 0 5px; }
	kbd {
		font: 500 10.5px ui-monospace, Menlo, monospace;
		background: var(--td-sidebar, var(--td-surface));
		border: 1px solid var(--td-border);
		padding: 1px 5px;
		border-radius: 5px;
	}
</style>
