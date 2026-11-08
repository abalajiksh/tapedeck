<script lang="ts">
	/**
	 * The graphs under a report chapter, sized to the period:
	 *
	 * - a week: its sessions;
	 * - a month: who topped each week, and its sessions;
	 * - a year: daily volume, obsession waves, the long tail, and who topped
	 *   each month.
	 *
	 * Fetched apart from the chapter (`/api/v1/reports/graphs`) and keyed on
	 * the period by the page, like `PeriodNote`, so stepping never shows one
	 * period's graphs under another's chapter. The charts are the Statistics
	 * page's own components — one drawing of a long tail, not two.
	 */
	import { onMount } from 'svelte';
	import { api, type ReportGraphs } from '$lib/api';
	import VolumeChart from '$lib/components/charts/VolumeChart.svelte';
	import WavesChart from '$lib/components/charts/WavesChart.svelte';
	import LongTailChart from '$lib/components/charts/LongTailChart.svelte';
	import SessionsChart from '$lib/components/charts/SessionsChart.svelte';
	import ThroneChart from '$lib/components/charts/ThroneChart.svelte';

	export let unit: 'week' | 'month' | 'year';
	export let periodKey: string;

	let data: ReportGraphs | null = null;
	let failed = false;

	onMount(async () => {
		try {
			data = await api.getReportGraphs(unit, periodKey);
		} catch (e) {
			failed = true;
			console.warn('report graphs', e);
		}
	});
	$: g = data?.graphs;
</script>

{#if failed}
	<p class="note">The graphs for this period could not be loaded.</p>
{:else if !data || !g}
	<div class="tile wait">Drawing the graphs…</div>
{:else if g.volume.plays}
	<section class="graphs">
		{#if unit === 'year' && g.start_day}
			<div class="tile">
				<h3 class="h-display th">Every day of the year</h3>
				<p class="lede">
					<strong>{g.volume.plays.toLocaleString()}</strong> listens over
					<strong>{g.volume.active_days.toLocaleString()}</strong> days you listened.
				</p>
				<VolumeChart start={g.start_day} daily={g.daily} median={g.volume.median_active_day} peak={g.volume.peak} />
			</div>
			{#if g.waves.length}
				<div class="tile">
					<h3 class="h-display th">Obsession waves</h3>
					<WavesChart start={g.start_day} waves={g.waves}>
						<svelte:fragment slot="caption">
							Listens a day, averaged over the previous seven, for the {g.waves.length} artists you played
							most this year. Click a name to open them.
						</svelte:fragment>
					</WavesChart>
				</div>
			{/if}
		{/if}

		{#if data.throne.length}
			<div class="tile">
				<h3 class="h-display th">Who held the throne</h3>
				<p class="lede">Your most-played artist {unit === 'year' ? 'each month' : 'each week'}.</p>
				<ThroneChart parts={data.throne} per={unit === 'year' ? 'month' : 'week'} />
			</div>
		{/if}

		{#if unit === 'year'}
			<div class="tile">
				<h3 class="h-display th">The long tail</h3>
				<p class="lede">
					<strong>{g.long_tail.once.toLocaleString()}</strong> of {g.long_tail.counts.length.toLocaleString()}
					artists heard once this year{#if g.long_tail.hundred_plus} · <strong>{g.long_tail.hundred_plus.toLocaleString()}</strong>
					heard 100 times or more{/if}.
				</p>
				<LongTailChart counts={g.long_tail.counts} credits={g.long_tail.credits} half={g.long_tail.half} eighty={g.long_tail.eighty} height={240} />
			</div>
		{:else}
			<div class="tile">
				<h3 class="h-display th">Sessions</h3>
				<SessionsChart total={g.sessions.total} gapMinutes={g.sessions.gap_minutes} lengths={g.sessions.lengths} devotion={g.sessions.devotion} height={180} />
			</div>
		{/if}
	</section>
{/if}

<style>
	.graphs { display: flex; flex-direction: column; gap: 14px; margin-top: 14px; }
	/* The chapters' tile, repeated: Svelte scopes styles, so the class has to
	   be defined where it is used. */
	.tile { background: var(--td-surface); border: 1px solid var(--td-border); border-radius: 20px; padding: 20px 22px; min-width: 0; }
	.wait { margin-top: 14px; color: var(--td-muted-3); font-size: 13px; }
	.th { font-size: 18px; margin: 0 0 6px; }
	.lede { margin: 0 0 12px; font-size: 13px; color: var(--td-muted); }
	.lede strong { color: var(--td-text); }
	.note { color: var(--td-muted-3); font-size: 12.5px; margin: 14px 0 0; }
</style>
