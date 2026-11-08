<script lang="ts">
	/**
	 * Listening sessions: how long they run, and whether one artist has them.
	 *
	 * A session ends at a gap longer than `gapMinutes`. The devotion bar is one
	 * stacked bar rather than the script's pie — three parts of one whole read
	 * more exactly as lengths on a shared line than as angles — and it only
	 * counts sessions of `minListens` or more, because a one-song session is
	 * "all one artist" by definition.
	 */
	import { LayerCake, Svg } from 'layercake';
	import { scaleBand, scaleLinear } from 'd3-scale';
	import { nf } from './graphs';

	export let total = 0;
	export let gapMinutes = 30;
	export let lengths: { label: string; sessions: number }[] = [];
	export let devotion: { min_listens: number; devoted: number; mixed: number; scattered: number };
	export let height = 200;

	$: max = Math.max(1, ...lengths.map((l) => l.sessions));
	$: judged = devotion.devoted + devotion.mixed + devotion.scattered;
	/** Ordinal, one hue: most devoted darkest. */
	$: parts = [
		{ key: 'Devoted', n: devotion.devoted, note: '80% or more one artist', fill: 'var(--td-accent)', ink: 'var(--td-on-accent)' },
		{ key: 'Mixed', n: devotion.mixed, note: '40–80%', fill: 'color-mix(in srgb, var(--td-accent) 55%, var(--td-surface))', ink: 'var(--td-text)' },
		{ key: 'Scattered', n: devotion.scattered, note: 'under 40%', fill: 'color-mix(in srgb, var(--td-accent) 22%, var(--td-surface))', ink: 'var(--td-text)' }
	];
	const share = (n: number, of: number) => (of > 0 ? Math.round((n / of) * 100) : 0);
</script>

<div class="grid2">
	<div>
		<div class="k">How long they run · listens per session</div>
		<div style="height:{height}px;">
			<LayerCake padding={{ top: 16, right: 0, bottom: 22, left: 0 }}
				xScale={scaleBand().paddingInner(0.25)} yScale={scaleLinear()}
				xDomain={lengths.map((l) => l.label)} yDomain={[0, max]} data={[]}
				let:xScale let:yScale let:height={h}>
				<Svg>
					{#each lengths as l}
						{@const x = xScale(l.label) ?? 0}
						{@const bw = xScale.bandwidth()}
						<rect {x} y={yScale(l.sessions)} width={bw} height={h - yScale(l.sessions)}
							fill="var(--td-accent)" rx="3" opacity="0.9">
							<title>{l.label} listen{l.label === '1' ? '' : 's'}: {nf(l.sessions)} sessions</title>
						</rect>
						<text x={x + bw / 2} y={yScale(l.sessions) - 5} text-anchor="middle" class="val">{nf(l.sessions)}</text>
						<text x={x + bw / 2} y={h + 16} text-anchor="middle" class="tick">{l.label}</text>
					{/each}
				</Svg>
			</LayerCake>
		</div>
	</div>

	<div>
		<div class="k">One artist or many</div>
		{#if judged}
			<div class="bar" role="img"
				aria-label={parts.map((p) => `${p.key} ${share(p.n, judged)}%`).join(', ')}>
				{#each parts as p}
					{#if p.n > 0}
						<div class="seg" style="flex:{p.n};background:{p.fill};color:{p.ink};"
							title="{p.key}: {nf(p.n)} sessions ({share(p.n, judged)}%)">
							{#if share(p.n, judged) >= 12}{share(p.n, judged)}%{/if}
						</div>
					{/if}
				{/each}
			</div>
			<div class="keys">
				{#each parts as p}
					<div class="key">
						<span class="sw" style="background:{p.fill};"></span>
						<div>
							<div><strong>{p.key}</strong> · {nf(p.n)} <span class="sub">({share(p.n, judged)}%)</span></div>
							<div class="sub">{p.note}</div>
						</div>
					</div>
				{/each}
			</div>
			<p class="caption">
				Of the {nf(judged)} sessions with {devotion.min_listens} or more listens, by the share
				their most-played artist took.
			</p>
		{:else}
			<p class="caption">No session in this range ran to {devotion.min_listens} listens.</p>
		{/if}
	</div>
</div>
<p class="caption">
	{nf(total)} session{total === 1 ? '' : 's'}. A gap of more than {gapMinutes} minutes starts a new one.
</p>

<style>
	.grid2 { display: grid; grid-template-columns: minmax(0, 3fr) minmax(0, 2fr); gap: 28px; align-items: start; }
	@media (max-width: 760px) { .grid2 { grid-template-columns: 1fr; } }
	.k { font-size: 11px; letter-spacing: 0.12em; text-transform: uppercase; color: var(--td-muted-2); margin-bottom: 10px; }
	.val { font-size: 11px; fill: var(--td-text); font-family: inherit; font-variant-numeric: tabular-nums; }
	.tick { font-size: 11px; fill: var(--td-muted-3); font-family: inherit; }
	.bar { display: flex; gap: 2px; height: 30px; border-radius: 8px; overflow: hidden; }
	.seg { display: flex; align-items: center; justify-content: center; font-size: 12px; font-weight: 600; min-width: 2px; }
	.keys { display: flex; flex-direction: column; gap: 10px; margin-top: 14px; font-size: 13px; }
	.key { display: flex; gap: 10px; align-items: flex-start; }
	.sw { width: 12px; height: 12px; border-radius: 3px; flex: none; margin-top: 3px; }
	.sub { color: var(--td-muted-3); font-size: 12px; }
	.caption { font-size: 12px; color: var(--td-muted-3); margin: 10px 0 0; }
</style>
