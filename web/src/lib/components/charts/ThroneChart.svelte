<script lang="ts">
	/**
	 * The throne: who topped each week of a month, or each month of a year,
	 * and by how much — their share of everything played in that part.
	 *
	 * Every bar names its artist, so colour is a second cue rather than the
	 * only one: an artist who held the throne more than once keeps one series
	 * colour across all their reigns (most reigns first, at most six), and a
	 * one-off winner is drawn in a neutral tone. Colour follows the artist,
	 * never the row.
	 */
	import { SERIES, nf } from './graphs';
	import { openArtist } from '$lib/entities';

	export let parts: { start: string; label: string; artist?: string; plays: number; listens: number }[] = [];
	/** What one part is: a month's parts are weeks, a year's are months. */
	export let per: 'week' | 'month' = 'month';

	$: colours = (() => {
		const wins = new Map<string, number>();
		for (const p of parts) if (p.artist) wins.set(p.artist, (wins.get(p.artist) ?? 0) + 1);
		const repeat = [...wins.entries()]
			.filter(([, n]) => n > 1)
			.sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
			.slice(0, SERIES.length);
		return new Map(repeat.map(([name], i) => [name, SERIES[i]]));
	})();
	const share = (p: { plays: number; listens: number }) => (p.listens ? p.plays / p.listens : 0);
</script>

{#if parts.some((p) => p.artist)}
	<div class="rows">
		{#each parts as p (p.start)}
			<div class="row">
				<span class="when">{p.label}</span>
				{#if p.artist}
					<button class="bar" on:click={() => p.artist && openArtist(p.artist)}
						title="{p.artist}: {nf(p.plays)} of {nf(p.listens)} listens">
						<span class="fill" style="width:{Math.max(1, share(p) * 100)}%;background:{colours.get(p.artist) ?? 'var(--td-muted-3)'};"></span>
						<span class="who">
							<strong>{p.artist}</strong>
							<span class="sub">{nf(p.plays)} of {nf(p.listens)} · {Math.round(share(p) * 100)}%</span>
						</span>
					</button>
				{:else}
					<span class="none">nothing played</span>
				{/if}
			</div>
		{/each}
	</div>
	<p class="caption">
		The bar is their share of everything you played that {per}.
		An artist who held it more than once keeps one colour.
	</p>
{/if}

<style>
	.rows { display: flex; flex-direction: column; gap: 6px; }
	.row { display: grid; grid-template-columns: 5.5rem minmax(0, 1fr); gap: 12px; align-items: center; }
	.when { font-size: 12.5px; color: var(--td-muted-2); text-align: right; font-variant-numeric: tabular-nums; }
	.bar {
		position: relative; display: flex; align-items: center; min-height: 30px; padding: 0; border: 0;
		background: var(--td-surface2); border-radius: 8px; overflow: hidden; cursor: pointer; font-family: inherit;
		text-align: left; color: var(--td-text);
	}
	.fill { position: absolute; left: 0; top: 0; bottom: 0; opacity: 0.38; border-radius: 8px 0 0 8px; }
	.who { position: relative; padding: 5px 10px; font-size: 12.5px; display: flex; gap: 8px; align-items: baseline; min-width: 0; }
	.who strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.bar:hover strong { color: var(--td-accent); }
	.sub { color: var(--td-muted); font-size: 11.5px; white-space: nowrap; }
	.none { font-size: 12px; color: var(--td-muted-3); font-style: italic; padding: 6px 0; }
	.caption { font-size: 12px; color: var(--td-muted-3); margin: 10px 0 0; }
</style>
