<script lang="ts">
	/**
	 * Your classical listening in the shape of Bachtrack's annual statistics —
	 * period, form, composers year by year, living and women composers, where
	 * composers and performers come from, and who took you furthest — refitted
	 * from concerts to listens. Every share is of the classical hours in the
	 * window, and says so when its population is narrower.
	 */
	import type { ClassicalStats } from '$lib/api';
	import { createEventDispatcher } from 'svelte';

	export let stats: ClassicalStats;

	const dispatch = createEventDispatcher<{ composer: { key: string; name: string } }>();
	const SERIES = ['var(--td-series-1)', 'var(--td-series-2)', 'var(--td-series-3)', 'var(--td-series-4)', 'var(--td-series-5)'];

	const pct = (n: number, of: number) => (of > 0 ? Math.round((n / of) * 1000) / 10 : 0);
	const hrs = (s: number) => Math.round(s / 360) / 10;
	let regions: Intl.DisplayNames | null = null;
	try { regions = new Intl.DisplayNames(undefined, { type: 'region' }); } catch { regions = null; }
	const country = (c: string) => regions?.of(c) ?? c;

	$: periodMax = Math.max(1, ...stats.periods.map((p) => p.seconds));
	$: formMax = Math.max(1, ...stats.forms.map((p) => p.seconds));
	$: years = stats.years.years;
	$: lineMax = Math.max(1, ...stats.years.composers.flatMap((c) => c.plays));
	const W = 640;
	const H = 180;
	$: x = (i: number) => (years.length > 1 ? (i / (years.length - 1)) * (W - 40) + 20 : W / 2);
	$: y = (v: number) => H - 20 - (v / lineMax) * (H - 40);
	$: path = (plays: number[]) => plays.map((v, i) => `${i ? 'L' : 'M'}${x(i)},${y(v)}`).join(' ');
</script>

