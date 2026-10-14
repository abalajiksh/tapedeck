<script lang="ts">
	/**
	 * One lyric line in Latin letters, drawn two ways.
	 *
	 * * `romanized` — the words, with a dot between sung syllables where the
	 *   word is sung syllable by syllable (Korean): `kko·tyang·gi·man`. The
	 *   dots are drawn, not stored, so copying the line copies plain text.
	 * * `both` — interlinear: each part under its original, one Hangul block,
	 *   one Japanese or Chinese token, one Indic word at a time. Lines wrap
	 *   only between parts, never inside one.
	 *
	 * Colours come from the parent through `currentColor` and the two custom
	 * properties below, because it is drawn on the reader's always-dark stage
	 * and on themed panels alike.
	 */
	import type { RomanLine } from '$lib/api';

	export let line: RomanLine;
	export let mode: 'romanized' | 'both' = 'romanized';
</script>

{#if mode === 'both'}
	<span class="il">
		{#each line.words as w, wi}
			{#if wi > 0}<span class="ilgap" aria-hidden="true"></span>{/if}
			<span class="ilword" class:syl={w.syl}>
				{#each w.parts as p}
					<span class="ilcol"><span class="ilo">{p.o}</span><span class="ilr">{p.lit ? '' : p.r}</span></span>
				{/each}
			</span>
		{/each}
	</span>
{:else}
	<span class="rl"
		>{#each line.words as w, wi}{#if wi > 0}{' '}{/if}<span class="rw"
				>{#each w.parts as p, pi}{#if pi > 0 && w.syl && !p.lit && !w.parts[pi - 1].lit}<span
							class="dot"
							aria-hidden="true">·</span
						><wbr />{/if}{p.r}{/each}</span
			>{/each}</span
	>
{/if}

<style>
	/* Breaks at spaces and at the <wbr> between syllables, never mid-syllable. */
	.rl { word-break: keep-all; overflow-wrap: normal; }
	.dot { color: var(--roman-dot, currentColor); opacity: 0.45; padding: 0 0.04em; }
	.il { display: inline-flex; flex-wrap: wrap; align-items: flex-end; row-gap: 0.35em; }
	.ilgap { width: 0.7em; }
	.ilword { display: inline-flex; gap: 0.12em; }
	/* Sung syllables get room to read as syllables under their blocks. */
	.ilword.syl { gap: 0.3em; }
	.ilcol { display: inline-flex; flex-direction: column; align-items: center; line-height: 1.1; }
	.ilo { font-size: 0.5em; color: var(--roman-original, currentColor); opacity: 0.72; font-family: inherit; }
	.ilr { white-space: nowrap; }
</style>
