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
	import { api, lyricKey } from '$lib/api';
	import type { LyricDoc } from '$lib/api';
	import { deck, watchNowPlaying } from '$lib/nowplaying';

	let doc: LyricDoc | null = null;
	let loading = true;
	let error = '';
	let artist = '';
	let title = '';
	let album = '';

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

	async function load() {
		loading = true;
		error = '';
		try {
			doc = await api.lyrics(artist, title, album || undefined);
			album = doc.album ?? '';
			if (cursor == null) cursor = doc.lines.find((l) => l.text.trim())?.n ?? null;
		} catch (e) {
			error = clean(e);
		} finally {
			loading = false;
		}
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

	// The line the clock is on: the last one whose cue has passed.
	//
	// `$deck.position` is **named in the statement**, not read through a call — a
	// reactive block re-runs on the variables it syntactically references and
	// does not trace them through a function. That is the Svelte trap this
	// codebase has walked into four times now.
	$: if (deckOn && doc) {
		const at = $deck.position;
		let n: number | null = null;
		for (const l of doc.lines) if (l.at != null && l.at <= at + 0.25) n = l.n;
		if (n != null) cursor = n;
	}

	/** Only the lines with words. A stanza break is a numbered line — that is
	 *  what makes an index stable — but the theatre reads one line at a time,
	 *  and stepping onto a blank one would be stepping onto nothing. */
	$: spoken = (doc?.lines ?? []).filter((l) => l.text.trim());
	$: at = cursor == null ? -1 : spoken.findIndex((l) => l.n === cursor);
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
	$: ribbonLabel = deckOn ? mmss($deck.position) : current?.at != null ? mmss(current.at) : `${at + 1}/${spoken.length}`;

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

	/** Hand the cursor back to the deck.
	 *
	 *  `steering` had no way out: pointing at a line set it, and **arriving with
	 *  `?line=` set it too** — so opening the reader from the deck strip, or
	 *  from a `[[line:…]]` chip, pinned the words at that line and the only way
	 *  to get them moving again was to edit the URL. Reported exactly that way.
	 *
	 *  Arriving steered is still right: you clicked a particular line and being
	 *  yanked off it by the playhead would be worse. What was missing is the way
	 *  back, and it is one press or the Escape key. */
	function followDeck() {
		steering = false;
		// Jump straight to where the music is rather than waiting for the next
		// cue to pass, which on a long line is an uncomfortable few seconds of
		// looking like nothing happened.
		if (onDeck && doc) {
			const at = $deck.position;
			let n: number | null = null;
			for (const l of doc.lines) if (l.at != null && l.at <= at + 0.25) n = l.n;
			if (n != null) cursor = n;
		}
	}

	/** Whether handing it back is on offer: the deck is on this track and
	 *  reporting a real playhead, and we are not already following. An
	 *  estimated position must not drive a highlight that looks measured. */
	$: canFollow = onDeck && ($deck.track?.position_known ?? false) && steering;

	function step(by: number) {
		if (!spoken.length) return;
		const next = at < 0 ? 0 : at + by;
		if (next < 0 || next >= spoken.length) return;
		pointAt(spoken[next].n);
	}

	function onKey(e: KeyboardEvent) {
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
		const q = new URLSearchParams({ artist, title: t });
		if (album) q.set('album', album);
		return `/lyrics?${q}`;
	}

	/** Hand the line to Liner Notes, which is where writing happens.
	 *
	 *  The **track**, not the record: the short `[[line:12]]` is read against
	 *  what the note is about, so it only means line twelve of this song inside
	 *  a note about this song. */
	function quoteHref(n: number): string {
		const q = new URLSearchParams({ kind: 'track', name: title, artist, quote: `[[line:${n}]]` });
		return `/notes?${q}`;
	}

	/** The annotate-lines mode, opened on this track and this line. */
	function pinHref(n: number): string {
		const q = new URLSearchParams({ kind: 'track', name: title, artist, pin: String(n) });
		if (album) q.set('album', album);
		return `/notes?${q}`;
	}

	/** Which track comes before and after, so the reader can walk the record. */
	$: siblings = doc?.tracklist ?? [];
	$: mine = siblings.findIndex(
		(t) => t.title.trim().toLowerCase() === (doc?.title ?? '').trim().toLowerCase()
	);
	$: prev = mine > 0 ? siblings[mine - 1] : null;
	$: next = mine >= 0 && mine < siblings.length - 1 ? siblings[mine + 1] : null;

	onMount(async () => {
		const p = new URLSearchParams(window.location.search);
		artist = p.get('artist') ?? '';
		title = p.get('title') ?? '';
		album = p.get('album') ?? '';
		const l = Number(p.get('line'));
		cursor = Number.isFinite(l) && l > 0 ? l : null;
		if (cursor != null) steering = true;
		if (!artist || !title) {
			error = 'No track named — open a lyric from a listen or a note.';
			loading = false;
			return;
		}
		await load();
		unwatch = watchNowPlaying();
	});

	// An async `onMount` returns a promise, which Svelte cannot use as a
	// teardown — so the watch would outlive the page and hold the shared poll
	// open for ever. Released here instead, the same trap v0.80.0 records.
	onDestroy(() => unwatch?.());
</script>

<svelte:head><title>{title ? `${title} — Lyrics` : 'Lyrics'} — Tapedeck</title></svelte:head>
<svelte:window on:keydown={onKey} />

{#if loading}
	<div class="shell"><div class="stage plain">Reading…</div></div>
{:else if error && !doc}
	<div class="shell"><div class="stage plain">{error}</div></div>
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
						most-played first — or LRCLIB may simply not have it.
					</p>
				{:else}
					<div class="prevlines">
						{#each above as l, i (l.n)}
							<button class="ghost g{above.length - i}" on:click={() => pointAt(l.n)}>{l.text}</button>
						{/each}
					</div>

					{#if current}
						<div class="nowline">
							<span class="nowrule"></span>
							<div class="nowtext">{current.text}</div>
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
							<button class="ghost b{i + 1}" on:click={() => pointAt(l.n)}>{l.text}</button>
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
					{#if deckOn}
						<span class="wpill good"><span class="blip"></span>Following</span>
					{:else if canFollow}
						<!-- The way back. Drawn only when the deck really is on this
						     track with a measured playhead, so it never offers to
						     follow something it cannot. -->
						<button class="wpill follow" on:click={followDeck} title="Follow the deck again (Esc)">
							<span class="blip"></span>Follow the deck
						</button>
					{/if}
				</div>

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
		{#if doc.source}Words via {doc.source}.{:else}Where these words came from was not recorded.{/if}
		Cached here so a note can point at a line — they are not ours, and they never leave this
		instance: not in a backup, not in an export, not to anyone you have patched with.
		{#if !doc.timed && spoken.length}
			This lyric is untimed, so the ribbon is your place in the words rather than in the track —
			↑ and ↓ move it.
		{/if}
	</p>
{/if}

<style>
	/* The pill as a control. Same size and shape as its neighbours so the row
	   still reads as a row of pills, with the accent and a pointer to say this
	   one does something. */
	/* Literal, like every other colour on this stage. The first version used
	   `var(--td-accent)` — in the one file whose own note says a token here
	   follows the theme onto a ground that does not. It happens to be the same
	   orange in both today, which is exactly how that trap survives review. */
	.wpill.follow {
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
	.wpill.follow:focus-visible { background: rgba(198, 113, 57, 0.14); }
	.wpill.follow .blip { animation: none; }

	/* **Dark in both themes, on purpose**, which is the one screen in this app
	   where that is true — see the note at the top of this file. Every colour is
	   therefore literal rather than a token: a token would follow the theme and
	   put cream text on a cream stage in light mode, which is the `--td-panel`
	   trap. Stating them all is what makes that impossible here. */
	.shell { max-width: 1460px; margin: 0 auto; }
	.stage {
		border-radius: 22px;
		overflow: hidden;
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
