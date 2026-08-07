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
