<script lang="ts">
	/**
	 * *Other versions* — the other records LRCLIB holds for a track, and the
	 * control for holding one of them instead.
	 *
	 * **One copy, two doors.** It lived inside the notes panel's picker and
	 * nowhere else, so the reader — where somebody actually sees the wrong
	 * words — had no way to it at all once *any* words were held: its only
	 * fetch button is drawn when nothing is. A second copy here would have been
	 * a second answer to "which records are there", so both now open this.
	 *
	 * **Adopted by id, never by sending the words back.** The picker used to
	 * `PUT` the chosen text through the correction path, which credited a
	 * stranger's upload to "You" and dropped the record id. The server now
	 * fetches the record itself and credits LRCLIB.
	 *
	 * What a row shows is what tells two uploads apart: the **script** (the
	 * usual reason to look — a romanised tag where the original was wanted),
	 * timed or not, the line count against the one held (a reference is an
	 * index), and the length against the track's.
	 */
	import { createEventDispatcher, onMount, tick } from 'svelte';
	import { api } from '$lib/api';
	import type { LyricMatch } from '$lib/api';

	/** The track, as it is filed here — what a pick is stored under. */
	export let artist: string;
	export let title: string;
	/** Its length in seconds, when known, to compare each record's against. */
	export let duration: number | null = null;
	/** Lines in the words held now, blanks included; 0 when none. */
	export let lines = 0;
	/** The words held are in Latin letters with nothing to romanize — so a
	 *  version in the original script can be kept *beside* them instead of
	 *  replacing them (the JISOO 꽃 case: a lyric tagged romanised in your own
	 *  files). */
	export let latinHeld = false;

	const dispatch = createEventDispatcher<{
		close: void;
		adopted: { renumbered: boolean; lines_before: number; lines_after: number; companion?: boolean };
	}>();

	let searchArtist = artist;
	let searchTitle = title;
	let matches: LyricMatch[] = [];
	let searching = false;
	let saving = false;
	let why = '';
	let error = '';
	let firstField: HTMLInputElement | undefined;

	const clean = (e: unknown) => String(e).replace(/^API \d+: /, '');

	function mmss(s: number): string {
		const m = Math.floor(s / 60);
		return `${m}:${String(Math.floor(s - m * 60)).padStart(2, '0')}`;
	}

	/** How far a record's length is from the track's. LRCLIB itself matches
	 *  within ±2 s, so that is what counts as the same recording. */
	function drift(d: number | null): string {
		if (d == null || duration == null) return '';
		const off = Math.round(d - duration);
		if (Math.abs(off) <= 2) return 'same length';
		return off > 0 ? `${off}s longer` : `${-off}s shorter`;
	}

	/** The first few lines with words in them — enough to recognise a lyric
	 *  and its script, not a copy of it. Cues are stripped from a timed one. */
	function preview(m: LyricMatch): string[] {
		const text = m.plain?.trim() ? m.plain : (m.synced_text ?? '');
		return text
			.split('\n')
			.map((l) => l.replace(/^\s*(\[\d+:\d+(?:\.\d+)?\]\s*)+/, '').trim())
			.filter(Boolean)
			.slice(0, 3);
	}

	async function search() {
		searching = true;
		why = '';
		error = '';
		matches = [];
		try {
			const r = await api.lyricMatches(artist, title, {
				artist: searchArtist !== artist ? searchArtist : undefined,
				title: searchTitle !== title ? searchTitle : undefined
			});
			matches = r.matches;
			// "LRCLIB has nothing" is an answer; "we could not ask" is not, and
			// reporting the second as the first sends somebody off to type the
			// words out by hand.
			why = r.unavailable
				? (r.why ?? 'Could not reach LRCLIB.')
				: matches.length
					? ''
					: 'LRCLIB has nothing under those names. Try the title as LRCLIB might file it.';
		} catch (e) {
			error = clean(e);
		}
		searching = false;
	}

	async function adopt(m: LyricMatch, original_script = false) {
		if (m.id == null || saving) return;
		saving = true;
		error = '';
		try {
			const r = await api.adoptLyrics({ artist, title, id: m.id, original_script });
			if ('unavailable' in r) {
				error = r.why;
			} else if (r.status === 'companion') {
				dispatch('adopted', { renumbered: false, lines_before: lines, lines_after: lines, companion: true });
			} else {
				dispatch('adopted', {
					renumbered: r.renumbered,
					lines_before: r.lines_before,
					lines_after: r.lines_after
				});
			}
		} catch (e) {
			error = clean(e);
		}
		saving = false;
	}

	function onKey(e: KeyboardEvent) {
		if (e.key === 'Escape') dispatch('close');
	}

	onMount(async () => {
		search();
		await tick();
		firstField?.focus();
	});
