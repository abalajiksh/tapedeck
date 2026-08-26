<script lang="ts">
	/**
	 * What you listened to, month by month — the year's genres as stacked bands.
	 *
	 * The series are the year's three biggest genres *by name*, fixed across
	 * every month, plus "other" as the remainder. The server does that naming
	 * (`genre_bands` in `reports.rs`) rather than the client picking per month:
	 * a stacked area whose series change identity partway along is unreadable,
	 * and the shape would imply a shift that is only a relabelling.
	 *
	 * These are shares of **tag mentions**, not of listens — one track carries
	 * several tags. The caller says so; do not drop that line.
	 */
	export let names: string[] = [];
	export let shares: number[][] = [];
	export let labels: string[] = [];

	const W = 640;
	const H = 210;
	const PAD_X = 22;
	const TOP = 14;
	const BASE = 182;

	/** Three accents plus a muted "other", matching the design's four. */
	const COLOURS = ['var(--td-accent)', 'var(--td-accent-2)', '#b7a318', 'var(--td-muted-3)'];

	$: n = shares.length;
	$: x = (i: number) => (n > 1 ? PAD_X + (i / (n - 1)) * (W - PAD_X * 2) : W / 2);
	$: yOf = (frac: number) => BASE - frac * (BASE - TOP);

	/** Cumulative stack: each band runs along its own top and back along the
	 *  one below, so they meet exactly instead of overlapping on a seam. */
	$: bands = (() => {
		if (!n) return [];
		const seriesCount = shares[0]?.length ?? 0;
		const cum = new Array(n).fill(0);
		const out: { d: string; color: string }[] = [];
		for (let s = 0; s < seriesCount; s++) {
			const bottom = [...cum];
			const top = cum.map((c, i) => c + (shares[i]?.[s] ?? 0));
			let d = `M${x(0).toFixed(1)},${yOf(top[0]).toFixed(1)}`;
			for (let i = 1; i < n; i++) d += ` L${x(i).toFixed(1)},${yOf(top[i]).toFixed(1)}`;
			for (let i = n - 1; i >= 0; i--) d += ` L${x(i).toFixed(1)},${yOf(bottom[i]).toFixed(1)}`;
			out.push({ d: `${d} Z`, color: COLOURS[s] ?? 'var(--td-muted-3)' });
			for (let i = 0; i < n; i++) cum[i] = top[i];
		}
		return out;
	})();

	/**
	 * A name written into the ribbon itself, where the ribbon is thick enough
	 * to hold it.
	 *
	 * The legend below says which colour is which; this says it *in place*,
	 * which is the difference between a chart you decode and one you read — and
	 * it is the one thing Last.fm's version of this does better.
	 *
	 * Only where the band is genuinely thick: below `MIN_BAND` the text would
	 * overflow its own ribbon and sit on the neighbours, which is worse than no
	 * label. `other` is never labelled — it is a residue, not a genre, and
	 * naming it would present it as one.
	 *
	 * The threshold is a fraction of the plot's height rather than a pixel
	 * count so it holds if the chart is resized, and it is set from the one
	 * measurement that matters: the plot is 168 units tall, so 0.12 is ~20 of
	 * them against an 11.5-unit line. Set higher it silently drops bands with
	 * ample room — 0.16 discarded one occupying 27 units, which is what
	 * prompted this number.
	 */
	const MIN_BAND = 0.12;
	$: bandLabels = (() => {
		if (!n) return [];
		const seriesCount = shares[0]?.length ?? 0;
		const cum = new Array(n).fill(0);
		const out: { x: number; y: number; name: string }[] = [];
		for (let s = 0; s < seriesCount; s++) {
			const bottom = [...cum];
			const top = cum.map((c, i) => c + (shares[i]?.[s] ?? 0));
			for (let i = 0; i < n; i++) cum[i] = top[i];
			// `names` covers the real genres; the last series is `other`.
			const name = names[s];
			if (!name) continue;
			// The widest point of this band, which is where a word fits.
			let best = -1;
			let bestThick = 0;
			for (let i = 0; i < n; i++) {
				const thick = top[i] - bottom[i];
				if (thick > bestThick) {
					bestThick = thick;
					best = i;
				}
			}
			if (best < 0 || bestThick < MIN_BAND) continue;
			out.push({
				x: x(best),
				y: (yOf(top[best]) + yOf(bottom[best])) / 2,
				name
			});
		}
		return out;
	})();

	$: legend = [...names, 'other'].map((name, i) => ({
		name,
		color: COLOURS[i] ?? 'var(--td-muted-3)'
	}));
	$: empty = !names.length;
</script>

{#if empty}
	<p class="note">Genres fill in as listens are enriched from MusicBrainz.</p>
{:else}
	<svg viewBox="0 0 {W} {H}" style="width:100%;height:auto;display:block;" role="img"
		aria-label="Genre share by month">
		{#each bands as b}
			<path d={b.d} fill={b.color} opacity="0.88" />
		{/each}
		{#each bandLabels as b}
			<text x={b.x} y={b.y} text-anchor="middle" dominant-baseline="middle"
				font-size="11.5" font-weight="600" font-family="inherit"
				fill="var(--td-ink-on-accent, #17120e)"
				paint-order="stroke" stroke="rgba(0,0,0,.18)" stroke-width="2.5"
			>{b.name}</text>
		{/each}
		{#each labels as l, i}
			<text x={x(i)} y={H - 4} text-anchor="middle" font-size="11"
				fill="var(--td-muted-3)" font-family="inherit">{l}</text>
		{/each}
	</svg>
	<div class="legend">
		{#each legend as l}
			<span class="item"><span class="dot" style="background:{l.color};"></span>{l.name}</span>
		{/each}
		<span class="caveat">· share of tag mentions, not of plays</span>
	</div>
{/if}

<style>
	.legend {
		display: flex; gap: 16px; flex-wrap: wrap; margin-top: 8px;
		font-size: 12px; color: var(--td-muted-2);
	}
	.item { display: inline-flex; align-items: center; gap: 6px; }
	.dot { width: 11px; height: 11px; border-radius: 3px; }
	.caveat { color: var(--td-muted-3); }
	.note { font-size: 12.5px; color: var(--td-muted-3); margin: 0; }
</style>
