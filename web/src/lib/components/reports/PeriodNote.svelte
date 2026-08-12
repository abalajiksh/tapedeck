<script lang="ts">
	/**
	 * The liner note for a week, a month or a year.
	 *
	 * Reports is the one screen in Tapedeck that is *about a stretch of time*
	 * rather than about a record, and it is where you would actually write "the
	 * month I could not stop playing Untrue". Every other note target — a
	 * release, an artist, a single listen — answers a different question.
	 *
	 * It hangs off `period_key`, which is the same key `reports::period_of`
	 * builds and is stable by construction: report weeks deliberately ignore the
	 * user's week-start preference precisely so a named period cannot change
	 * shape under a note written about it.
	 */
	import { api, type Note } from '$lib/api';

	/** `week` / `month` / `year`, and the key exactly as Reports names it. */
	export let unit: 'week' | 'month' | 'year';
	export let periodKey: string;
	/** "August 2026", "24 Jul – 30 Jul" — what the stepper calls this period. */
	export let label: string;

	let note: Note | null = null;
	let loading = true;
	let editing = false;
	let draft = '';
	let saving = false;
	let error = '';

	// Stepping to another period must not leave the previous one's note on
	// screen, and must not carry an open editor across — that draft belongs to
	// the period it was started in.
	let lastKey = '';
	$: if (periodKey && `${unit}:${periodKey}` !== lastKey) {
		lastKey = `${unit}:${periodKey}`;
		editing = false;
		draft = '';
		error = '';
		void load();
	}

	async function load() {
		loading = true;
		try {
			const r = await api.noteFor({ period_unit: unit, period_key: periodKey });
			note = r.note;
		} catch {
			// A note is an addition to the chapter, never a precondition for it —
			// a failure here must not take the report down with it.
			note = null;
		}
		loading = false;
	}

	function startEdit() {
		draft = note?.body_md ?? '';
		editing = true;
		error = '';
	}

	async function save() {
		if (!draft.trim()) return;
		saving = true;
		error = '';
		try {
			note = await api.saveNote({
				period_unit: unit,
				period_key: periodKey,
				body_md: draft
			});
			editing = false;
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		saving = false;
	}

	async function remove() {
		if (!note) return;
		saving = true;
		try {
			await api.deleteNote(note.id);
			note = null;
			editing = false;
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		saving = false;
	}
</script>

<div class="panel note-panel">
	<div class="head">
		<div>
			<div class="eyebrow">Liner notes</div>
			<h3 class="h-display" style="font-size:18px;margin:2px 0 0;">On {label}</h3>
		</div>
		{#if !editing}
			<div style="display:flex;gap:8px;">
				<button class="btn btn-secondary" on:click={startEdit} disabled={loading}>
					{note ? 'Edit' : 'Write one'}
				</button>
				{#if note}
					<button class="btn btn-secondary" on:click={remove} disabled={saving}>Delete</button>
				{/if}
			</div>
		{/if}
	</div>

	{#if error}<p class="err">{error}</p>{/if}

	{#if editing}
		<textarea
			class="input"
			bind:value={draft}
			rows="7"
			placeholder="What this {unit} sounded like. Markdown, and [[album:…]] / [[artist:…]] / [[gear:…]] all work."
		></textarea>
		<div class="row">
			<button class="btn btn-primary" on:click={save} disabled={saving || !draft.trim()}>
				{saving ? 'Saving…' : 'Save'}
			</button>
			<button class="btn btn-secondary" on:click={() => (editing = false)} disabled={saving}>
				Cancel
			</button>
			<span class="hint">
				<!-- A period is not about one record, so there is nothing for a
				     positional shortcode to be relative to. Said here rather than
				     discovered by writing one that renders as plain text. -->
				Named shortcodes resolve; <code>[[track:3]]</code> has no record to count within.
			</span>
		</div>
	{:else if loading}
		<p class="quiet">…</p>
	{:else if note}
		<!-- Rendered and sanitised server-side, shortcodes already resolved —
		     the same {@html} the notes page uses. -->
		<div class="body">{@html note.body_html ?? ''}</div>
	{:else}
		<p class="quiet">
			Nothing written about this {unit} yet. The numbers say what you played; this is
			for what it was like.
		</p>
	{/if}
</div>

<style>
	.note-panel {
		border-radius: 22px;
		padding: 20px 24px;
		margin-top: 18px;
	}
	.head {
		display: flex;
		justify-content: space-between;
		align-items: flex-start;
		gap: 16px;
		flex-wrap: wrap;
		margin-bottom: 12px;
	}
	.eyebrow {
		font-size: 11px;
		letter-spacing: 0.16em;
		text-transform: uppercase;
		color: var(--td-muted-3);
	}
	.row {
		display: flex;
		gap: 10px;
		align-items: center;
		flex-wrap: wrap;
		margin-top: 10px;
	}
	.hint {
		font-size: 12px;
		color: var(--td-muted-3);
	}
	.quiet {
		margin: 0;
		font-size: 13.5px;
		color: var(--td-muted);
		max-width: 60ch;
	}
	.err {
		margin: 0 0 10px;
		font-size: 13px;
		color: #b2622d;
	}
	.body {
		font-size: 14.5px;
		line-height: 1.65;
	}
	.body :global(p) { margin: 0 0 10px; }
	.body :global(p:last-child) { margin-bottom: 0; }
	textarea.input {
		width: 100%;
		font-family: inherit;
		font-size: 14px;
		line-height: 1.6;
		resize: vertical;
	}
</style>
