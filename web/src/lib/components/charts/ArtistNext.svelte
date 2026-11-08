<script lang="ts">
	/**
	 * Who you go on to after this artist: the next listen, within a quarter of
	 * an hour, when it was somebody else. A share of those hand-overs, and a
	 * listen crediting two artists counts for both — so the bars are each out
	 * of `total` and need not add up to it.
	 */
	import { nf } from './graphs';
	import { openArtist } from '$lib/entities';

	export let total = 0;
	export let artists: { name: string; plays: number }[] = [];

	$: max = Math.max(1, ...artists.map((a) => a.plays));
</script>

{#if artists.length}
	<div class="rows">
		{#each artists as a}
			<button class="row" on:click={() => openArtist(a.name)} title="Open {a.name}">
				<span class="nm">{a.name}</span>
				<span class="track"><span class="fill" style="width:{(a.plays / max) * 100}%;"></span></span>
				<span class="n">{nf(a.plays)} <span class="of">of {nf(total)}</span></span>
			</button>
		{/each}
	</div>
	<p class="caption">
		Of the {nf(total)} times one of their listens was followed within a quarter of an hour by somebody else.
	</p>
{:else}
	<p class="caption">They have not been followed by anyone else within a quarter of an hour yet.</p>
{/if}

<style>
	.rows { display: flex; flex-direction: column; gap: 8px; }
	.row {
		display: grid; grid-template-columns: minmax(0, 11rem) minmax(0, 1fr) auto; gap: 12px; align-items: center;
		background: none; border: 0; padding: 0; font-family: inherit; color: var(--td-text); cursor: pointer; text-align: left;
	}
	.row:hover .nm { color: var(--td-accent); }
	.nm { font-size: 13px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.track { height: 8px; border-radius: 999px; background: var(--td-track); overflow: hidden; }
	.fill { display: block; height: 100%; background: var(--td-accent); border-radius: 999px; }
	.n { font-size: 12.5px; font-variant-numeric: tabular-nums; }
	.of { color: var(--td-muted-3); }
	.caption { font-size: 12px; color: var(--td-muted-3); margin: 10px 0 0; }
</style>
