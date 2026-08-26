<script lang="ts">
	/**
	 * What you can write in a liner note.
	 *
	 * Its own component because the answer was previously one line of small grey
	 * text under the textarea, listing three of the four shortcodes and none of
	 * what they do — so `[[gear:…]]` and the positional form existed and were
	 * undiscoverable unless you read the source. A feature nobody can find is not
	 * a feature.
	 *
	 * Collapsed by default once you've opened it, remembered in localStorage:
	 * this is a reference, and someone who has written twenty notes should not
	 * have to scroll past it every time. Not a modal or a tooltip — you need it
	 * *while* typing, which is exactly when a hover target is in the way.
	 */
	export let open = false;
	/** Persist the open/closed choice under this key. */
	export let remember = 'td-legend-open';

	const KEY = remember;

	// Read once at construction rather than in onMount: this renders inside an
	// editor that is already on screen, and a legend that pops open a frame
	// later moves the textarea under the cursor.
	if (typeof localStorage !== 'undefined') {
		const stored = localStorage.getItem(KEY);
		if (stored !== null) open = stored === '1';
	}

	function toggle() {
		open = !open;
		try {
			localStorage.setItem(KEY, open ? '1' : '0');
		} catch {
			// Private browsing. The legend still works; it just forgets.
		}
	}

	const CODES: { code: string; what: string }[] = [
		{ code: '[[track:Xtal]]', what: 'a track, by name' },
		{ code: '[[track:3]]', what: 'the third track of the record this note is about' },
		{ code: '[[album:Untrue]]', what: 'a record' },
		{ code: '[[artist:Burial]]', what: 'an artist' },
		{ code: '[[gear:Bifrost 2]]', what: 'something on your own shelf of equipment' },
		// Two forms; the short one is read against whatever the note is about,
		// the same rule the positional [[track:3]] follows.
		{ code: '[[line:12]]', what: 'line 12 of the track this note is about' },
		{ code: '[[line:A:3:12]]', what: 'side A, track 3, line 12 of this record' }
	];
</script>

<div class="legend" class:open>
	<button class="head" on:click={toggle} aria-expanded={open}>
		<svg class="chev" width="12" height="12" viewBox="0 0 12 12" fill="none"
			stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"
			aria-hidden="true">
			<path d="M4 2.5 8 6l-4 3.5" />
		</svg>
		<span>Markdown, and <code>[[…]]</code> to point at other music</span>
	</button>

	{#if open}
		<div class="body">
			<dl>
				{#each CODES as c}
					<div class="pair">
						<dt><code>{c.code}</code></dt>
						<dd>{c.what}</dd>
					</div>
				{/each}
			</dl>
			<p>
				A reference that matches nothing stays plain text with a dotted underline —
				never an error. Most of any library was never in MusicBrainz, and those are
				the records worth writing about.
			</p>
			<p>
				The body is Markdown: <code>**bold**</code>, <code>_italic_</code>,
				<code>## headings</code>, lists, tables, footnotes.
			</p>
		</div>
	{/if}
</div>

<style>
	.legend {
		border: 1px solid var(--td-border);
		border-radius: 14px;
		background: var(--td-surface2);
		overflow: hidden;
	}

	.head {
		display: flex;
		align-items: center;
		gap: 8px;
		width: 100%;
		text-align: left;
		border: none;
		background: transparent;
		font: inherit;
		font-size: 12.5px;
		color: var(--td-muted-2);
		cursor: pointer;
		padding: 9px 13px;
	}
	.head:hover { color: var(--td-text); }

	.chev { flex: none; transition: transform 0.14s ease; }
	.legend.open .chev { transform: rotate(90deg); }
	@media (prefers-reduced-motion: reduce) { .chev { transition: none; } }

	.body {
		padding: 2px 13px 13px;
		font-size: 12.5px;
		color: var(--td-muted);
	}

	dl { margin: 0 0 10px; }

	.pair {
		display: grid;
		/* The codes are the scannable column; the prose wraps beside them rather
		   than under, which is what makes this readable at a glance. */
		grid-template-columns: minmax(0, auto) minmax(0, 1fr);
		gap: 4px 12px;
		align-items: baseline;
		margin-bottom: 4px;
	}
	@media (max-width: 520px) {
		.pair { grid-template-columns: minmax(0, 1fr); gap: 1px; }
	}

	dt { margin: 0; }
	dd { margin: 0; color: var(--td-muted-3); }

	code {
		background: var(--td-sidebar);
		padding: 2px 6px;
		border-radius: 6px;
		font-size: 12px;
		white-space: nowrap;
	}

	p { margin: 0 0 6px; line-height: 1.5; }
	p:last-child { margin-bottom: 0; }
</style>
