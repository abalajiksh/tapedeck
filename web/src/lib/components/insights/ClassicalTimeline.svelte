<script lang="ts">
	/**
	 * Your classical listening on a timeline, after Chronologue: one column per
	 * composer in order of birth, years running down, and each piece you heard
	 * as a dash at the year it was composed — thicker the more you played it.
	 *
	 * A piece MusicBrainz gives no year stands at its composer's mid-career and
	 * is drawn hollow, so a guess never passes for a date. Columns take their
	 * period's colour from the theme's series, not a hand-picked rainbow.
	 */
	import { createEventDispatcher } from 'svelte';
	import type { ClassicalTimeline } from '$lib/api';

	export let timeline: ClassicalTimeline;

	const dispatch = createEventDispatcher<{ piece: { key: string; title: string } }>();
	const COL = 34;
	const PX_PER_YEAR = 3;

	function period(year: number): string {
		if (year < 1600) return 'var(--td-series-6)';
		if (year < 1750) return 'var(--td-series-1)';
		if (year < 1820) return 'var(--td-series-2)';
		if (year < 1900) return 'var(--td-series-3)';
		if (year < 1945) return 'var(--td-series-4)';
		if (year < 1975) return 'var(--td-series-5)';
		return 'var(--td-accent)';
	}

	$: years = timeline.pieces.map((p) => p.year);
	$: first = Math.floor((Math.min(...years, 9999) - 10) / 10) * 10;
	$: last = Math.ceil((Math.max(...years, 0) + 10) / 10) * 10;
	$: height = Math.max(120, (last - first) * PX_PER_YEAR);
	$: decades = first < last ? Array.from({ length: (last - first) / 10 + 1 }, (_, i) => first + i * 10) : [];
	$: col = new Map(timeline.composers.map((c, i) => [c.key, i]));
	$: maxPlays = Math.max(1, ...timeline.pieces.map((p) => p.plays));
	$: width = timeline.composers.length * COL;
	const thickness = (plays: number) => 2 + Math.round((plays / maxPlays) * 6);
</script>

{#if timeline.pieces.length}
	<div class="wrap panel">
		<div class="scroller">
			<div class="board" style="width:{width + 52}px">
				<div class="heads" style="margin-left:52px">
					{#each timeline.composers as c}
						<div class="head" style="width:{COL}px;background:{period((c.born ?? 1700) + 35)}" title="{c.name}{c.born ? ` (b. ${c.born})` : ''}">
							<span>{c.name}</span>
						</div>
					{/each}
				</div>
				<div class="body" style="height:{height}px">
					{#each decades as d}
						<div class="decade" style="top:{(d - first) * PX_PER_YEAR}px"><span>{d}</span></div>
					{/each}
					{#each timeline.pieces as p}
						{@const i = col.get(p.composer) ?? 0}
						<button class="dash" class:hollow={!p.dated}
							style="left:{52 + i * COL + 4}px;top:{(p.year - first) * PX_PER_YEAR}px;height:{thickness(p.plays)}px;--c:{period(p.year)}"
							title="{p.title} — {p.dated ? p.year : `undated, placed at ${p.year}`} · {p.plays} {p.plays === 1 ? 'play' : 'plays'}"
							on:click={() => dispatch('piece', { key: p.key, title: p.title })}></button>
					{/each}
				</div>
			</div>
		</div>
		<p class="foot">
			{timeline.pieces.length} pieces by {timeline.composers.length} composers. Solid dashes stand at the year MusicBrainz gives;
			hollow ones have no date and stand at their composer's mid-career. Thicker is more played.
			{#if timeline.unplaced}{timeline.unplaced} pieces have no composer or date to place them by.{/if}
		</p>
	</div>
{/if}

<style>
	.wrap { border-radius: 22px; padding: 14px 0 12px; overflow: hidden; }
	.scroller { overflow-x: auto; overflow-y: hidden; padding: 0 14px; }
	.board { position: relative; }
	.heads { display: flex; gap: 0; }
	.head {
		height: 150px; display: flex; align-items: flex-end; justify-content: center;
		border-right: 2px solid var(--td-surface); border-radius: 6px 6px 0 0; padding-bottom: 8px;
	}
	.head span {
		writing-mode: vertical-rl; transform: rotate(180deg); font-size: 12px; font-weight: 600;
		color: var(--td-on-accent); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-height: 136px;
	}
	.body { position: relative; }
	.decade { position: absolute; left: 0; right: 0; border-top: 1px dashed var(--td-border); }
	.decade span { position: absolute; left: 0; top: -8px; font-size: 10.5px; color: var(--td-muted-3); background: var(--td-surface); padding-right: 6px; }
	.dash {
		position: absolute; width: 26px; border: none; padding: 0; border-radius: 2px;
		background: var(--c); cursor: pointer;
	}
	.dash.hollow { background: transparent; box-shadow: inset 0 0 0 1px var(--c); }
	.dash:hover { outline: 2px solid var(--td-text); }
	.foot { font-size: 12px; color: var(--td-muted); margin: 10px 14px 0; }
</style>
