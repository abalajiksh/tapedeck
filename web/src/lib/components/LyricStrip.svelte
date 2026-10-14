<script lang="ts">
	/**
	 * The lyrics drawer on the dashboard deck.
	 *
	 * **The rule that decides the whole component, and it is the design's own:**
	 * on an instrumental, or when no lyrics are held, *the strip is not drawn at
	 * all*. An empty card saying "no lyrics" on every instrumental is a worse
	 * dashboard than one row shorter — so every state below either shows a line
	 * or renders nothing, and there is no "nothing here yet" branch.
	 *
	 * Closed it is one line: the words being sung. Open it is the few lines
	 * either side, dimmed, with the current one lit — the design's well. Drawer
	 * state is a fact about this browser sitting in front of the deck, so
	 * localStorage.
	 *
	 * The **quote** button hands the line to the notes editor as a shortcode; the
	 * **pin** opens the reader on it, where a pin is written. Neither ever puts
	 * the words themselves anywhere — a reference is an index.
	 */
	import { onMount } from 'svelte';
	import { api, romanOriginal } from '$lib/api';
	import type { LyricDoc, LyricLine } from '$lib/api';
	import { lyricView } from '$lib/lyricview';
	import RomanLine from '$lib/components/RomanLine.svelte';

	/** The reader's script choice, shared: Latin letters when chosen and the
	 *  line has them; "both" draws the romanization with the original small
	 *  above it, which is what fits a strip. */
	$: view = doc?.romanization ? $lyricView : 'original';
	const showRoman = (l: LyricLine, v: string) => v !== 'original' && !!l.roman;

	export let artist: string;
	export let title: string;
	export let album: string | null = null;
	/** Seconds into the track, ticked by the shared deck store. */
	export let position = 0;
	/** False when that number is the server's arithmetic rather than a playhead.
	 *  An estimate must not drive a highlight that looks measured, so the strip
	 *  stays closed on the first line rather than following a guess. */
	export let positionKnown = true;

	let doc: LyricDoc | null = null;
	let open = false;
	/** The track the loaded lyric belongs to, so a track change re-fetches
	 *  exactly once rather than on every tick of the position. */
	let loadedFor = '';

	const key = (a: string, t: string) => `${a.trim().toLowerCase()}|${t.trim().toLowerCase()}`;

	async function load(a: string, t: string) {
		const k = key(a, t);
		if (k === loadedFor) return;
		loadedFor = k;
		doc = null;
		try {
			doc = await api.lyrics(a, t, album ?? undefined);
		} catch {
			// A deck that stops rendering because a lyric fetch failed is worse
			// than a deck with no words under it.
			doc = null;
		}
	}

	// Named in the statement, not read through a call — a reactive block re-runs
	// on the variables it references and does not trace them through a function.
	$: void artist, title, load(artist, title);

	$: lines = doc?.lines ?? [];
	/** Everything the strip needs to exist: real words, timed, and a playhead
	 *  worth believing. Untimed lyrics have a reader but no line to be *on*. */
	$: usable = !!doc && !doc.instrumental && (doc.timed ?? false) && positionKnown && lines.length > 0;

	/** The last line whose cue has passed. */
	$: current = (() => {
		if (!usable) return null;
		let found: (typeof lines)[number] | null = null;
		for (const l of lines) if (l.at != null && l.at <= position + 0.25) found = l;
		return found;
	})();

	/** Two either side, which is the design's well. */
	$: window_ = (() => {
		if (!current) return [];
		const i = lines.findIndex((l) => l.n === current!.n);
		return lines.slice(Math.max(0, i - 2), i + 3);
	})();

	function toggle() {
		open = !open;
		try {
			localStorage.setItem('td-deck-lyrics', open ? '1' : '0');
		} catch {
			/* not worth failing the deck over */
		}
	}

	/** The reader, addressed at a particular line.
	 *
	 *  For *pinning*, where the line is the subject. **Not** for the ↗, which
	 *  opens the theatre live: this strip is showing the deck, so a reader
	 *  opened from it should follow the deck too — addressing it would pin the
	 *  words to whatever was playing at the moment of the click, and the next
	 *  song would leave them behind. That is the bug the live mode exists for,
	 *  and this link was one of the ways into it. */
	function readerHref(line?: number): string {
		const q = new URLSearchParams({ artist, title });
		if (album) q.set('album', album);
		if (line) q.set('line', String(line));
		return `/lyrics?${q}`;
	}

	/** Open the note on this track with the reference already in the body.
	 *
	 *  The **track**, not the record: a line belongs to a song, and the short
	 *  `[[line:12]]` is read against whatever the note is about — so it only
	 *  means line twelve of this song inside a note about this song. Naming the
	 *  record instead would file the reference against a subject that cannot
	 *  resolve it. */
	function quoteHref(line: number): string {
		const q = new URLSearchParams({
			kind: 'track',
			name: title,
			artist,
			quote: `[[line:${line}]]`
		});
		return `/notes?${q}`;
	}

	function mmss(s: number): string {
		const m = Math.floor(s / 60);
		return `${m}:${String(Math.floor(s - m * 60)).padStart(2, '0')}`;
	}

	onMount(() => {
		try {
			open = localStorage.getItem('td-deck-lyrics') === '1';
		} catch {
			// A browser that refuses storage still gets a deck.
		}
	});
</script>

