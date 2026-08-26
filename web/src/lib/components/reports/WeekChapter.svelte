<script lang="ts">
	/**
	 * A week, as the design draws it: the compact sibling of the month card.
	 *
	 * Same head, smaller type, and only the pieces a week has enough data to
	 * support — one feature card rather than two, no clock, no genre breakdown.
	 * That is the design's own judgement and it is a sound one: seven days of
	 * listening make a thin hour-of-day distribution and a thinner genre split.
	 */
	import type { ReportPeriod } from '$lib/api';
	import { openAlbum } from '$lib/entities';
	import PeriodBars from '$lib/components/charts/PeriodBars.svelte';
	import ChapterHead from './ChapterHead.svelte';
	import TrendTrio from './TrendTrio.svelte';
	import ArtistHero from './ArtistHero.svelte';
	import FeatureCard from './FeatureCard.svelte';
	import CrushBand from './CrushBand.svelte';
	import { nf } from './bits';

	export let p: ReportPeriod;
	export let onShare: (() => void) | null = null;
</script>

<article class="panel card">
	<ChapterHead {p} unit="week" {onShare}>
		<p slot="blurb" class="blurb"><slot name="blurb" /></p>
		<svelte:fragment slot="chart">
			<div class="chart-k">Scrobbles by day</div>
			<PeriodBars
				values={p.sub}
				labels={p.sub_labels}
				previous={p.prev?.sub ?? null}
				currentLabel={p.label}
				previousLabel={p.prev?.label ?? 'Previous'}
			/>
		</svelte:fragment>
	</ChapterHead>

	{#if p.milestones.length}
		<div class="milestones">
			{#each p.milestones as m}<span class="milestone">{m}</span>{/each}
		</div>
	{/if}

	<TrendTrio
		size={28}
		rows={[
			{ label: 'Avg. daily', value: nf(p.avg_daily), now: p.avg_daily, before: p.prev?.avg_daily, was: p.prev ? nf(p.prev.avg_daily) : null },
			{ label: 'Listening time', value: `${p.hours}h`, now: p.hours, before: p.prev?.hours, was: p.prev ? `${p.prev.hours}h` : null },
			{ label: 'Artists', value: nf(p.artists), now: p.artists, before: p.prev?.artists, was: p.prev ? nf(p.prev.artists) : null }
		]}
	/>

	{#if p.top_artist || p.top_track}
		<div class="feature-grid">
			{#if p.top_artist}
				<ArtistHero artist={p.top_artist} ratio="16/10" initialSize={130} />
			{/if}
			{#if p.top_track}
				<FeatureCard
					kind="track"
					label="Track of the week"
					title={p.top_track.title}
					sub={`${p.top_track.artist} · ${nf(p.top_track.plays)} plays`}
					iconSize={60}
					onOpen={p.top_album ? () => openAlbum(p.top_album?.artist ?? '', p.top_album?.name ?? '') : null}
				/>
			{/if}
		</div>
	{/if}

	{#if p.crush}
		<CrushBand crush={p.crush} unit="week" compact />
	{/if}

	<!-- One line, not a panel. A week's gear split is two or three chains and
	     the design's whole argument for this chapter is that seven days do not
	     support the distributions a month does — so this says which, and how
	     much, and stops. Company rides on the same line for the same reason. -->
	{#if p.gear.chains.length || p.company.marked_plays > 0}
		<div class="through">
			{#if p.gear.chains.length}
				<span class="tk">Through</span>
				{#each p.gear.chains.slice(0, 3) as c, i}
					<span class="tv">{i ? ' · ' : ''}{c.name}
						<span class="tm">{c.hours >= 0.1 ? `${c.hours}h` : `${nf(c.plays)}×`}</span>
					</span>
				{/each}
			{/if}
			{#if p.company.marked_plays > 0}
				<span class="tk" style="margin-left:14px;">With</span>
				<span class="tv">
					{p.company.people.map((w) => w.name).slice(0, 3).join(', ') || 'somebody'}
					<span class="tm">{nf(p.company.marked_plays)} marked</span>
				</span>
			{/if}
		</div>
	{/if}

	<slot name="footer" />
</article>

<style>
	.card { border-radius: 24px; padding: 26px; }
	.blurb { font-size: 13.5px; color: var(--td-muted); margin: 4px 0 0; max-width: 52ch; }
	.chart-k { font-size: 12px; color: var(--td-muted-2); margin-bottom: 8px; }
	.feature-grid {
		display: grid; grid-template-columns: 1.3fr 1fr; gap: 14px; margin-bottom: 24px;
	}
	.milestones { display: flex; gap: 8px; flex-wrap: wrap; margin-bottom: 18px; }
	.milestone {
		background: var(--td-good-bg); color: var(--td-good-fg);
		font-size: 12px; font-weight: 600; border-radius: 999px; padding: 5px 12px;
	}
	.through {
		display: flex; flex-wrap: wrap; align-items: baseline; gap: 6px;
		font-size: 13px; margin-top: 18px; padding-top: 16px;
		border-top: 1px solid var(--td-border);
	}
	.tk {
		font-size: 11px; letter-spacing: .09em; text-transform: uppercase;
		color: var(--td-muted-2);
	}
	.tv { color: var(--td-text); }
	.tm { color: var(--td-muted-3); font-variant-numeric: tabular-nums; }
	@media (max-width: 760px) { .feature-grid { grid-template-columns: 1fr; } }
</style>
