<script lang="ts">
	/**
	 * What your listening is made of, month by month: hours of classical,
	 * score & ambient, and everything else, stacked — each listen counted once,
	 * classical first. Statistics counts every listen in all its other figures;
	 * this is the one chart that says what share was which.
	 *
	 * The seasons are not a band of their own — a Christmas song is also pop,
	 * or also classical — so they sit as a strip under each month instead.
	 */
	import type { CompositionMonth } from '$lib/api';

	export let months: CompositionMonth[] = [];

	const H = 160;
	const BANDS = [
		{ k: 'classical', label: 'Classical', colour: 'var(--td-series-1)' },
		{ k: 'score', label: 'Scores & Ambient', colour: 'var(--td-series-2)' },
		{ k: 'other', label: 'Everything else', colour: 'var(--td-track)' }
	] as const;
	const SEASONS = [
		{ k: 'christmas', label: 'Christmas', colour: 'var(--td-series-4)' },
		{ k: 'halloween', label: 'Halloween', colour: 'var(--td-series-5)' },
		{ k: 'easter', label: 'Easter', colour: 'var(--td-series-6)' }
	] as const;

	$: peak = Math.max(1, ...months.map((m) => m.seconds));
	$: total = months.reduce((n, m) => n + m.seconds, 0);
	$: share = (k: 'classical' | 'score') =>
		total > 0 ? Math.round((months.reduce((n, m) => n + m[k], 0) / total) * 100) : 0;
	$: seasonal = SEASONS.filter((s) => months.some((m) => m[s.k] > 0));
	const h = (s: number) => Math.round(s / 360) / 10;
	const label = (m: string) => new Date(`${m}-15T12:00:00`).toLocaleDateString(undefined, { month: 'short' });
	const year = (m: string) => m.slice(0, 4);
</script>

{#if months.length}
	<p class="lede">
		<strong>{share('classical')}%</strong> classical and <strong>{share('score')}%</strong> score & ambient, by
		time listened.
	</p>
	<div class="chart" style="--n:{months.length}">
		{#each months as m, i}
			<div class="col" title="{label(m.month)} {year(m.month)}: {h(m.seconds)}h — {h(m.classical)}h classical, {h(m.score)}h score & ambient">
				<div class="stack" style="height:{(m.seconds / peak) * H}px">
					{#each BANDS as b}
						{#if m[b.k] > 0}<div style="flex:{m[b.k]};background:{b.colour}"></div>{/if}
					{/each}
				</div>
				<div class="seasons">
					{#each seasonal as s}
						<div style="opacity:{m.seconds ? Math.min(1, (m[s.k] / m.seconds) * 4) : 0};background:{s.colour}"></div>
					{/each}
				</div>
				{#if months.length <= 14 || i % 3 === 0}
					<div class="x">{label(m.month)}{#if i === 0 || m.month.endsWith('-01')} <span class="yr">{year(m.month)}</span>{/if}</div>
				{/if}
			</div>
		{/each}
	</div>
	<div class="legend">
		{#each BANDS as b}<span><i style="background:{b.colour}"></i>{b.label}</span>{/each}
		{#each seasonal as s}<span><i class="thin" style="background:{s.colour}"></i>{s.label} (strip: its share of the month)</span>{/each}
	</div>
{/if}

<style>
	.lede { margin: 0 0 14px; font-size: 13.5px; color: var(--td-muted); }
	.lede strong { color: var(--td-text); }
	.chart { display: grid; grid-template-columns: repeat(var(--n), minmax(0, 1fr)); gap: 3px; align-items: end; }
	.col { display: flex; flex-direction: column; justify-content: flex-end; min-width: 0; }
	.stack { display: flex; flex-direction: column-reverse; border-radius: 4px 4px 0 0; overflow: hidden; min-height: 1px; }
	.stack > div { min-height: 0; }
	.seasons { display: flex; flex-direction: column; gap: 1px; margin-top: 2px; }
	.seasons > div { height: 3px; border-radius: 2px; }
	.x { font-size: 10.5px; color: var(--td-muted-3); margin-top: 4px; white-space: nowrap; overflow: hidden; }
	.yr { color: var(--td-muted-2); }
	.legend { display: flex; flex-wrap: wrap; gap: 14px; margin-top: 12px; font-size: 12px; color: var(--td-muted); }
	.legend i { display: inline-block; width: 10px; height: 10px; border-radius: 3px; margin-right: 6px; vertical-align: -1px; }
	.legend i.thin { height: 4px; vertical-align: 2px; }
</style>
