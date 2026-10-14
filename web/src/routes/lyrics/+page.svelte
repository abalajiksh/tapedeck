<script lang="ts">
	/**
	 * The lyric reader — the design's **teleprompter theatre**, artboard 1b of
	 * `temp/Lyrics Final.dc.html`.
	 *
	 * **Deliberately dark in both themes, which is the one place in this
	 * application that is true.** Everywhere else a hardcoded palette is the
	 * `--td-panel` trap; here it is the design, and the reason is that this
	 * screen is not a page of the app so much as a place you go to read. It has
	 * its own chrome, its own way out (the close button), and nothing else on
	 * it. Ashwin's call, 2026-08-26.
	 *
	 * Three things carry the reading, and all three are the design's:
	 *
	 * * **One enlarged line** at 52px, with the neighbours *fading and
	 *   shrinking* away from it — not merely dimmed. The gradient is what makes
	 *   a wall of text read as a place in a song.
	 * * **The ribbon** down the left: where you are, as a position rather than a
	 *   number.
	 * * **Untimed lyrics use the same theatre.** The design says so outright:
	 *   the ribbon becomes a scroll position, the enlarged line follows the
	 *   cursor or the keyboard rather than the clock, and every affordance stays
	 *   put. There is one reader, not two.
	 *
	 * **Pins are not written here.** *Quote this line* and *Pin a note* both
	 * hand off to Liner Notes, which is where the design puts the writing — the
	 * annotate-lines mode with the margin and the connector dots. This screen
	 * shows what is pinned on the track and otherwise stays out of the way.
	 *
	 * Two elements are absent rather than deferred: the **ES** toggle, because
	 * LRCLIB returns the words and nothing else, and the **version picker**,
	 * because `/api/get` returns one record with no version history. Both would
	 * be controls over data that does not exist.
	 */
	import { onMount, onDestroy } from 'svelte';
	import { page } from '$app/stores';
	import { api, lyricKey, romanOriginal } from '$lib/api';
	import type { LyricDoc, LyricLine } from '$lib/api';
	import { syncUrl } from '$lib/url';
	import { deck, watchNowPlaying } from '$lib/nowplaying';
	import LyricVersions from '$lib/components/LyricVersions.svelte';
	import RomanLine from '$lib/components/RomanLine.svelte';
	import { lyricView, setLyricView, VIEW_LABEL } from '$lib/lyricview';
	import type { LyricView } from '$lib/lyricview';

	let doc: LyricDoc | null = null;
	let loading = true;
	let error = '';

	/** **The address is read reactively, not once on mount**, and both halves of
	 *  that matter.
	 *
	 *  A sibling-track link is a navigation to *this same route*, so Svelte
	 *  keeps the component alive and only the page store changes — reading the
	 *  query string in `onMount` meant clicking a track in the wing changed the
	 *  address and not the words. And it is what lets the reader move between
	 *  live and addressed without a reload. */
	$: q = $page.url.searchParams;
	$: wantArtist = (q.get('artist') ?? '').trim();
	$: wantTitle = (q.get('title') ?? '').trim();
	$: wantAlbum = (q.get('album') ?? '').trim();

	/** **Live is the default, and the reason is the bug it fixes.** The reader
	 *  used to be addressed only — `?artist=…&title=…` — so it went on showing
	 *  whatever the URL named after the next song started, and the only way out
	 *  was to edit the query string.
	 *
	 *  So a bare `/lyrics` follows the deck: it asks `/api/v1/lyrics/live` once,
	 *  and again whenever the deck's track changes. An address still wins when
	 *  there is one, because a `[[line:…]]` chip and a history row are naming a
	 *  particular song on purpose — and *Follow the deck* hands it back. */
	$: live = !wantArtist || !wantTitle;

	/** What `doc` is about, and what the deck is on. Compared as folded keys,
	 *  which is the same fold the server stores lyrics under. */
	$: deckKey = $deck.playing && $deck.track ? lyricKey($deck.track.artist, $deck.track.title) : '';
	$: addressed = live ? '' : lyricKey(wantArtist, wantTitle);

	/** The key of whatever we have asked for. A sentinel to begin with, so the
	 *  first pass always loads — including in live mode with an empty deck,
	 *  where the target key is legitimately the empty string. */
	let wanted = '\u0000';
	let ready = false;
	/** Set when the deck has nothing on and the reader has nothing to show. */
	let idle = false;

	/** Fetching the words on demand — the button, not the backfill. */
	let looking = false;
	let lookMsg = '';

	/** *Other versions*: the other LRCLIB records for this track, and the way
	 *  to hold one of them instead. Offered whether or not words are held —
	 *  the reader had no way to it at all once any were, which is exactly when
	 *  somebody is looking at the wrong ones. */
	let versions = false;
	let versionMsg = '';

	/** What the words are drawn as. Only a lyric the server romanized offers
	 *  a choice; anything else is its own script, whatever the setting says. */
	$: view = (doc?.romanization ? $lyricView : 'original') as LyricView;
	/** Words held in Latin letters with no romanization behind them — the
	 *  case an original-script companion is for (*Other versions* offers it). */
	$: latinHeld = !!doc && !doc.romanization && (doc.lines ?? []).some((l) => l.text.trim());

	/** Japanese long vowels, toggled in the reader and kept as a preference;
	 *  the words are fetched again in the new style. */
	let restyling = false;
	async function toggleMacrons() {
		if (!doc?.romanization || restyling) return;
		restyling = true;
		try {
			await api.putPreferences({ lyrics_macrons: !doc.romanization.macrons });
			if (live) await loadLive();
			else await loadAddressed();
		} finally {
			restyling = false;
		}
	}

	async function forgetCompanion() {
		if (!doc) return;
		await api.clearCompanion(doc.artist, doc.title).catch(() => {});
		if (live) await loadLive();
		else await loadAddressed();
		versionMsg = 'Forgotten. Romanization is made from the words shown again.';
	}

	/** The enlarged line. Moved by the deck when it is following, by the reader
	 *  otherwise — the design's one theatre for timed and untimed alike. */
	let cursor: number | null = null;
	/** Set by pointing at a line by hand: the deck stops driving the cursor, or
	 *  the next tick would drag the reader back and it would look broken. */
	let steering = false;
	let unwatch: (() => void) | undefined;

	const clean = (e: unknown) => String(e).replace(/^API \d+: /, '');

	function mmss(s: number): string {
		const m = Math.floor(s / 60);
		return `${m}:${String(Math.floor(s - m * 60)).padStart(2, '0')}`;
	}

	/** Start on the first line with words in it, unless a `?line=` said
	 *  otherwise — a blank line is numbered, which is what makes an index
	 *  stable, but it is not a place to open on. */
	function settle() {
		if (cursor == null) cursor = doc?.lines.find((l) => l.text.trim())?.n ?? null;
	}

	/** The addressed reader: a particular song, because something named it. */
	async function loadAddressed() {
		loading = true;
		error = '';
		lookMsg = '';
		versionMsg = '';
		try {
			doc = await api.lyrics(wantArtist, wantTitle, wantAlbum || undefined);
			idle = false;
			settle();
		} catch (e) {
			error = clean(e);
		} finally {
			loading = false;
		}
	}

	/** Whatever is on the deck, in one request.
	 *
	 *  The playhead comes back with it so the first paint lands on the right
	 *  line rather than at the top of the song — a cold open (a fresh tab, the
	 *  shelf's *Read the words*) has no deck poll behind it yet. */
	async function loadLive() {
		loading = !doc;
		error = '';
		lookMsg = '';
		versionMsg = '';
		try {
			const r = await api.liveLyrics();
			if (!r.playing) {
				// **The last words stay on screen.** A deck legitimately empties
				// between tracks — a Plex tick with no sessions clears it — and
				// blanking the theatre every time would flicker. `idle` is what
				// the pill reads; the next track replaces the words outright.
				idle = true;
				wanted = '';
				return;
			}
			idle = false;
			doc = r;
			wanted = lyricKey(r.artist, r.title);
			// Land on the line the music is on, when it can be known.
			cursor = null;
			steering = false;
			if (r.position_known) lineAt(r.position);
			settle();
		} catch (e) {
			if (!doc) error = clean(e);
		} finally {
			loading = false;
		}
	}

	/** Look for this track's words now rather than waiting for the trickle.
	 *
	 *  The backfill runs at a few tracks a minute, most-played first, so the one
	 *  in front of you may be hours down it. A deliberate press is not a page
	 *  load, which is the whole reason this may ask LRCLIB where opening the
	 *  reader may not. */
	async function lookNow() {
		if (!doc || looking) return;
		looking = true;
		lookMsg = '';
		try {
			const r = await api.fetchLyricsNow({
				artist: doc.artist,
				title: doc.title,
				album: doc.album ?? undefined,
				// Only when the deck is on this track: LRCLIB matches on length,
				// and handing it another song's is how a lookup comes back with
				// somebody else's words.
				duration: onDeck ? ($deck.track?.duration ?? undefined) : undefined
			});
			if (r.unavailable) {
				// Asked to wait is not the same as nothing found, and must not
				// be reported as it.
				lookMsg = r.why ?? 'Could not ask just now — try again shortly.';
				return;
			}
			doc = r;
			cursor = null;
			settle();
			if (!r.lines.length && !r.instrumental) {
				lookMsg = 'Nobody has words for this one yet. Worth trying again another day.';
			}
		} catch (e) {
			lookMsg = clean(e);
		} finally {
			looking = false;
		}
	}

	/** The track's own length, to set each LRCLIB record's against: from the
	 *  record's tracklist, or the deck when it is playing this. */
	$: trackDuration =
		doc?.tracklist?.find((t) => t.title.trim().toLowerCase() === doc?.title.trim().toLowerCase())
			?.duration ??
		(onDeck ? ($deck.track?.duration ?? null) : null);

	async function adopted(
		e: CustomEvent<{ renumbered: boolean; lines_before: number; lines_after: number; companion?: boolean }>
	) {
		versions = false;
		cursor = null;
		steering = false;
		if (live) await loadLive();
		else await loadAddressed();
		const r = e.detail;
		// Said after the reload, which clears it for a different track.
		if (r.companion) {
			versionMsg = 'Kept beside your words as their original script — romanization is made from it.';
			if ($lyricView === 'original') setLyricView('romanized');
			return;
		}
		versionMsg = r.renumbered
			? `Swapped — ${r.lines_after} lines where there were ${r.lines_before}, so every quote past the change now points at a different line.`
			: 'Swapped. Kept from now on, and still credited to LRCLIB.';
	}

	// ── Following the deck ──
	//
	// Through the shared `deck` store rather than a poll of its own: it is
	// reference-counted, so the sidebar card, the dashboard and this page
	// together are one request every three seconds, and it ticks between polls.
	//
	// Only ever from a **measured** playhead. `position_known` false means the
	// server is counting up from when a submission arrived rather than reading a
	// position, and an estimate must not drive a highlight that looks measured.
	$: onDeck =
		$deck.playing &&
		!!$deck.track &&
		!!doc &&
		lyricKey($deck.track.artist, $deck.track.title) === lyricKey(doc.artist, doc.title);
	$: deckOn = onDeck && ($deck.track?.position_known ?? false) && !steering;

	/** Put the cursor where the clock is: the last line whose cue has passed.
	 *
	 *  A function so the reactive block below and `loadLive` can both use it —
	 *  but note the block still **names `$deck.position` itself**. A reactive
	 *  statement re-runs on the variables it syntactically references and does
	 *  not trace them through a call, which is the Svelte trap this codebase has
	 *  now walked into five times. */
	function lineAt(secs: number) {
		if (!doc) return;
		let n: number | null = null;
		// **Only lines that have words.** A synced lyric cues its instrumental
		// gaps as empty lines — `[01:36.68]` with nothing after it, which is
		// the conventional way an LRC marks one, and Teardrop's LRCLIB record
		// carries five. Considering them here landed the cursor on a line the
		// theatre cannot show, and the stage went **completely blank** for the
		// length of every break: no lit line, no ghosts either side, and not
		// even the "nothing held" message, since there are words. The rule was
		// already written on `spoken` below — stepping onto a blank line is
		// stepping onto nothing — and this is the place that did not follow it.
		//
		// So during a break the last line sung stays lit while the ribbon runs
		// on, which is what a teleprompter does and what the words are for.
		for (const l of doc.lines) if (l.at != null && l.at <= secs + 0.25 && l.text.trim()) n = l.n;
		if (n != null) cursor = n;
	}

	$: if (deckOn && doc?.timed) lineAt($deck.position);

	/** **An untimed lyric scrolls too, and it is honest about being a guess.**
	 *
	 *  The design's rule is that the ribbon becomes your place in the *words*
	 *  when there are no cues, and moving it by hand is right for reading a
	 *  lyric with nothing playing. It is the wrong answer while the record is
	 *  on: the words then simply do not follow, which is what the theatre is
	 *  for.
	 *
	 *  So the clock drives it proportionally — but only where there is a real
	 *  playhead *and* a length to divide by, and the ribbon says `≈` for as long
	 *  as it is doing so. This estimates the **line**, never the position: the
	 *  seconds under the ribbon stay the measurement they always were. Verses
	 *  and instrumental breaks make it wrong in the middle of a song, which is
	 *  exactly why it is marked rather than presented as a cue. */
	$: if (deckOn && doc && !doc.timed && spoken.length && ($deck.track?.duration ?? 0) > 0) {
		const frac = Math.min(1, Math.max(0, $deck.position / ($deck.track?.duration ?? 1)));
		cursor = spoken[Math.min(spoken.length - 1, Math.floor(frac * spoken.length))].n;
	}

	/** Whether the line being lit is estimated rather than cued. */
	$: estimating = deckOn && !!doc && !doc.timed && spoken.length > 0;

	/** Only the lines with words. A stanza break is a numbered line — that is
	 *  what makes an index stable — but the theatre reads one line at a time,
	 *  and stepping onto a blank one would be stepping onto nothing. */
	$: spoken = (doc?.lines ?? []).filter((l) => l.text.trim());
	$: at = resolveAt(cursor, spoken);

	/** Where the cursor sits among the lines that have words.
	 *
	 *  Resolved rather than looked up, because a cursor can legitimately name a
	 *  line the theatre has nothing to show for: a cued instrumental gap (the
	 *  bug above, closed at its source but reachable from `?line=` too), or a
	 *  pin on a line some later correction blanked. Both used to return `-1`,
	 *  which empties `current`, `above` and `below` together and leaves a blank
	 *  stage — the one failure this screen cannot explain to anybody looking at
	 *  it.
	 *
	 *  Falls back to the last line before the cursor, and to the first line
	 *  when there is none: an LRC that opens on a cued blank intro should show
	 *  what is coming rather than nothing at all. */
	function resolveAt(c: number | null, sp: LyricLine[]): number {
		if (c == null || !sp.length) return -1;
		const exact = sp.findIndex((l) => l.n === c);
		if (exact >= 0) return exact;
		let before = -1;
		for (let i = 0; i < sp.length; i++) if (sp[i].n < c) before = i;
		return before >= 0 ? before : 0;
	}
	$: current = at >= 0 ? spoken[at] : null;
	/** Three above and four below, fading and shrinking away — the design's own
	 *  counts, and what turns a list into a place in a song. */
	$: above = at > 0 ? spoken.slice(Math.max(0, at - 3), at) : [];
	$: below = at >= 0 ? spoken.slice(at + 1, at + 5) : [];
	$: pinnedLines = (doc?.lines ?? []).filter((l) => l.pin);

	/** How far through — the ribbon's fill.
	 *
	 *  The playhead when the deck is driving; otherwise the position *in the
	 *  lyric*, which is the design's "the ribbon becomes a plain scroll
	 *  position". Two different measurements, and the label under it says which. */
	$: progress = (() => {
		if (deckOn && $deck.track?.duration) {
			return Math.min(1, $deck.position / $deck.track.duration);
		}
		return spoken.length > 1 && at >= 0 ? at / (spoken.length - 1) : 0;
	})();
	// `≈` when the *line* is estimated from the playhead rather than cued. The
	// seconds themselves are still a measurement — see `estimating`.
	$: ribbonLabel = deckOn
		? `${estimating ? '≈' : ''}${mmss($deck.position)}`
		: current?.at != null
			? mmss(current.at)
			: `${at + 1}/${spoken.length}`;

	/** The coordinate, as the design writes it: the track's position on the
	 *  record where one is known, and the line either way. */
	$: trackNo = doc?.tracklist?.find(
		(t) => t.title.trim().toLowerCase() === (doc?.title ?? '').trim().toLowerCase()
	)?.track_number;
	$: mark = trackNo ? `${trackNo}` : '';
	$: coord = current
		? `${trackNo ? `${trackNo} · ` : ''}line ${current.n}${current.at != null ? ` · ${mmss(current.at)}` : ''}`
		: '';

	function pointAt(n: number) {
		cursor = n;
		steering = true;
	}

	/** Hand the reader back to the deck — **both senses of it**.
	 *
	 *  There are two ways the theatre stops following, and one button that ends
	 *  either. *Steering* is the cursor: pointing at a line stops the clock
	 *  moving it, and arriving with `?line=` does the same, which is right —
	 *  you clicked a particular line, and being yanked off it would be worse.
	 *  *Addressed* is the whole page: a `[[line:…]]` chip or a history row names
	 *  a song, and the reader stays on it while the music moves on.
	 *
	 *  Merging them is not a shortcut. The label says "follow the deck", and a
	 *  press that un-froze the cursor while leaving the reader on last week's
	 *  song would be a button that half did what it said. */
	function followDeck() {
		steering = false;
		if (!live) {
			// Leaving the address is what makes the theatre follow the *music*
			// rather than a song. `syncUrl` rather than `history.replaceState`,
			// which breaks the Back button and logs nothing — the standing rule
			// in `$lib/url`. Replace rather than push: switching mode is not a
			// place somebody wants to come back to.
			syncUrl('/lyrics');
			return;
		}
		// Jump straight to where the music is rather than waiting for the next
		// cue to pass, which on a long line is an uncomfortable few seconds of
		// looking like nothing happened.
		if (onDeck) lineAt($deck.position);
	}

	/** Whether handing it back is on offer.
	 *
	 *  Either the cursor is frozen on a track the deck really is playing with a
	 *  measured playhead, or the reader is addressed and something is on. The
	 *  second case does not require the deck to be on *this* track — that is
	 *  the whole point of it. */
	$: canFollow =
		(onDeck && ($deck.track?.position_known ?? false) && steering) ||
		(!live && $deck.playing && !!$deck.track);

	function step(by: number) {
		if (!spoken.length) return;
		const next = at < 0 ? 0 : at + by;
		if (next < 0 || next >= spoken.length) return;
		pointAt(spoken[next].n);
	}

	function onKey(e: KeyboardEvent) {
		// The dialog has the keyboard while it is open: Escape closes it, and
		// must not also leave the page.
		if (versions) return;
		const t = e.target as HTMLElement | null;
		if (t && /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName)) return;
		if (e.key === 'ArrowDown' || e.key === 'ArrowRight') { e.preventDefault(); step(1); }
		else if (e.key === 'ArrowUp' || e.key === 'ArrowLeft') { e.preventDefault(); step(-1); }
		// Escape unwinds the innermost state first: steering with the arrows is
		// something to get *out of* before leaving the page, and a keyboard user
		// who has just stepped through a verse wants the music back rather than
		// the dashboard. It still closes when there is nothing to hand back.
		else if (e.key === 'Escape') {
			if (canFollow) followDeck();
			else close();
		}
	}

	function close() {
		// Back where they came from when that is inside the app, and to the
		// dashboard otherwise — a reader opened from a fresh tab has no history
		// to go back to, and leaving it on a dead Back is worse than a guess.
		if (window.history.length > 1) window.history.back();
		else window.location.href = '/';
	}

	/** Where a sibling track's reader lives. Carries the album so its own
	 *  tracklist comes back without another resolution. */
	function readerHref(t: string): string {
		const p = new URLSearchParams({ artist: doc?.artist ?? '', title: t });
		if (doc?.album) p.set('album', doc.album);
		return `/lyrics?${p}`;
	}

	/** Hand the line to Liner Notes, which is where writing happens.
	 *
	 *  The **track**, not the record: the short `[[line:12]]` is read against
	 *  what the note is about, so it only means line twelve of this song inside
	 *  a note about this song. */
	function quoteHref(n: number): string {
		const p = new URLSearchParams({
			kind: 'track',
			name: doc?.title ?? '',
			artist: doc?.artist ?? '',
			quote: `[[line:${n}]]`
		});
		return `/notes?${p}`;
	}

	/** The annotate-lines mode, opened on this track and this line. */
	function pinHref(n: number): string {
		const p = new URLSearchParams({
			kind: 'track',
			name: doc?.title ?? '',
			artist: doc?.artist ?? '',
			pin: String(n)
		});
		if (doc?.album) p.set('album', doc.album);
		return `/notes?${p}`;
	}

	/** Which track comes before and after, so the reader can walk the record. */
	$: siblings = doc?.tracklist ?? [];
	$: mine = siblings.findIndex(
		(t) => t.title.trim().toLowerCase() === (doc?.title ?? '').trim().toLowerCase()
	);
	$: prev = mine > 0 ? siblings[mine - 1] : null;
	$: next = mine >= 0 && mine < siblings.length - 1 ? siblings[mine + 1] : null;

	/** What the reader should be showing: the deck's track in live mode, the
	 *  addressed one otherwise.
	 *
	 *  **This is the whole of "it switches on its own".** The deck store already
	 *  polls once every three seconds for the sidebar and the dashboard; all
	 *  this does is notice when the track behind that poll has changed. So a
	 *  song change costs **one** request for the new words, and a poll that
	 *  finds the same song costs nothing — where re-asking `/lyrics/live` on a
	 *  timer would pull a document with a tracklist and every pin in it twenty
	 *  times a minute. */
	$: target = live ? deckKey : addressed;
	$: if (ready && target !== wanted) {
		wanted = target;
		void (live ? loadLive() : loadAddressed());
	}

	onMount(async () => {
		// `?line=` still arrives steered, and that is deliberate: somebody
		// clicked a particular line. `followDeck` is the way back out, which is
		// what it did not have.
		const l = Number($page.url.searchParams.get('line'));
		cursor = Number.isFinite(l) && l > 0 ? l : null;
		if (cursor != null) steering = true;

		// The first load happens here rather than being left to the reactive
		// block, because in live mode with a deck that has not been polled yet
		// the target key is legitimately the empty string — and a page that
		// waited for the first poll would open blank for up to three seconds
		// when the server could have answered immediately.
		if (live) await loadLive();
		else await loadAddressed();
		wanted = live ? (doc ? lyricKey(doc.artist, doc.title) : '') : addressed;
		ready = true;
		unwatch = watchNowPlaying();
	});

	// An async `onMount` returns a promise, which Svelte cannot use as a
	// teardown — so the watch would outlive the page and hold the shared poll
	// open for ever. Released here instead, the same trap v0.80.0 records.
	onDestroy(() => unwatch?.());
