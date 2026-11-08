<script lang="ts">
	/**
	 * Artists you mostly play one song from, most anchored first.
	 *
	 * A ranked list rather than the script's bar chart, whose every bar was
	 * 100% long and said nothing a number doesn't. Counted over everyone
	 * credited, so a soundtrack's singers each stand on their own rather than
	 * as one "Composer, Singer, Singer" artist with one song.
	 */
	import { nf } from './graphs';
	import { openArtist } from '$lib/entities';

	export let anchors: { name: string; plays: number; title: string; title_plays: number; share: number; tracks: number }[] = [];
	export let minPlays = 20;

	/** Six sit beside the long tail without towering over it. */
	const SHOWN = 6;
	let all = false;
	$: shown = all ? anchors : anchors.slice(0, SHOWN);
</script>

{#if anchors.length}
	<ol class="list">
		{#each shown as a}
			<li>
				<div class="top">
					<button class="nm" on:click={() => openArtist(a.name)}>{a.name}</button>
					<span class="pc">{Math.round(a.share * 100)}%</span>
				</div>
				<div class="song">“{a.title}”</div>
				<div class="track"><div class="fill" style="width:{a.share * 100}%;"></div></div>
				<div class="sub">
					{nf(a.title_plays)} of {nf(a.plays)} plays · {nf(a.tracks)} song{a.tracks === 1 ? '' : 's'} played
				</div>
			</li>
		{/each}
	</ol>
	{#if anchors.length > SHOWN}
		<button class="more" on:click={() => (all = !all)}>
			{all ? 'Show fewer' : `Show all ${anchors.length}`}
		</button>
	{/if}
	<p class="caption">Artists with at least {minPlays} plays here, by the share of them that are one song.</p>
{:else}
	<p class="caption">No artist with {minPlays} or more plays in this range leans on a single song.</p>
{/if}

<style>
	.list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 12px; }
	.top { display: flex; justify-content: space-between; gap: 10px; align-items: baseline; }
	.nm { background: none; border: 0; padding: 0; font-family: inherit; font-size: 13px; font-weight: 600; color: var(--td-text); cursor: pointer; text-align: left; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
	.nm:hover { color: var(--td-accent); }
	.pc { font-size: 13px; font-weight: 600; font-variant-numeric: tabular-nums; flex: none; }
	.song { font-size: 12.5px; color: var(--td-muted); margin: 1px 0 5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.track { height: 6px; border-radius: 999px; background: var(--td-track); overflow: hidden; }
	.fill { height: 100%; background: var(--td-accent); border-radius: 999px; }
	.sub { font-size: 11.5px; color: var(--td-muted-3); margin-top: 4px; }
	.more { margin-top: 12px; background: none; border: 0; padding: 0; font-family: inherit; font-size: 12.5px; color: var(--td-accent); cursor: pointer; }
	.caption { font-size: 12px; color: var(--td-muted-3); margin: 12px 0 0; }
</style>