</script>

<svelte:window on:keydown={onKey} />

<div class="scrim" role="presentation" on:click={() => dispatch('close')}></div>
<div class="dialog" role="dialog" aria-modal="true" aria-label="Other versions">
	<h3 class="h-display" style="font-size:18px;margin:0 0 4px;">Other versions</h3>
	<p class="dlgsub">
		LRCLIB keeps no version history — these are <em>other records</em> for the same song. The one
		you pick is kept: no later lookup and no library tag replaces it, and it stays credited to
		LRCLIB.
		{#if latinHeld}
			Your words are in Latin letters: <em>Keep as original</em> keeps a version in its own script
			beside them, and romanization is made from it — your words stay what a note quotes.
		{/if}
	</p>

	<form class="vsearch" on:submit|preventDefault={search}>
		<label class="vf">
			<span>Artist</span>
			<input class="input" bind:this={firstField} bind:value={searchArtist} />
		</label>
		<label class="vf">
			<span>Title</span>
			<input class="input" bind:value={searchTitle} />
		</label>
		<button class="btn btn-secondary" type="submit" disabled={searching}>
			{searching ? 'Asking…' : 'Search'}
		</button>
	</form>

	{#if error}<p class="vmsg">{error}</p>{/if}
	{#if searching}
		<p class="vmsg">Asking LRCLIB…</p>
	{:else if why}
		<p class="vmsg">{why}</p>
	{:else}
		<div class="vlist">
			{#each matches as m (m.id ?? `${m.artist}-${m.title}-${m.lines}`)}
				<div class="vrow" class:held={m.held}>
					<div class="vhead">
						<span class="vmain">
							<span class="vt">{m.title ?? title}</span>
							<span class="va">{m.artist ?? artist}{m.album ? ` · ${m.album}` : ''}</span>
						</span>
						{#if m.held}
							<span class="vpill lit">Held now</span>
						{:else if latinHeld && m.language && m.lines === lines}
							<!-- Your words stay; this is kept beside them and romanized.
							     Offered only where it lines up — the server checks the
							     timing too. -->
							<span class="vbtns">
								<button
									class="btn btn-primary vuse"
									on:click={() => adopt(m, true)}
									disabled={saving || m.id == null}
									aria-label="Keep as the original script: {m.title ?? title}{m.album ? `, ${m.album}` : ''}, {m.lines} lines"
								>
									{saving ? '…' : 'Keep as original'}
								</button>
								<button
									class="btn btn-secondary vuse"
									on:click={() => adopt(m)}
									disabled={saving || m.id == null}
									aria-label="Use instead of your words: {m.title ?? title}{m.album ? `, ${m.album}` : ''}, {m.lines} lines"
								>
									Use instead
								</button>
							</span>
						{:else}
							<button
								class="btn btn-primary vuse"
								on:click={() => adopt(m)}
								disabled={saving || m.id == null}
								aria-label="Use this version: {m.title ?? title}{m.album ? `, ${m.album}` : ''}, {m.lines} lines{m.duration ? `, ${mmss(m.duration)}` : ''}"
							>
								{saving ? '…' : 'Use this'}
							</button>
						{/if}
					</div>
					<div class="vmeta">
						<!-- The script first: it is the commonest reason to be here, and
						     a romanised upload looks identical to a native one on every
						     other field. -->
						<span class="vpill" class:lit={!!m.language}>{m.language ?? 'Latin'}</span>
						<span class="vpill" class:lit={m.synced}>{m.synced ? 'Timed' : 'Plain'}</span>
						<span class="vn" class:warn={lines > 0 && m.lines !== lines}>
							{m.lines} lines{#if lines > 0 && m.lines !== lines}{` (yours: ${lines})`}{/if}
						</span>
						{#if m.duration}
							<span class="vn">{mmss(m.duration)}{#if drift(m.duration)}{` · ${drift(m.duration)}`}{/if}</span>
						{/if}
					</div>
					{#if preview(m).length}
						<div class="vprev">
							{#each preview(m) as l}<span>{l}</span>{/each}
						</div>
					{/if}
				</div>
			{/each}
		</div>
		{#if matches.some((m) => !m.held && lines > 0 && m.lines !== lines)}
			<p class="vnote">
				A version with a different line count moves every <code>[[line:…]]</code> quote past the
				change onto different words.
			</p>
		{/if}
	{/if}

	<div class="dlgrow">
		<button class="btn btn-secondary" on:click={() => dispatch('close')}>Close</button>
	</div>
</div>

<style>
	.scrim { position: fixed; inset: 0; background: rgba(0, 0, 0, 0.45); z-index: 40; }
	.dialog {
		position: fixed;
		z-index: 41;
		top: 50%;
		left: 50%;
		transform: translate(-50%, -50%);
		width: min(720px, 92vw);
		max-height: 86vh;
		overflow: auto;
		background: var(--td-surface);
		/* Stated rather than inherited: the reader behind it is dark in both
		   themes, and a dialog taking its colour from there reads in one. */
		color: var(--td-text);
		border: 1px solid var(--td-border);
		border-radius: 22px;
		padding: 22px;
		text-align: left;
	}
	.dlgsub { font-size: 12.5px; color: var(--td-muted); margin: 0 0 14px; line-height: 1.55; }
	.dlgrow { display: flex; align-items: center; gap: 8px; margin-top: 14px; flex-wrap: wrap; }
	.vsearch { display: flex; align-items: flex-end; gap: 8px; flex-wrap: wrap; margin-bottom: 12px; }
	.vf { display: flex; flex-direction: column; gap: 4px; flex: 1 1 180px; min-width: 0; }
	.vf span { font-size: 11.5px; color: var(--td-muted-2); }
	.vf .input { min-height: 34px; font-size: 13px; padding: 5px 10px; }
	.vmsg { font-size: 12.5px; color: var(--td-muted); margin: 10px 2px; line-height: 1.5; }
	.vlist { display: flex; flex-direction: column; gap: 8px; }
	.vrow {
		display: flex;
		flex-direction: column;
		gap: 7px;
		padding: 11px 13px;
		border-radius: 13px;
		border: 1px solid var(--td-border);
		background: var(--td-surface2);
	}
	.vrow.held { border-color: var(--td-accent); }
	.vhead { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
	.vmain { display: flex; flex-direction: column; gap: 1px; min-width: 0; }
	.vt { font-size: 13.5px; font-weight: 600; }
	.va { font-size: 11.5px; color: var(--td-muted-2); overflow-wrap: anywhere; }
	.vuse { flex: none; font-size: 12px; padding: 6px 14px; }
	.vbtns { display: flex; gap: 6px; flex-wrap: wrap; justify-content: flex-end; }
	.vmeta { display: flex; align-items: center; gap: 7px; flex-wrap: wrap; }
	.vpill {
		font-size: 10px;
		font-weight: 700;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		padding: 2px 7px;
		border-radius: 999px;
		border: 1px solid var(--td-border);
		color: var(--td-muted-3);
	}
	.vpill.lit { border-color: var(--td-accent); color: var(--td-accent); }
	.vn { font-size: 11px; color: var(--td-muted-3); font-variant-numeric: tabular-nums; }
	.vn.warn { color: var(--td-accent); }
	.vprev {
		display: flex;
		flex-direction: column;
		gap: 1px;
		font-size: 12px;
		line-height: 1.45;
		color: var(--td-muted);
		border-left: 2px solid var(--td-border);
		padding-left: 9px;
	}
	.vnote { font-size: 11.5px; color: var(--td-muted); margin: 10px 2px 0; line-height: 1.5; }
</style>
