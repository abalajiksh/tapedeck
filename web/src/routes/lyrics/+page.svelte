<script lang="ts">
	/**
	 * The lyric reader.
	 *
	 * **A first pass against `temp/Lyrics Final.dc.html`, and the reduction is
	 * written down here rather than left to be discovered** — the lesson Patch
	 * cost this project twice. Built: the numbered lines, the gutter in either
	 * mode, the source credit, and landing on a line a note points at. Not built
	 * yet: pins in the margin, "quoted in N notes", the tracklist beside it,
	 * *Following the deck*, and the annotate-and-drop-at-cursor mode.
	 *
	 * Two things in the design are deliberately absent rather than deferred.
	 * **Translations** — every line in it carries a Spanish translation and there
	 * is an `ES` toggle, but LRCLIB returns the words and nothing else and no
	 * provider is configured for one; a control over data that does not exist is
	 * worse than no control. And the **version picker** ("LRCLIB v3 ▾") implies a
	 * version history the API does not have.
	 */
	import { onMount } from 'svelte';
	import { api } from '$lib/api';

	type Line = { n: number; at: number | null; text: string };
	type Doc = {
		artist: string;
		title: string;
		language?: string | null;
		timed?: boolean;
		instrumental?: boolean;
		lines: Line[];
		source?: string;
	};

	let doc: Doc | null = null;
	let loading = true;
	let error = '';
	let artist = '';
	let title = '';
	/** The line a note pointed at, so the reader opens where the quote is. */
	let focus: number | null = null;
	/** Timestamps or line numbers. A fact about this browser, so localStorage. */
	let gutter: 'timestamps' | 'num' = 'timestamps';

	function mmss(s: number): string {
		const m = Math.floor(s / 60);
		return `${m}:${String(Math.floor(s - m * 60)).padStart(2, '0')}`;
	}

	async function load() {
		loading = true;
		error = '';
		try {
			doc = await api.lyrics(artist, title);
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		} finally {
			loading = false;
		}
	}

	onMount(async () => {
		const p = new URLSearchParams(window.location.search);
		artist = p.get('artist') ?? '';
		title = p.get('title') ?? '';
		const l = Number(p.get('line'));
		focus = Number.isFinite(l) && l > 0 ? l : null;
		try {
			const g = localStorage.getItem('td-lyrics-gutter');
			if (g === 'num' || g === 'timestamps') gutter = g;
		} catch {
			// A browser that refuses storage still gets a reader.
		}
		if (!artist || !title) {
			error = 'No track named — open a lyric from a listen or a note.';
			loading = false;
			return;
		}
		await load();
		// After a paint, or the element the quote points at does not exist yet.
		requestAnimationFrame(() =>
			document.querySelector('.ln.focus')?.scrollIntoView({ block: 'center' })
		);
	});

	function setGutter(g: 'timestamps' | 'num') {
		gutter = g;
		try {
			localStorage.setItem('td-lyrics-gutter', g);
		} catch {
			/* not worth failing the page over */
		}
	}

	$: timed = doc?.timed ?? false;
</script>

<svelte:head><title>{title ? `${title} — Lyrics` : 'Lyrics'} — Tapedeck</title></svelte:head>

<p class="eyebrow">Lyrics</p>

