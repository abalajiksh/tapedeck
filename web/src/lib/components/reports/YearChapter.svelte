<script lang="ts">
	/**
	 * A year is a **bento grid, not a card** — the one place the design breaks
	 * the chapter shape, and rightly so. A year has no "scrobbles by week" to
	 * headline; what it has is a shape over twelve months and a handful of
	 * superlatives, and those want tiles of different sizes rather than one
	 * column of sections.
	 *
	 * Six columns, spans of 2 and 4. Below 900px it collapses to one, because a
	 * 2-of-6 tile at phone width is a strip.
	 */
	import type { ReportPeriod } from '$lib/api';
	import { openAlbum, openArtist } from '$lib/entities';
	import MonthLine from './MonthLine.svelte';
	import GenreBands from './GenreBands.svelte';
	import { nf, delta, deltaText, ordinal, barPct } from './bits';

	export let p: ReportPeriod;
	export let onShare: (() => void) | null = null;

	$: d = delta(p.plays, p.prev?.plays);
	$: rankText =
		p.rank === 1
			? 'Your busiest year on record'
			: p.rank > 0
				? `Your ${ordinal(p.rank)} busiest year of ${p.rank_of}`
				: '';
</script>

<div class="bento">
	<!-- Headline -->
	<section class="tile hero span2">
		<div>
			<div class="swatch" style="background:{p.colour ?? 'rgba(244,231,207,.14)'};"></div>
			<div class="h-display big">{nf(p.plays)}</div>
			<div class="unit">scrobbles in {p.label}</div>
		</div>
		<div class="foot">
			{#if d != null}
				<span class="d" class:up={d > 0} class:down={d < 0}>{deltaText(d)} vs {p.prev?.label}</span>
			{/if}
			<span class="hrs">{p.hours} hours</span>
			{#if rankText}<span class="rank">{rankText}</span>{/if}
		</div>
	</section>

	<!-- Shape of the year -->
	<section class="tile span4">
		<div class="head">
			<h3 class="h-display th">Scrobbles by month</h3>
			<div class="legend">
				<span class="item"><span class="line acc"></span>{p.label}</span>
				{#if p.prev}<span class="item"><span class="line prior"></span>{p.prev.label}</span>{/if}
			</div>
		</div>
		<MonthLine values={p.sub} previous={p.prev?.sub ?? null} labels={p.sub_labels} />
	</section>

	<!-- Genre bands -->
	<section class="tile span4">
		<h3 class="h-display th" style="margin-bottom:6px;">What you listened to, month by month</h3>
		<GenreBands names={p.genre_bands?.names ?? []} shares={p.genre_bands?.shares ?? []} labels={p.sub_labels} />
	</section>

	<!-- Top of the year -->
	<section class="tile span2">
		<div class="k">Top of the year</div>
		<div class="tops">
			{#if p.top_artist}
				<button class="top" on:click={() => openArtist(p.top_artist?.name ?? '')}>
					<span class="tk">Artist</span>
					<span class="tn">{p.top_artist.name}</span>
					<span class="ts">{nf(p.top_artist.plays)} plays</span>
				</button>
			{/if}
			{#if p.top_album}
				<button class="top" on:click={() => openAlbum(p.top_album?.artist ?? '', p.top_album?.name ?? '')}>
					<span class="tk">Album</span>
					<span class="tn">{p.top_album.name}</span>
					<span class="ts">{nf(p.top_album.plays)} plays</span>
				</button>
			{/if}
			{#if p.top_track}
				<div class="top">
					<span class="tk">Track</span>
					<span class="tn">{p.top_track.title}</span>
					<span class="ts">{nf(p.top_track.plays)} plays</span>
				</div>
			{/if}
			{#if !p.top_artist && !p.top_album && !p.top_track}
				<p class="note">Nothing played this year.</p>
			{/if}
		</div>
	</section>

	<!-- Crush -->
	<section class="tile crush span2">
		{#if p.crush}
			<div>
				<div class="k light">Crush of the year</div>
				<button class="cname h-display" on:click={() => openArtist(p.crush?.name ?? '')}>{p.crush.name}</button>
				<p class="cblurb">Your most-played artist you had never heard before.</p>
			</div>
			<div class="h-display cn">{nf(p.crush.plays)}</div>
		{:else}
			<div>
				<div class="k light">Crush of the year</div>
				<p class="cblurb">
					Everything here was new — this is the earliest year on record, so there is
					nothing to have discovered against.
				</p>
			</div>
		{/if}
	</section>

	<!-- Discovery -->
	<section class="tile span2">
		<div class="k">You discovered</div>
		<div class="disc"><span class="h-display dn">{nf(p.new_artists)}</span><span class="du">new artists</span></div>
		<div class="disc"><span class="h-display dn">{nf(p.shelf.sides)}</span><span class="du">sides off the shelf</span></div>
		{#if p.milestones.length}
			<div class="miles">
				{#each p.milestones as m}<span class="milestone">{m}</span>{/each}
			</div>
		{/if}
	</section>

	<!-- Through what. A year is the span on which gear actually changes, so
	     this is the chapter where it says the most. -->
	{#if p.gear.chains.length}
		<section class="tile span3">
			<div class="k">Heard through</div>
			{#each p.gear.chains.slice(0, 5) as c}
				<div class="gearrow">
					<span class="gname">{c.name}</span>
					<span class="gbar"><span style="width:{barPct(c.hours, p.gear.chains[0].hours)}%"></span></span>
					<span class="gval">{c.hours >= 0.1 ? `${c.hours}h` : `${nf(c.plays)}×`}</span>
				</div>
			{/each}
			{#if p.gear.unattributed > 0}
				<p class="caveat">
					{nf(p.gear.unattributed)} listens ran through nothing recorded and are not here.
				</p>
			{/if}
		</section>
	{/if}

	{#if p.gear.items.length}
		<section class="tile span3">
			<div class="k">On what</div>
			{#each p.gear.items.slice(0, 5) as g}
				<div class="gearrow">
					<span class="gname">{g.name}</span>
					<span class="gbar"><span style="width:{barPct(g.hours, p.gear.items[0].hours)}%"></span></span>
					<span class="gval">{g.hours >= 0.1 ? `${g.hours}h` : `${nf(g.plays)}×`}</span>
				</div>
			{/each}
		</section>
	{/if}

	<!-- Counted, never a share. Nothing here claims the rest was solitary. -->
	{#if p.company.marked_plays > 0}
		<section class="tile span2">
			<div class="k">In company</div>
			<div class="disc">
				<span class="h-display dn">{nf(p.company.marked_plays)}</span>
				<span class="du">listens marked as heard with somebody, of {nf(p.company.of_plays)}</span>
			</div>
			<div class="miles">
				{#each p.company.people.slice(0, 6) as w}<span class="milestone">{w.name}</span>{/each}
			</div>
		</section>
	{/if}
</div>

{#if onShare}
	<div class="actions">
		<button class="btn btn-primary" on:click={onShare}>
			<svg viewBox="0 0 24 24" width="15" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="18" cy="5" r="3"/><circle cx="6" cy="12" r="3"/><circle cx="18" cy="19" r="3"/><path d="M8.6 13.5l6.8 4M15.4 6.5l-6.8 4"/></svg>Share chapter
		</button>
	</div>
{/if}

<style>
	.bento { display: grid; grid-template-columns: repeat(6, 1fr); gap: 14px; }
	.gearrow {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 56px auto;
		align-items: center;
		gap: 9px;
		font-size: 13px;
		margin-top: 7px;
	}
	.gname { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.gbar { height: 5px; border-radius: 999px; background: var(--td-track); overflow: hidden; }
	.gbar > span {
		display: block;
		height: 100%;
		border-radius: 999px;
		background: linear-gradient(90deg, var(--td-accent-2), var(--td-accent));
	}
	.gval { font-variant-numeric: tabular-nums; color: var(--td-muted-2); }
	.caveat { font-size: 11.5px; color: var(--td-muted-3); margin: 10px 0 0; line-height: 1.5; }
	.span2 { grid-column: span 2; }
	/* Half the grid. Two of these make a row, which is what the gear pair
	   wants — and it has to be *declared*: a `span3` used and defined nowhere
	   renders one column wide with nothing erroring and `svelte-check` clean,
	   the trap v0.42.0 recorded. */
	.span3 { grid-column: span 3; }
	.span4 { grid-column: span 4; }
	.tile {
		background: var(--td-surface); border: 1px solid var(--td-border);
		border-radius: 20px; padding: 20px 22px; min-width: 0;
	}
	.hero {
		background: linear-gradient(150deg, var(--td-panel), var(--td-panel2));
		border-color: transparent; color: #f4e7cf;
		display: flex; flex-direction: column; justify-content: space-between;
	}
	.swatch {
		width: 44px; height: 44px; border-radius: 13px; margin-bottom: 16px;
		box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.14);
	}
	.big { font-size: 52px; line-height: 0.9; }
	.unit { font-size: 14px; color: #c9a678; margin-top: 6px; }
	.foot { display: flex; flex-wrap: wrap; gap: 6px 12px; margin-top: 18px; font-size: 13px; font-weight: 600; }
	.d { color: #c9a678; }
	.d.up { color: #aebf92; }
	.d.down { color: #e0a07a; }
	.hrs { color: #c9a678; font-weight: 400; }
	.rank { flex-basis: 100%; color: #aebf92; font-weight: 600; font-size: 12.5px; }

	.head { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; margin-bottom: 6px; flex-wrap: wrap; }
	.th { font-size: 18px; margin: 0; }
	.legend { display: flex; gap: 14px; font-size: 12px; color: var(--td-muted-2); }
	.item { display: inline-flex; align-items: center; gap: 6px; }
	.line { width: 14px; height: 3px; border-radius: 2px; }
	.line.acc { background: var(--td-accent); }
	.line.prior { background: var(--td-muted-3); }

	.k {
		font-size: 11px; letter-spacing: 0.1em; text-transform: uppercase;
		color: var(--td-muted-2); margin-bottom: 12px;
	}
	.k.light { color: rgba(255, 255, 255, 0.82); }
	.tops { display: flex; flex-direction: column; gap: 12px; }
	.top {
		background: none; border: none; padding: 0; text-align: left;
		font: inherit; color: inherit; min-width: 0; cursor: default;
	}
	button.top { cursor: pointer; }
	button.top:hover .tn { color: var(--td-accent); }
	.tk { display: block; font-size: 11px; color: var(--td-muted-3); }
	.tn { display: block; font-weight: 700; font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.ts { display: block; font-size: 12px; color: var(--td-muted-2); }

	.crush {
		background: linear-gradient(135deg, var(--td-accent), var(--td-accent-hi));
		border-color: transparent; color: #fff;
		display: flex; flex-direction: column; justify-content: space-between;
	}
	.cname {
		font-size: 26px; color: #fff; background: none; border: none;
		padding: 0; cursor: pointer; text-align: left; line-height: 1.1;
	}
	.cname:hover { text-decoration: underline; text-underline-offset: 4px; }
	.cblurb { font-size: 12.5px; color: rgba(255, 255, 255, 0.9); margin: 6px 0 0; }
	.cn { font-size: 32px; margin-top: 14px; }

	.disc { display: flex; align-items: baseline; gap: 8px; margin-top: 10px; }
	.dn { font-size: 34px; }
	.du { font-size: 13px; color: var(--td-muted-2); }
	.miles { display: flex; gap: 6px; flex-wrap: wrap; margin-top: 12px; }
	.milestone {
		background: var(--td-good-bg); color: var(--td-good-fg);
		font-size: 11.5px; font-weight: 600; border-radius: 999px; padding: 4px 10px;
	}
	.note { font-size: 12.5px; color: var(--td-muted-3); margin: 0; }
	.actions { display: flex; justify-content: flex-end; margin-top: 16px; }

	/* A 2-of-6 tile is a strip on a phone. One column, and the charts keep
	   their aspect because they are viewBox-scaled SVG. */
	@media (max-width: 900px) {
		.bento { grid-template-columns: 1fr; }
		.span2, .span3, .span4 { grid-column: span 1; }
	}
</style>
