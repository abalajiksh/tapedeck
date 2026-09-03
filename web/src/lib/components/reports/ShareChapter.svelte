<script lang="ts">
	/**
	 * Export a chapter as an image.
	 *
	 * **The picture is drawn on the server** — `GET /api/v1/reports/image` — so
	 * this component chooses, previews and saves, and lays nothing out. That is
	 * the whole point of the endpoint: an Android or iOS client asks for the same
	 * image rather than reimplementing a layout that would then be free to drift
	 * from this one.
	 *
	 * Theme is asked here rather than inherited from the app's own setting. A
	 * share lands on somebody else's feed, so "which look" is a question about
	 * the post and not about how the reader likes their dashboard.
	 */
	import { onDestroy } from 'svelte';

	/** Which chapter. All three have their own composition on the server — a
	 *  week's by-day bars, a year's twelve-month shape and genre bands — rather
	 *  than one card relabelled, which is why this is a unit and not a title. */
	export let unit: 'week' | 'month' | 'year';
	export let offset = 0;
	export let label = '';
	export let open = false;

	type Size = 'story' | 'square' | 'wide';
	type Theme = 'dark' | 'light';

	const SIZES: { k: Size; label: string; hint: string }[] = [
		{ k: 'story', label: 'Story', hint: '1080 × 1920' },
		{ k: 'square', label: 'Square', hint: '1080 × 1080' },
		{ k: 'wide', label: 'Wide', hint: '1600 × 900' }
	];
	const THEMES: { k: Theme; label: string }[] = [
		{ k: 'dark', label: 'Dark' },
		{ k: 'light', label: 'Light' }
	];

	let size: Size = 'story';
	let theme: Theme = 'dark';
	let saving = false;
	let error = '';
	/** Whether the server could promise every character on the card. */
	let coverage: string | null = null;

	$: src = `/api/v1/reports/image?unit=${unit}&offset=${offset}&size=${size}&theme=${theme}`;
	$: filename = `tapedeck-${label.toLowerCase().replace(/[^a-z0-9]+/g, '-')}-${size}.png`;

	/**
	 * Saved from a fetched blob rather than by pointing an `<a download>` at the
	 * endpoint.
	 *
	 * Two reasons, and the second is the one that bites: a cross-origin
	 * `download` attribute is ignored by every browser, so the moment this app is
	 * served from anywhere but the API's own origin the link would *navigate* to
	 * the PNG instead of saving it — and it would look like it worked. Fetching
	 * also lets the glyph-coverage header be read, which a link never could.
	 */
	async function save() {
		saving = true;
		error = '';
		try {
			const res = await fetch(src, { credentials: 'same-origin' });
			if (!res.ok) throw new Error(`The image could not be drawn (${res.status})`);
			coverage = res.headers.get('x-tapedeck-glyph-coverage');
			const blob = await res.blob();
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = filename;
			a.click();
			// Freed on the next frame: revoking immediately can beat the download.
			setTimeout(() => URL.revokeObjectURL(url), 60_000);
		} catch (e) {
			error = e instanceof Error ? e.message : 'The image could not be drawn';
		} finally {
			saving = false;
		}
	}

	function close() {
		open = false;
		error = '';
	}

	onDestroy(() => {
		open = false;
	});
</script>

{#if open}
	<div class="dialog-backdrop" on:click={close} on:keydown={() => {}} role="presentation">
		<div
			class="dialog"
			style="width:min(560px,100%);"
			on:click|stopPropagation
			on:keydown|stopPropagation
			role="presentation"
		>
			<div class="dialog-title">Share {label}</div>
			<div class="dialog-body">
				<div class="field">
					<span class="lbl">Shape</span>
					<div class="seg">
						{#each SIZES as s (s.k)}
							<button
								class="seg-btn"
								class:on={size === s.k}
								on:click={() => (size = s.k)}>{s.label}</button
							>
						{/each}
					</div>
					<!-- Beside the control rather than in a `title`: a tooltip
					     overrode each button's accessible name, so the shapes read
					     as "1080 × 1920" instead of "Story" — and a tooltip is
					     unreachable from a keyboard or a phone anyway. -->
					<p class="hint">{SIZES.find((s) => s.k === size)?.hint}</p>
				</div>

				<div class="field">
					<span class="lbl">Look</span>
					<div class="seg">
						{#each THEMES as t (t.k)}
							<button
								class="seg-btn"
								class:on={theme === t.k}
								on:click={() => (theme = t.k)}>{t.label}</button
							>
						{/each}
					</div>
					<p class="hint">
						Asked here rather than taken from your own theme — this one lands on
						somebody else's screen.
					</p>
				</div>

				<!-- Keyed so switching shape or look replaces the image rather than
				     leaving the previous one up while the next decodes. -->
				{#key src}
					<div class="preview" class:wide={size === 'wide'}>
						<img {src} alt="{label} as a {size} card" />
					</div>
				{/key}

				{#if coverage === 'partial'}
					<p class="warn">
						Some characters on this card aren't in Tapedeck's own fonts — Hangul, CJK
						and Cyrillic fall back to whatever the server has installed, so they may
						come out as boxes. Everything else is exact.
					</p>
				{/if}
				{#if error}<p class="bad">{error}</p>{/if}

				<div class="acts">
					<button class="btn btn-secondary" on:click={close}>Close</button>
					<a class="btn btn-secondary" href="{src}&format=svg" target="_blank" rel="noreferrer">
						Open the SVG
					</a>
					<button class="btn btn-primary" on:click={save} disabled={saving}>
						{saving ? 'Drawing…' : 'Save the PNG'}
					</button>
				</div>
			</div>
		</div>
	</div>
{/if}

<style>
	.lbl {
		display: block;
		font-size: 12px;
		margin-bottom: 5px;
		color: var(--td-muted);
	}
	.hint {
		font-size: 12px;
		color: var(--td-muted-2);
		margin: 6px 0 0;
	}
	.field {
		margin-bottom: 16px;
	}
	/* Repeated per page, the way Reports and Settings both repeat it: Svelte
	   scopes styles, so matching the existing definition matters more than
	   deduplicating it. */
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
	.preview {
		display: flex;
		justify-content: center;
		padding: 14px;
		border-radius: 16px;
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
	}
	.preview img {
		max-width: 100%;
		max-height: 46vh;
		border-radius: 10px;
		display: block;
	}
	.preview.wide img {
		max-height: 30vh;
	}
	.warn,
	.bad {
		font-size: 12.5px;
		margin-top: 12px;
		padding: 9px 12px;
		border-radius: 0 12px 12px 0;
		color: var(--td-text);
	}
	.warn {
		background: var(--td-good-bg);
		border-left: 3px solid var(--td-good-fg);
	}
	.bad {
		background: var(--td-bad-bg);
		border-left: 3px solid var(--td-bad-fg);
	}
	.acts {
		display: flex;
		gap: 8px;
		justify-content: flex-end;
		flex-wrap: wrap;
		margin-top: 16px;
	}
</style>