<!-- Nothing at all when there is nothing to say. -->
{#if usable && current}
	{#if !open}
		<button class="strip" on:click={toggle}>
			<span class="chev">▾</span>
			<span class="lbl">Lyrics</span>
			<span class="one">
				{#if showRoman(current, view) && current.roman}<RomanLine line={current.roman} />{:else}{current.text}{/if}
			</span>
			<span class="badge">Timed</span>
		</button>
	{:else}
		<div class="well">
			<div class="wtop">
				<button class="chevbtn" on:click={toggle} title="Collapse lyrics" aria-label="Collapse lyrics">
					<span class="chev up">▾</span><span class="lbl">Hide</span>
				</button>
				<span class="badge">Timed{doc?.source ? ` · ${doc.source}` : ''}</span>
				<span style="flex:1;"></span>
				<span class="pos">line {current.n} / {lines.length}</span>
				<a class="out" href="/lyrics" title="Open the reader" aria-label="Open the reader">↗</a>
			</div>
			<div class="lines">
				{#each window_ as l (l.n)}
					<div class="wl" class:now={l.n === current.n}>
						<span class="wt">{l.at != null ? mmss(l.at) : l.n}</span>
						<span class="wx">
							{#if showRoman(l, view) && l.roman}
								{#if view === 'both'}<span class="wxo">{romanOriginal(l.roman)}</span>{/if}
								<RomanLine line={l.roman} />
							{:else}{l.text}{/if}
						</span>
					</div>
				{/each}
			</div>
			<div class="wact">
				<a class="qbtn" href={quoteHref(current.n)} title="Quote this line in a note">
					<span class="q">”</span> Quote
				</a>
				<a class="pbtn" href={readerHref(current.n)}>Pin a note</a>
			</div>
		</div>
	{/if}
{/if}

<style>
	/* Every colour here is a **token**, not a hardcoded cream.
	   The first version assumed the deck card was the dark panel in both
	   themes and wrote `#f4e7cf` text on a translucent cream — which is
	   right in dark mode and, in light mode, is cream on `--td-surface`'s
	   cream: invisible. That is the `--td-panel` trap of v0.70.8 arriving in a
	   new place, and the only thing that caught it was opening the page in the
	   other theme. */
	.strip,
	.well {
		margin-top: 12px;
		border-radius: 14px;
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
		color: var(--td-text);
	}
	.strip {
		display: flex;
		align-items: center;
		gap: 10px;
		width: 100%;
		padding: 9px 14px;
		cursor: pointer;
		text-align: left;
		font-family: inherit;
	}
	.strip:hover { border-color: var(--td-accent); }
	.chev { font-size: 10px; color: var(--td-accent); }
	.chev.up { display: inline-block; transform: rotate(180deg); }
	.lbl {
		font-size: 10px;
		letter-spacing: 0.14em;
		text-transform: uppercase;
		color: var(--td-muted-3);
	}
	.one {
		flex: 1;
		min-width: 0;
		font-size: 13.5px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.badge {
		font-size: 10px;
		padding: 2px 9px;
		border-radius: 999px;
		border: 1px solid var(--td-border);
		color: var(--td-muted-3);
		white-space: nowrap;
	}
	.well { padding: 11px 14px 12px; }
	.wtop { display: flex; align-items: center; gap: 9px; margin-bottom: 9px; flex-wrap: wrap; }
	/* A control, not a label.
	   This was `background:none; border:none; padding:0` — a bare chevron and
	   the word "Lyrics", which is exactly what a *heading* looks like. It
	   worked; nobody could tell it was the way back. Reported as "is there a
	   way to toggle the lyrics back to minimized". Affordance, not behaviour. */
	.chevbtn {
		display: inline-flex;
		align-items: center;
		gap: 7px;
		background: transparent;
		border: 1px solid var(--td-border);
		border-radius: 999px;
		padding: 3px 10px 3px 8px;
		cursor: pointer;
		font-family: inherit;
		color: inherit;
	}
	.chevbtn:hover,
	.chevbtn:focus-visible {
		border-color: var(--td-accent);
		color: var(--td-accent);
	}
	.pos { font-size: 10.5px; color: var(--td-muted-3); font-variant-numeric: tabular-nums; }
	.out {
		width: 24px;
		height: 24px;
		display: grid;
		place-items: center;
		border-radius: 999px;
		border: 1px solid var(--td-border);
		color: var(--td-muted-2);
		text-decoration: none;
		font-size: 12px;
	}
	.out:hover { color: var(--td-accent); border-color: var(--td-accent); }
	.lines { display: flex; flex-direction: column; gap: 1px; }
	.wl {
		display: grid;
		grid-template-columns: 34px minmax(0, 1fr);
		gap: 10px;
		align-items: baseline;
		padding: 3px 2px;
	}
	.wt {
		font-size: 10px;
		color: var(--td-muted-3);
		text-align: right;
		font-variant-numeric: tabular-nums;
	}
	.wx {
		font-size: 13px;
		line-height: 1.4;
		color: var(--td-muted-2);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.wl.now .wx { color: var(--td-text); font-size: 15px; font-weight: 600; }
	.wl.now .wt { color: var(--td-accent); }
	.wact { display: flex; align-items: center; gap: 8px; margin-top: 10px; }
	.qbtn,
	.pbtn {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: 11.5px;
		padding: 5px 12px;
		border-radius: 999px;
		text-decoration: none;
	}
	.qbtn { background: var(--td-accent); color: #fbf5ea; }
	.qbtn:hover { background: #b2622d; color: #fbf5ea; }
	.pbtn { border: 1px solid var(--td-border); color: var(--td-muted-2); }
	.pbtn:hover { color: var(--td-accent); border-color: var(--td-accent); }
	.q { font-size: 13px; line-height: 1; }
	@media (max-width: 560px) {
		.one { display: none; }
	}
	.wxo { display: block; font-size: 0.78em; opacity: 0.7; }
</style>