</script>

<svelte:head><title>{doc ? `${doc.title} — Lyrics` : 'Lyrics'} — Tapedeck</title></svelte:head>
<svelte:window on:keydown={onKey} />

{#if loading && !doc}
	<div class="shell"><div class="stage plain">Reading…</div></div>
{:else if error && !doc}
	<div class="shell"><div class="stage plain">{error}</div></div>
{:else if !doc}
	<!-- Live, with an empty deck. Not an error and not a blank page: the reader
	     is waiting, and it says which — the words arrive on their own when
	     something starts. -->
	<div class="shell">
		<div class="stage plain">
			<p>Nothing playing.</p>
			<p class="plainsub">
				The words follow the deck — start something and they will appear here. To read a
				particular song, open it from a listen or a note.
			</p>
		</div>
	</div>
{:else if doc}
	<div class="shell">
		<div class="stage">
			<!-- The record turning behind it. Decoration, and it says so: the
			     reader has no transport and this is not one. -->
			<svg class="platter" viewBox="0 0 400 400" width="600" height="600" aria-hidden="true">
				<g class="spin" class:still={!deckOn}>
					<circle cx="200" cy="200" r="196" fill="#000" />
					<circle cx="200" cy="200" r="160" fill="none" stroke="#5c4c39" stroke-width="1.5" />
					<circle cx="200" cy="200" r="128" fill="none" stroke="#5c4c39" stroke-width="1.5" />
					<circle cx="200" cy="200" r="96" fill="none" stroke="#5c4c39" stroke-width="1.5" />
					<circle cx="200" cy="200" r="62" fill="#c67139" />
					<line x1="200" y1="140" x2="200" y2="166" stroke="#f4e7cf" stroke-width="5" stroke-linecap="round" />
				</g>
			</svg>

			<!-- The ribbon: where you are, as a position rather than a number. -->
			<div class="ribbon" style="--p:{(progress * 100).toFixed(2)}%">
				{#if mark}<span class="rmark">{mark}</span>{/if}
				<div class="rtrack">
					<span class="rfill"></span>
					<span class="rhead"></span>
				</div>
				<span class="rtime">{ribbonLabel}</span>
			</div>

			<div class="centre">
				{#if doc.instrumental}
					<p class="plainmsg">No words — this track is instrumental.</p>
				{:else if !spoken.length}
					<p class="plainmsg">
						Nothing held for this one yet. Lyrics are fetched a few tracks a minute,
						most-played first — so this one may still be a way down the queue.
					</p>
					<!-- The button the queue's pace makes necessary. A deliberate
					     press is not a page load, which is why this may ask
					     LRCLIB where opening the reader may not. -->
					<button class="lookbtn" on:click={lookNow} disabled={looking}>
						{looking ? 'Looking…' : 'Look for the words now'}
					</button>
					<!-- A record filed under another name never comes back from a
					     lookup, which matches on the exact title. -->
					<button class="lookalt" on:click={() => (versions = true)}>Other versions…</button>
					{#if lookMsg}<p class="lookmsg">{lookMsg}</p>{/if}
				{:else}
					<div class="prevlines">
						{#each above as l, i (l.n)}
							<button class="ghost g{above.length - i}" on:click={() => pointAt(l.n)}>
								{#if view !== 'original' && l.roman}
									{#if view === 'both'}<span class="gorig">{romanOriginal(l.roman)}</span>{/if}
									<RomanLine line={l.roman} />
								{:else}{l.text}{/if}
							</button>
						{/each}
					</div>

					{#if current}
						<div class="nowline">
							<span class="nowrule"></span>
							<div class="nowtext">
								{#if view !== 'original' && current.roman}
									<RomanLine line={current.roman} mode={view === 'both' ? 'both' : 'romanized'} />
								{:else}{current.text}{/if}
							</div>
							<div class="nowacts">
								<a class="act primary" href={quoteHref(current.n)}>
									<span class="qm">”</span> Quote this line
								</a>
								<a class="act" href={pinHref(current.n)}>
									<svg viewBox="0 0 24 24" width="13" fill="none" stroke="currentColor" stroke-width="2.75" stroke-linecap="round"><path d="M12 17v5" /><path d="M9 10.8V5a3 3 0 0 1 6 0v5.8l2 3.2H7z" /></svg>
									Pin a note
								</a>
								<span class="coord">{coord}</span>
								{#if (current.quoted ?? 0) > 0}
									<span class="coord lit">quoted in {current.quoted} note{current.quoted === 1 ? '' : 's'}</span>
								{/if}
								{#if doc.plays}
									<span class="coord">played {doc.plays} time{doc.plays === 1 ? '' : 's'}</span>
								{/if}
							</div>
							{#if current.pin}
								<div class="nowpin">{current.pin.excerpt}</div>
							{/if}
						</div>
					{/if}

					<div class="nextlines">
						{#each below as l, i (l.n)}
							<button class="ghost b{i + 1}" on:click={() => pointAt(l.n)}>
								{#if view !== 'original' && l.roman}
									{#if view === 'both'}<span class="gorig">{romanOriginal(l.roman)}</span>{/if}
									<RomanLine line={l.roman} />
								{:else}{l.text}{/if}
							</button>
						{/each}
					</div>
				{/if}
			</div>

			<aside class="wing">
				<div class="whead">
					<div style="min-width:0;">
						<div class="wttl">{doc.title}</div>
						<div class="wsub">{doc.artist}{doc.album ? ` · ${doc.album}` : ''}</div>
					</div>
					<button class="wclose" on:click={close} aria-label="Close the reader">
						<svg viewBox="0 0 24 24" width="13" fill="none" stroke="currentColor" stroke-width="2.75" stroke-linecap="round"><path d="M18 6 6 18M6 6l12 12" /></svg>
					</button>
				</div>

				<div class="wpills">
					{#if doc.timed}<span class="wpill good">Timed</span>{:else if spoken.length}<span class="wpill">Untimed</span>{/if}
					{#if doc.language}<span class="wpill">{doc.language}</span>{/if}
					{#if doc.edited}<span class="wpill">corrected here</span>{/if}
					{#if doc.chosen}<span class="wpill">picked here</span>{/if}
					{#if idle && live}
						<span class="wpill">Deck is quiet</span>
					{/if}
					{#if deckOn}
						<span class="wpill good">
							<span class="blip"></span>{estimating ? 'Following ≈' : 'Following'}
						</span>
					{:else if canFollow}
						<!-- The way back. Drawn only when the deck really is on this
						     track with a measured playhead, so it never offers to
						     follow something it cannot. -->
						<button class="wpill follow" on:click={followDeck} title="Follow the deck again (Esc)">
							<span class="blip"></span>Follow the deck
						</button>
					{/if}
				</div>

				{#if doc.romanization}
					<!-- The script toggle. Remembered per account and shared with the
					     deck strip and the notes panel. -->
					<div class="wseg" role="group" aria-label="Show the words as">
						{#each ['original', 'romanized', 'both'] as const as v}
							<button class="wsegb" class:on={view === v} aria-pressed={view === v} on:click={() => setLyricView(v)}>
								{VIEW_LABEL[v]}
							</button>
						{/each}
					</div>
					{#if doc.romanization.language === 'Japanese' && view !== 'original'}
						<div class="wacts">
							<button
								class="wpill wbtn"
								on:click={toggleMacrons}
								disabled={restyling}
								aria-pressed={doc.romanization.macrons}
								title="Long vowels as macrons (tōkyō) or spelled out (toukyou)"
							>
								{doc.romanization.macrons ? 'Macrons on' : 'Macrons off'}
							</button>
						</div>
					{/if}
					{#if doc.romanization.from_companion}
						<p class="wmsg">
							Romanized from LRCLIB's {doc.romanization.language ?? 'original'} words, kept beside yours.
							<button class="wlink" on:click={forgetCompanion}>Forget it</button>
						</p>
					{/if}
					{#if !doc.romanization.kanji && (doc.romanization.language === 'Japanese' || doc.romanization.language === 'Chinese')}
						<p class="wmsg">This build has no dictionary for kanji, so those are left as written.</p>
					{/if}
				{:else if doc.companion && !doc.companion.lines_up}
					<p class="wmsg">
						The original-script version kept for this track no longer lines up with these words, so it is
						not used.
						<button class="wlink" on:click={forgetCompanion}>Forget it</button>
					</p>
				{/if}
				{#if spoken.length}
					<div class="wacts">
						<button class="wpill wbtn" on:click={() => (versions = true)}>Other versions</button>
					</div>
				{/if}
				{#if versionMsg}<p class="wmsg">{versionMsg}</p>{/if}

				<div class="wrule"></div>

				{#if siblings.length > 1}
					<div class="wlabel">{doc.album ?? 'The record'}</div>
					<div class="wlist">
						{#each siblings as t (t.title)}
							{@const here = t.title.trim().toLowerCase() === doc.title.trim().toLowerCase()}
							{#if here}
								<div class="wrow here">
									<span class="wn">{t.track_number ?? '·'}</span>
									<span class="wt">{t.title}</span>
									<span class="wd">{t.duration ? mmss(t.duration) : ''}</span>
								</div>
							{:else}
								<a class="wrow" class:mute={t.lyrics !== 'words'} href={readerHref(t.title)}>
									<span class="wn">{t.track_number ?? '·'}</span>
									<span class="wt">
										<!-- The separator is inside the expression, not a leading
										     space in the span: Svelte trims whitespace at the start
										     of an element's content, so the two words run together —
										     the `—bought` bug of v0.96.1 in a new place. -->
										{t.title}{#if t.lyrics === 'instrumental'}<span class="winst">{' · instrumental'}</span>{/if}
									</span>
									<span class="wd">{t.duration ? mmss(t.duration) : ''}</span>
								</a>
							{/if}
						{/each}
					</div>
				{/if}

				<span style="flex:1;"></span>

				{#if pinnedLines.length}
					<div class="wpins">
						<div class="wlabel">Pinned on this track</div>
						{#each pinnedLines as l (l.n)}
							<button class="wpin" on:click={() => pointAt(l.n)}>
								<span class="wpinn">line {l.n}</span> — {l.pin?.excerpt}
							</button>
						{/each}
					</div>
				{/if}

				{#if prev || next}
					<div class="wnav">
						{#if prev}
							<a class="wnavbtn" href={readerHref(prev.title)} aria-label="Previous track: {prev.title}">
								<svg viewBox="0 0 24 24" width="14" fill="currentColor"><path d="M19 5 9 12l10 7z" /><rect x="5" y="5" width="3" height="14" rx="1" /></svg>
							</a>
						{:else}<span class="wnavbtn off" aria-hidden="true"></span>{/if}
						{#if next}
							<a class="wnavbtn" href={readerHref(next.title)} aria-label="Next track: {next.title}">
								<svg viewBox="0 0 24 24" width="14" fill="currentColor"><path d="M5 5l10 7-10 7z" /><rect x="16" y="5" width="3" height="14" rx="1" /></svg>
							</a>
						{:else}<span class="wnavbtn off" aria-hidden="true"></span>{/if}
					</div>
				{/if}
			</aside>
		</div>
	</div>

	<p class="foot">
		<!-- Never defaulted to "LRCLIB": provenance is recorded now, and a row
		     from before it was says so rather than claiming a source nothing
		     can vouch for. Naming the *server* matters — words off your own
		     Plex are the pressing you played, with your tagging in them. -->
		<!-- Provenance is a sentence about words, so it is not printed where
		     there are none: "where these words came from was not recorded" over
		     an empty theatre reads as a fact about a lyric nobody holds. -->
		{#if spoken.length || doc.instrumental}
			{#if doc.source}Words via {doc.source}.{:else}Where these words came from was not
				recorded.{/if}
			Cached here so a note can point at a line — they are not ours, and they never leave this
			instance: not in a backup, not in an export, not to anyone you have patched with.
		{:else}
			Words are cached here so a note can point at a line. They are not ours, and they never
			leave this instance: not in a backup, not in an export, not to anyone you have patched
			with.
		{/if}
		{#if estimating}
			This lyric has no cues, so the line is worked out from how far through the track you are —
			an estimate, which is what the ≈ means, and it drifts around long instrumentals. ↑ and ↓
			take it over.
		{:else if !doc.timed && spoken.length}
			This lyric is untimed, so the ribbon is your place in the words rather than in the track —
			↑ and ↓ move it.
		{/if}
	</p>
{/if}

{#if versions && doc}
	<LyricVersions
		artist={doc.artist}
		title={doc.title}
		duration={trackDuration}
		lines={doc.lines.length}
		{latinHeld}
		on:close={() => (versions = false)}
		on:adopted={adopted}
	/>
{/if}

<style>
	/* The pill as a control. Same size and shape as its neighbours so the row
	   still reads as a row of pills, with the accent and a pointer to say this
	   one does something. */
	/* Literal, like every other colour on this stage. The first version used
	   `var(--td-accent)` — in the one file whose own note says a token here
	   follows the theme onto a ground that does not. It happens to be the same
	   orange in both today, which is exactly how that trap survives review. */
	.wpill.follow,
	.wpill.wbtn {
		border-color: rgba(198, 113, 57, 0.75);
		color: #d98c4f;
		cursor: pointer;
		font: inherit;
		font-size: 10.5px;
		font-weight: 700;
		letter-spacing: 0.05em;
		text-transform: uppercase;
	}
	.wpill.follow:hover,
	.wpill.follow:focus-visible,
	.wpill.wbtn:hover,
	.wpill.wbtn:focus-visible { background: rgba(198, 113, 57, 0.14); }
	.wacts { display: flex; gap: 5px; margin-top: -4px; }
	.wseg {
		display: inline-flex;
		align-self: flex-start;
		border: 1px solid rgba(244, 231, 207, 0.18);
		border-radius: 999px;
		padding: 2px;
		gap: 2px;
	}
	.wsegb {
		font: inherit;
		font-size: 10.5px;
		font-weight: 700;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		padding: 4px 10px;
		border-radius: 999px;
		border: none;
		background: none;
		color: #9a8c72;
		cursor: pointer;
	}
	.wsegb:hover,
	.wsegb:focus-visible { color: #e9dcc4; }
	.wsegb.on { background: rgba(198, 113, 57, 0.22); color: #f0b98c; }
	.wlink {
		font: inherit;
		background: none;
		border: none;
		padding: 0;
		color: #d98c4f;
		text-decoration: underline;
		cursor: pointer;
	}
	/* The original above a ghost line's romanization, in "both". */
	.gorig { display: block; font-size: 0.7em; opacity: 0.75; }
	.nowtext { --roman-original: #cdbfa6; }
	.wmsg { margin: -4px 0 0; font-size: 11.5px; line-height: 1.5; color: #9a8c72; }
	.wpill.follow .blip { animation: none; }

	/* **Dark in both themes, on purpose**, which is the one screen in this app
	   where that is true — see the note at the top of this file. Every colour is
	   therefore literal rather than a token: a token would follow the theme and
	   put cream text on a cream stage in light mode, which is the `--td-panel`
	   trap. Stating them all is what makes that impossible here. */
	.shell { max-width: 1460px; margin: 0 auto; }
	.stage {
		border-radius: 22px;
		/* `clip`, not `hidden`. The record drawing is wider than the stage below
		   about 1000px, and `hidden` still makes this a scroll container: focusing
		   anything in the wing (the close button, a track, *Other versions*)
		   scrolled the whole theatre sideways by the overhang. `clip` cuts the
		   same edge and cannot be scrolled. */
		overflow: clip;
		box-shadow: 0 12px 34px rgba(46, 43, 37, 0.18);
		height: min(760px, calc(100vh - 120px));
		min-height: 520px;
		background: radial-gradient(120% 90% at 22% 12%, #241c14 0%, #0f0c08 62%, #0a0806 100%);
		color: #e9dcc4;
		display: flex;
		position: relative;
	}
	.stage.plain {
		align-items: center;
		justify-content: center;
		height: 320px;
		min-height: 0;
		color: #cabda5;
		font-size: 14px;
	}
	.platter {
		position: absolute;
		right: -170px;
		bottom: -210px;
		opacity: 0.16;
		pointer-events: none;
	}
	.spin { transform-origin: 200px 200px; animation: td-platter 1.8s linear infinite; }
	/* Still when nothing is driving it. A turning record over a reader nobody is
	   playing to would be claiming something that is not happening — the same
	   rule the shelf's deck follows. */
	.spin.still { animation: none; }
	@keyframes td-platter { to { transform: rotate(360deg); } }
	@media (prefers-reduced-motion: reduce) {
		.spin { animation: none; }
	}

	.ribbon {
		flex: none;
		width: 86px;
		padding: 26px 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 14px;
		border-right: 1px solid rgba(244, 231, 207, 0.07);
		position: relative;
		z-index: 2;
	}
	.rmark {
		font: 600 9.5px 'Figtree', system-ui, sans-serif;
		letter-spacing: 0.18em;
		color: #6f6559;
		writing-mode: vertical-rl;
	}
	.rtrack {
		flex: 1;
		width: 8px;
		border-radius: 999px;
		background: #221b13;
		position: relative;
	}
	/* The fill and the playhead read `--p` off the ribbon rather than taking an
	   inline height, so the same two elements turn on their side under 900px
	   without a second set of markup to keep in step. */
	.rfill {
		position: absolute;
		left: 0;
		right: 0;
		top: 0;
		height: var(--p);
		border-radius: 999px;
		background: linear-gradient(180deg, #7a8a5e, #e07a55);
		transition: height 0.9s linear;
	}
	.rhead {
		position: absolute;
		left: -6px;
		right: -6px;
		top: var(--p);
		height: 3px;
		border-radius: 999px;
		background: #fbf3e2;
		box-shadow: 0 0 12px rgba(224, 122, 85, 0.8);
		transition: top 0.9s linear;
	}
	.rtime {
		font: 600 10px ui-monospace, Menlo, monospace;
		color: #9a8c72;
		writing-mode: vertical-rl;
		letter-spacing: 0.06em;
	}

	.centre {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		justify-content: center;
		padding: 0 56px;
		position: relative;
		z-index: 2;
		overflow: hidden;
	}
	.plainmsg { color: #9a8c72; font-size: 15px; line-height: 1.6; max-width: 46ch; }
	.plainsub {
		margin-top: 8px;
		max-width: 44ch;
		color: #8a7d68;
		font-size: 13px;
		line-height: 1.6;
		text-align: center;
	}
	/* Literal, like every colour on this stage — a token here would follow the
	   theme onto a ground that does not, which is the `--td-panel` trap this
	   file's own header warns about. Shaped like `.act.primary`, because it is
	   the same kind of thing: the one action offered by this state. */
	.lookbtn {
		margin-top: 16px;
		align-self: flex-start;
		font-family: 'Caprasimo', serif;
		font-size: 13px;
		padding: 9px 18px;
		border: 1px solid transparent;
		border-radius: 999px;
		background: #c67139;
		color: #fbf5ea;
		cursor: pointer;
	}
	.lookbtn:hover:not(:disabled) { background: #b2622d; }
	.lookbtn:disabled { background: rgba(244, 231, 207, 0.09); color: #9a8c72; cursor: default; }
	.lookalt {
		margin-top: 10px;
		align-self: flex-start;
		font: inherit;
		font-size: 12.5px;
		padding: 7px 16px;
		border-radius: 999px;
		border: 1px solid rgba(198, 113, 57, 0.75);
		background: none;
		color: #d98c4f;
		cursor: pointer;
	}
	.lookalt:hover,
	.lookalt:focus-visible { background: rgba(198, 113, 57, 0.14); }
	.lookmsg { margin-top: 10px; max-width: 46ch; color: #9a8c72; font-size: 13px; line-height: 1.55; }
	.prevlines { display: flex; flex-direction: column; gap: 4px; margin-bottom: 26px; }
	.nextlines { display: flex; flex-direction: column; gap: 5px; margin-top: 30px; }
	/* Fading **and shrinking** away from the line being read. Not merely dimmed:
	   the size gradient is what makes a wall of text read as a place in a song,
	   and it is the whole of the design's theatre. */
	.ghost {
		background: none;
		border: none;
		padding: 0;
		text-align: left;
		font-family: inherit;
		cursor: pointer;
		line-height: 1.3;
		width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.ghost:hover { color: #cabda5; }
	.g1 { font-size: 20px; color: #8e8271; }
	.g2 { font-size: 17px; color: #6b6053; }
	.g3 { font-size: 15px; color: #4e463c; }
	.b1 { font-size: 21px; color: #9d9280; }
	.b2 { font-size: 18px; color: #7b7062; }
	.b3 { font-size: 16px; color: #5e564a; }
	.b4 { font-size: 14px; color: #4a433a; }

	.nowline { position: relative; padding-left: 26px; }
	.nowrule {
		position: absolute;
		left: 0;
		top: 6px;
		bottom: 24px;
		width: 4px;
		border-radius: 999px;
		background: linear-gradient(180deg, #e07a55, #c67139);
	}
	.nowtext {
		font-family: 'Caprasimo', serif;
		font-size: clamp(28px, 4vw, 52px);
		line-height: 1.1;
		letter-spacing: -0.025em;
		color: #fdf6e7;
	}
	.nowacts {
		display: flex;
		align-items: center;
		gap: 8px;
		margin-top: 18px;
		flex-wrap: wrap;
	}
	.act {
		display: inline-flex;
		align-items: center;
		gap: 7px;
		font-family: 'Caprasimo', serif;
		font-size: 13px;
		padding: 8px 16px;
		border-radius: 999px;
		border: 1px solid rgba(244, 231, 207, 0.2);
		background: rgba(244, 231, 207, 0.07);
		color: #e9dcc4;
		text-decoration: none;
	}
	.act:hover { background: rgba(244, 231, 207, 0.15); color: #e9dcc4; }
	.act.primary { border-color: transparent; background: #c67139; color: #fbf5ea; }
	.act.primary:hover { background: #b2622d; color: #fbf5ea; }
	.qm { font-size: 15px; line-height: 1; }
	.coord {
		font: 500 10.5px ui-monospace, Menlo, monospace;
		padding: 5px 10px;
		border-radius: 999px;
		background: rgba(244, 231, 207, 0.06);
		color: #8a7d68;
	}
	.coord.lit { background: rgba(198, 113, 57, 0.22); color: #eab488; }
	.nowpin {
		margin-top: 12px;
		max-width: 52ch;
		border-left: 2px solid rgba(198, 113, 57, 0.6);
		padding-left: 12px;
		font-size: 13px;
		line-height: 1.55;
		color: #cabda5;
	}

	.wing {
		flex: none;
		width: 264px;
		border-left: 1px solid rgba(244, 231, 207, 0.07);
		padding: 22px 20px;
		display: flex;
		flex-direction: column;
		gap: 14px;
		position: relative;
		z-index: 2;
		background: rgba(10, 8, 6, 0.5);
		overflow-y: auto;
	}
	.whead { display: flex; justify-content: space-between; align-items: flex-start; gap: 8px; }
	.wttl { font-family: 'Caprasimo', serif; font-size: 15px; color: #f4e7cf; line-height: 1.25; }
	.wsub { font-size: 11.5px; color: #8a7d68; margin-top: 2px; }
	.wclose {
		width: 28px;
		height: 28px;
		flex: none;
		display: grid;
		place-items: center;
		border-radius: 999px;
		border: 1px solid rgba(244, 231, 207, 0.16);
		background: transparent;
		color: #cabda5;
		cursor: pointer;
	}
	.wclose:hover { background: rgba(244, 231, 207, 0.12); }
	.wpills { display: flex; flex-wrap: wrap; gap: 5px; }
	.wpill {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		font-size: 10.5px;
		padding: 4px 10px;
		border-radius: 999px;
		border: 1px solid rgba(244, 231, 207, 0.18);
		background: rgba(244, 231, 207, 0.07);
		color: #a5977f;
	}
	.wpill.good { border-color: transparent; background: rgba(122, 138, 94, 0.22); color: #bacb9b; }
	.blip {
		width: 6px;
		height: 6px;
		border-radius: 999px;
		background: currentColor;
		animation: td-blip 1.6s ease-in-out infinite;
	}
	@keyframes td-blip { 0%, 100% { opacity: 1; } 50% { opacity: 0.25; } }
	@media (prefers-reduced-motion: reduce) {
		.blip { animation: none; }
	}
	.wrule { height: 1px; background: rgba(244, 231, 207, 0.1); }
	.wlabel {
		font: 600 9.5px 'Figtree', system-ui, sans-serif;
		letter-spacing: 0.16em;
		text-transform: uppercase;
		color: #6f6559;
	}
	.wlist { display: flex; flex-direction: column; gap: 1px; }
	.wrow {
		display: flex;
		gap: 9px;
		padding: 6px 9px;
		border-radius: 10px;
		font-size: 12px;
		color: #cabda5;
		text-decoration: none;
	}
	.wrow:hover { background: rgba(244, 231, 207, 0.06); color: #e9dcc4; }
	/* A track nothing is held for is dimmed rather than hidden: it is part of
	   the record, and the reader it opens explains itself. */
	.wrow.mute { color: #6f6559; }
	.wrow.here { background: rgba(198, 113, 57, 0.2); color: #f4d3b4; font-weight: 600; }
	.wn { width: 14px; flex: none; color: inherit; }
	.wt { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.winst { font-size: 10.5px; }
	.wd { flex: none; font-variant-numeric: tabular-nums; }
	.wpins {
		border: 1px solid rgba(244, 231, 207, 0.12);
		border-radius: 16px;
		padding: 12px 14px;
		background: rgba(244, 231, 207, 0.04);
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.wpin {
		background: none;
		border: none;
		padding: 0;
		text-align: left;
		font-family: inherit;
		font-size: 12px;
		line-height: 1.5;
		color: #cabda5;
		cursor: pointer;
	}
	.wpin:hover { color: #f4e7cf; }
	.wpinn { color: #eab488; }
	.wnav { display: flex; gap: 6px; }
	.wnavbtn {
		flex: 1;
		height: 34px;
		display: grid;
		place-items: center;
		border-radius: 999px;
		border: 1px solid rgba(244, 231, 207, 0.18);
		background: rgba(244, 231, 207, 0.06);
		color: #e9dcc4;
	}
	.wnavbtn:hover { background: rgba(244, 231, 207, 0.14); color: #e9dcc4; }
	.wnavbtn.off { opacity: 0.25; }
	.foot {
		max-width: 1460px;
		margin: 12px auto 0;
		font-size: 12px;
		color: var(--td-muted-3);
		line-height: 1.55;
		text-wrap: pretty;
	}
	/* The wing goes under the stage rather than squeezing it: a tracklist in a
	   90px column is unreadable and so is a 52px line beside one. */
	@media (max-width: 900px) {
		.stage { flex-wrap: wrap; height: auto; min-height: 0; }
		.centre { padding: 32px 24px; width: 100%; order: 1; }
		.ribbon { flex-direction: row; width: 100%; height: 54px; padding: 0 22px; border-right: none; border-bottom: 1px solid rgba(244, 231, 207, 0.07); }
		.rtrack { width: auto; height: 8px; flex: 1; }
		.rfill { top: 0; bottom: 0; right: auto; height: auto; width: var(--p); transition: width 0.9s linear; }
		.rhead { top: -6px; bottom: -6px; left: var(--p); right: auto; width: 3px; height: auto; transition: left 0.9s linear; }
		.rmark, .rtime { writing-mode: horizontal-tb; }
		.wing { width: 100%; border-left: none; border-top: 1px solid rgba(244, 231, 207, 0.07); order: 2; }
	}
</style>