<div class="grid">
	<section class="panel card wide">
		<h3 class="h-display">By period</h3>
		<p class="sub">Share of your classical hours, by when a piece was composed —
			{pct(stats.dated, stats.seconds)}% placed by its composition year, the rest by its composer's mid-career.</p>
		<div class="periods">
			{#each stats.periods as p}
				<div class="pcol" title="{p.name}: {hrs(p.seconds)}h, {p.plays} plays">
					<div class="pval">{pct(p.seconds, stats.seconds)}%</div>
					<div class="pbar" style="height:{(p.seconds / periodMax) * 140}px"></div>
					<div class="plabel">{p.name}</div>
				</div>
			{/each}
		</div>
	</section>

	{#if years.length > 1 && stats.years.composers.length}
		<section class="panel card wide">
			<h3 class="h-display">Your composers, year by year</h3>
			<p class="sub">Plays of the five composers you played most, with their anniversaries marked.</p>
			<svg viewBox="0 0 {W} {H}" class="lines" role="img" aria-label="Plays per year of your top composers">
				{#each stats.years.composers as c, ci}
					<path d={path(c.plays)} fill="none" stroke={SERIES[ci % SERIES.length]} stroke-width="2.5" />
					{#each c.anniversaries as a}
						{@const i = years.indexOf(a.year)}
						{#if i >= 0}
							<circle cx={x(i)} cy={y(c.plays[i])} r="6" fill="none" stroke={SERIES[ci % SERIES.length]} stroke-width="2"><title>{a.label}</title></circle>
						{/if}
					{/each}
				{/each}
				{#each years as yr, i}
					<text x={x(i)} y={H - 4} text-anchor="middle" class="tick">{yr}</text>
				{/each}
			</svg>
			<div class="legend">
				{#each stats.years.composers as c, ci}
					<button class="leg" on:click={() => dispatch('composer', { key: c.key, name: c.name })}>
						<i style="background:{SERIES[ci % SERIES.length]}"></i>{c.name}
					</button>
				{/each}
			</div>
			{#if stats.years.composers.some((c) => c.anniversaries.length)}
				<ul class="ann">
					{#each stats.years.composers.flatMap((c) => c.anniversaries) as a}<li><b>{a.year}</b> {a.label}</li>{/each}
				</ul>
			{/if}
		</section>
	{/if}

	{#if stats.forms.length}
		<section class="panel card">
			<h3 class="h-display">By form</h3>
			<p class="sub">As MusicBrainz types each piece.</p>
			{#each stats.forms as f}
				<div class="hrow">
					<span class="hname">{f.name}</span>
					<div class="hbar"><div style="width:{(f.seconds / formMax) * 100}%"></div></div>
					<span class="hval">{hrs(f.seconds)}h</span>
				</div>
			{/each}
		</section>
	{/if}

	<section class="panel card">
		<h3 class="h-display">Living composers</h3>
		{#if stats.living.known}
			<div class="big">{pct(stats.living.seconds, stats.living.known)}%</div>
			<p class="sub">of the classical hours whose composer MusicBrainz describes ({pct(stats.living.known, stats.seconds)}% of all).</p>
			{#each stats.living.composers as c}<div class="who">{c.name} <span>{hrs(c.seconds)}h</span></div>{/each}
		{:else}
			<p class="sub">Tapedeck is still looking up who your composers are.</p>
		{/if}
	</section>

	<section class="panel card">
		<h3 class="h-display">Women composers</h3>
		{#if stats.women.known}
			<div class="big">{pct(stats.women.seconds, stats.women.known)}%</div>
			<p class="sub">of the described classical hours are by women and non-binary composers.</p>
			{#each stats.women.composers as c}<div class="who">{c.name} <span>{hrs(c.seconds)}h</span></div>{/each}
			{#if !stats.women.composers.length}<p class="sub">None yet in this window.</p>{/if}
		{:else}
			<p class="sub">Tapedeck is still looking up who your composers are.</p>
		{/if}
	</section>

	{#if stats.composer_countries.length}
		<section class="panel card">
			<h3 class="h-display">Where your composers are from</h3>
			{#each stats.composer_countries as c}
				<div class="who">{country(c.name)} <span>{hrs(c.seconds)}h · {c.count} {c.count === 1 ? 'composer' : 'composers'}</span></div>
			{/each}
		</section>
	{/if}

	{#if stats.performer_countries.length}
		<section class="panel card">
			<h3 class="h-display">Where your performers are from</h3>
			{#each stats.performer_countries as c}
				<div class="who">{country(c.name)} <span>{c.plays} plays · {c.count} {c.count === 1 ? 'performer' : 'performers'}</span></div>
			{/each}
		</section>
	{/if}

	{#if stats.range.length}
		<section class="panel card wide">
			<h3 class="h-display">Who took you furthest</h3>
			<p class="sub">Performers by how many composers you heard them play — Bachtrack counts the countries a conductor toured; this counts the composers they took you to.</p>
			{#each stats.range as r, i}
				<div class="range">
					<span class="rk h-display">{i + 1}</span>
					<span class="rname">{r.name} <span class="role">{r.role}</span></span>
					<span class="rval">{r.composers} composers · {r.pieces} pieces</span>
				</div>
			{/each}
		</section>
	{/if}
</div>

<style>
	.grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 16px; }
	.card { border-radius: 22px; padding: 20px; min-width: 0; }
	.wide { grid-column: 1 / -1; }
	h3 { font-size: 19px; }
	.sub { font-size: 12.5px; color: var(--td-muted); margin: 4px 0 14px; }
	.periods { display: grid; grid-template-columns: repeat(7, minmax(0, 1fr)); gap: 8px; align-items: end; }
	.pcol { display: flex; flex-direction: column; align-items: center; justify-content: flex-end; min-width: 0; }
	.pval { font-size: 12px; font-weight: 600; margin-bottom: 4px; font-variant-numeric: tabular-nums; }
	.pbar { width: 100%; border-radius: 8px 8px 0 0; background: linear-gradient(180deg, var(--td-accent), var(--td-accent-2)); min-height: 2px; }
	.plabel { font-size: 11px; color: var(--td-muted); margin-top: 6px; text-align: center; line-height: 1.2; }
	.lines { width: 100%; height: auto; display: block; }
	.tick { font-size: 11px; fill: var(--td-muted-3); }
	.legend { display: flex; flex-wrap: wrap; gap: 12px; margin-top: 8px; }
	.leg { border: none; background: none; font: inherit; font-size: 12.5px; color: var(--td-text); cursor: pointer; padding: 0; }
	.leg i { display: inline-block; width: 10px; height: 10px; border-radius: 3px; margin-right: 6px; vertical-align: -1px; }
	.ann { margin: 10px 0 0; padding: 0; list-style: none; font-size: 12.5px; color: var(--td-muted); }
	.ann b { color: var(--td-text); margin-right: 6px; }
	.hrow { display: grid; grid-template-columns: 110px 1fr 54px; gap: 8px; align-items: center; margin-bottom: 6px; }
	.hname { font-size: 13px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.hbar { height: 8px; border-radius: 999px; background: var(--td-track); overflow: hidden; }
	.hbar > div { height: 100%; background: linear-gradient(90deg, var(--td-accent-2), var(--td-accent)); }
	.hval { font-size: 12px; text-align: right; color: var(--td-muted); font-variant-numeric: tabular-nums; }
	.big { font-family: 'Caprasimo', serif; font-size: 40px; line-height: 1; }
	.who { display: flex; justify-content: space-between; gap: 10px; font-size: 13px; padding: 4px 0; border-top: 1px solid var(--td-border); }
	.who span { color: var(--td-muted); white-space: nowrap; font-variant-numeric: tabular-nums; }
	.range { display: flex; align-items: baseline; gap: 12px; padding: 6px 0; border-top: 1px solid var(--td-border); font-size: 13.5px; }
	.rk { width: 22px; color: var(--td-muted-3); }
	.rname { flex: 1; min-width: 0; font-weight: 600; }
	.role { font-weight: 400; color: var(--td-muted); font-size: 12px; margin-left: 6px; }
	.rval { color: var(--td-muted); font-variant-numeric: tabular-nums; white-space: nowrap; }
	@media (max-width: 640px) { .periods { gap: 4px; } .plabel { font-size: 9.5px; } }
</style>