{#if loading}
	<div class="panel pad">Reading…</div>
{:else if error}
	<div class="panel pad bad">{error}</div>
{:else if doc}
	<header class="head">
		<div>
			<h1 class="h-display">{doc.title}</h1>
			<p class="sub">
				{doc.artist}
				{#if doc.language}<span class="tag">{doc.language}</span>{/if}
			</p>
		</div>
		{#if timed}
			<div class="seg">
				<button class="seg-btn" class:on={gutter === 'timestamps'} on:click={() => setGutter('timestamps')}>Timed</button>
				<button class="seg-btn" class:on={gutter === 'num'} on:click={() => setGutter('num')}>Lines</button>
			</div>
		{/if}
	</header>

	{#if doc.instrumental}
		<!-- A real answer, not a gap: LRCLIB records this track as having no
		     words. Saying so is better than an empty page. -->
		<div class="panel pad muted">No words — this track is instrumental.</div>
	{:else if !doc.lines.length}
		<div class="panel pad muted">
			Nothing held for this one yet. Lyrics are fetched a few tracks a minute,
			most-played first — or LRCLIB may simply not have it.
		</div>
	{:else}
		<div class="panel lyric">
			{#each doc.lines as l (l.n)}
				{#if !l.text.trim()}
					<!-- A stanza break is a numbered line — that is what makes an
					     index stable — so it takes its space and keeps its number. -->
					<div class="ln break" class:focus={focus === l.n}><span class="g"></span><span></span></div>
				{:else}
					<div class="ln" class:focus={focus === l.n}>
						<span class="g">{gutter === 'timestamps' && l.at != null ? mmss(l.at) : l.n}</span>
						<span class="t">{l.text}</span>
					</div>
				{/if}
			{/each}
		</div>
		<p class="credit">
			Words via {doc.source ?? 'LRCLIB'}. Cached here so a note can point at a line —
			they are the provider's text, and they never leave this instance.
		</p>
	{/if}
{/if}

<style>
	.eyebrow {
		font-size: 12px;
		letter-spacing: 0.14em;
		text-transform: uppercase;
		color: var(--td-accent);
		margin: 0 0 6px;
	}
	.head {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 16px;
		flex-wrap: wrap;
		margin-bottom: 18px;
	}
	h1 {
		margin: 0;
		font-size: 34px;
		letter-spacing: -0.02em;
	}
	.sub {
		margin: 4px 0 0;
		color: var(--td-muted);
		font-size: 14px;
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.tag {
		font-size: 11px;
		padding: 2px 8px;
		border-radius: 999px;
		border: 1px solid var(--td-border);
		color: var(--td-muted);
		letter-spacing: 0.06em;
	}
	/* Repeated here, the way Reports and Settings both repeat it: Svelte scopes
	   styles per component, so a class defined elsewhere renders as nothing at
	   all — the trap v0.42.0 records, and this toggle hit it. Matching the
	   existing definition matters more than deduplicating it. */
	.seg {
		display: inline-flex;
		gap: 2px;
		padding: 3px;
		border-radius: 999px;
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
	}
	.seg-btn {
		border: none;
		background: none;
		cursor: pointer;
		padding: 6px 14px;
		border-radius: 999px;
		font: inherit;
		font-size: 13px;
		color: var(--td-muted);
	}
	.seg-btn.on {
		background: var(--td-accent);
		color: #fbf5ea;
	}
	.pad { padding: 22px; border-radius: 22px; }
	.muted { color: var(--td-muted); }
	.bad { color: var(--td-bad-fg); }
	.lyric {
		border-radius: 22px;
		padding: 26px 22px;
		display: flex;
		flex-direction: column;
	}
	.ln {
		display: grid;
		grid-template-columns: 58px 1fr;
		gap: 14px;
		align-items: baseline;
		padding: 4px 8px;
		border-radius: 8px;
		scroll-margin: 40vh;
	}
	.ln.break { min-height: 14px; }
	.g {
		font-size: 12px;
		color: var(--td-muted-3);
		font-variant-numeric: tabular-nums;
		text-align: right;
	}
	.t { font-size: 16.5px; line-height: 1.55; color: var(--td-text); }
	/* The line a note points at. A ring rather than a fill: the words are the
	   thing being read, and a highlight behind them fights the text. */
	.ln.focus {
		background: color-mix(in srgb, var(--td-accent) 10%, transparent);
		box-shadow: inset 2px 0 0 var(--td-accent);
	}
	.credit {
		font-size: 12px;
		color: var(--td-muted-3);
		margin: 12px 2px 0;
		text-wrap: pretty;
	}
</style>
