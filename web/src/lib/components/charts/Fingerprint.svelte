<script lang="ts">
	/**
	 * A period's shape, against the reader's own usual.
	 *
	 * **The second shape is you, not other people.** The chart this borrows its
	 * form from plots a listener against a "global average"; the equivalent here
	 * needs other members' listening and quietly exposes how much they play,
	 * which is the rule Reports settled when it refused the cross-user
	 * percentile and used a rank against your own periods instead.
	 *
	 * Hand-built arcs rather than LayerCake, for the reason `ListeningClock` is:
	 * there is no scale to compute beyond value-to-radius, and the marks are
	 * ordinary SVG styled with `--td-*` so they follow the theme.
	 */
	export let axes: string[] = [];
	export let you: number[] = [];
	export let usual: number[] = [];
	/** How many periods the "usually" shape rests on. One is not a habit. */
	export let usualPeriods = 0;
	export let size = 260;

	const R = 92;
	const C = 120;

	/** Point on the web for axis `i` at value `v` (0..1). */
	function pt(i: number, v: number, n: number): [number, number] {
		// Start at twelve o'clock and go clockwise, which is how a radar reads.
		const a = (Math.PI * 2 * i) / n - Math.PI / 2;
		const r = R * Math.max(0, Math.min(1, v));
		return [C + Math.cos(a) * r, C + Math.sin(a) * r];
	}
	const poly = (vals: number[]) =>
		vals.map((v, i) => pt(i, v, vals.length).join(',')).join(' ');

	$: n = axes.length;
	/** Only draw the comparison when it stands for something. */
	$: showUsual = usualPeriods >= 2 && usual.length === n;
	$: label = (i: number) => pt(i, 1.22, n);
</script>

<div class="fp" style="--fp-size:{size}px">
	<svg viewBox="0 0 240 240" role="img" aria-label="Listening fingerprint">
		<!-- The web. Four rings is enough to read a shape against without the
		     grid competing with the marks. -->
		{#each [0.25, 0.5, 0.75, 1] as ring}
			<polygon
				points={poly(Array(n).fill(ring))}
				fill="none"
				stroke="var(--td-border)"
				stroke-width="1"
			/>
		{/each}
		{#each Array(n) as _, i}
			{@const [x, y] = pt(i, 1, n)}
			<line x1={C} y1={C} x2={x} y2={y} stroke="var(--td-border)" stroke-width="1" />
		{/each}

		{#if showUsual}
			<polygon
				points={poly(usual)}
				fill="var(--td-muted-3)"
				fill-opacity="0.14"
				stroke="var(--td-muted-3)"
				stroke-width="1.5"
				stroke-dasharray="4 3"
			/>
		{/if}

		<polygon
			points={poly(you)}
			fill="var(--td-accent)"
			fill-opacity="0.22"
			stroke="var(--td-accent)"
			stroke-width="2"
			stroke-linejoin="round"
		/>
		{#each you as v, i}
			{@const [x, y] = pt(i, v, n)}
			<circle cx={x} cy={y} r="3" fill="var(--td-accent)" />
		{/each}

		{#each axes as a, i}
			{@const [x, y] = label(i)}
			<text
				{x}
				{y}
				text-anchor={x > C + 4 ? 'start' : x < C - 4 ? 'end' : 'middle'}
				dominant-baseline={y > C + 40 ? 'hanging' : y < C - 40 ? 'auto' : 'middle'}
				fill="var(--td-muted-2)"
				font-size="10.5"
			>{a}</text>
		{/each}
	</svg>

	<div class="key">
		<span class="sw you"></span> This period
		{#if showUsual}
			<span class="sw usual" style="margin-left:8px;"></span> You, usually
		{/if}
	</div>
	{#if !showUsual}
		<!-- Said rather than left to be inferred from a missing shape. -->
		<p class="note">
			Not enough history yet to say what your usual shape is — one period is not
			a habit.
		</p>
	{/if}
</div>

<style>
	.fp { display: flex; flex-direction: column; align-items: center; }
	svg { width: var(--fp-size); max-width: 100%; height: auto; overflow: visible; }
	.key {
		display: flex; align-items: center; gap: 6px; flex-wrap: wrap;
		font-size: 11.5px; color: var(--td-muted-2); margin-top: 6px;
	}
	.sw { width: 10px; height: 10px; border-radius: 3px; display: inline-block; }
	.sw.you { background: var(--td-accent); }
	.sw.usual { background: var(--td-muted-3); }
	.note {
		font-size: 11.5px; color: var(--td-muted-3); margin: 6px 0 0;
		max-width: 34ch; text-align: center; line-height: 1.5;
	}
</style>
