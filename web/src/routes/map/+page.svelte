<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { GenreMap } from '$lib/api';

	let data: GenreMap | null = null;
	let loading = true;
	let error = '';
	/** The map is 15× taller than wide, so the overview compresses the vertical
	 *  axis. Off shows true proportions, which is faithful but only readable by
	 *  scrolling. */
	let trueScale = false;
	let hovered: { name: string; plays?: number } | null = null;

	onMount(async () => {
		try {
			data = await api.getGenreMap();
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		loading = false;
	});

	// The canvas is 1500 × 22,648 — fifteen times taller than wide, an
	// unreadable vertical smear at true proportions.
	//
	// Framing to the listener's own bounding box was the obvious fix and it does
	// not work: on a real history the listening spans almost the whole height
	// (y 1,048–21,730 of 22,648), so the framed aspect comes out *worse* than the
	// full map. A wide-ranging taste is exactly the case that needs this to work.
	//
	// So the vertical axis is compressed instead — in the coordinates, not with
	// `preserveAspectRatio`, which would squash the circles into ellipses along
	// with everything else. The distortion is real and the UI says so; the y axis
	// still orders genres organic-to-mechanical, it just does it in less space.
	const VIEW_H = 1150;
	$: yScale = data && !trueScale ? VIEW_H / data.map.height : 1;
	$: viewH = data ? (trueScale ? data.map.height : VIEW_H) : 0;

	// Every point is drawn: 6,291 circles is nothing for an SVG, and at this
	// compression the backdrop *is* the picture — the shape of the whole space.
	$: backdrop = data?.map.points ?? [];

	// Radius by play count, on a square root so one dominant genre doesn't
	// swallow the plot — area reads as quantity, and sqrt makes area linear.
	$: maxPlays = data?.mine.length ? Math.max(...data.mine.map((m) => m.plays)) : 1;
	$: unit = data ? Math.max(data.map.width, viewH) / 260 : 1;
	const radius = (plays: number, max: number, unit: number) =>
		unit * (2 + 9 * Math.sqrt(plays / Math.max(max, 1)));

	$: pending = data ? data.listens_total - data.listens_with_genres : 0;
	$: top = data ? [...data.mine].sort((a, b) => b.plays - a.plays).slice(0, 12) : [];
</script>

<svelte:head><title>Genre map · Tapedeck</title></svelte:head>

<h2 class="h-display" style="font-size:26px;margin:0 0 4px;">Genre map</h2>
<p style="font-size:13px;color:var(--td-muted);margin:0 0 20px;max-width:64ch;">
	Every genre Spotify knew, positioned by Glenn McDonald's Every Noise at Once — left to right
	runs dense and atmospheric to spiky and bouncy, top to bottom organic to mechanical. The lit
	points are yours, sized by how much you've played them.
</p>

{#if loading}
	<div style="display:flex;justify-content:center;padding:60px;">
		<div class="spin" style="width:22px;height:22px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div>
	</div>
{:else if error}
	<div class="panel" style="border-radius:22px;padding:24px;">
		<p style="font-size:13.5px;color:var(--td-muted);margin:0;">{error}</p>
	</div>
{:else if data}
	{#if data.colour}
		<!-- The colour is the point of the second feature: Every Noise encodes
		     audio character in the three channels, so this really is what your
		     listening sounds like averaged together. -->
		<div class="panel" style="border-radius:22px;padding:22px;margin-bottom:18px;display:flex;align-items:center;gap:18px;flex-wrap:wrap;">
			<div style="width:64px;height:64px;border-radius:16px;flex:none;background:{data.colour};box-shadow:inset 0 0 0 1px rgba(0,0,0,.12);"></div>
			<div style="flex:1;min-width:220px;">
				<div class="h-display" style="font-size:19px;margin-bottom:3px;">Your listening has a colour</div>
				<p style="font-size:13px;color:var(--td-muted);margin:0;">
					<code style="font-size:13px;">{data.colour}</code> — the average of
					{data.placed_tags.toLocaleString()} genre tags, weighted by plays. Every Noise encodes
					a genre's audio character in the red, green and blue channels, so this is roughly what
					all of your listening sounds like at once.
				</p>
			</div>
		</div>
	{/if}

	<div class="panel" style="border-radius:22px;padding:18px;margin-bottom:18px;">
		<div style="display:flex;align-items:center;justify-content:space-between;gap:12px;flex-wrap:wrap;margin-bottom:12px;">
			<div style="font-size:12.5px;color:var(--td-muted-2);">
				{data.mine.length.toLocaleString()} of {data.map.points.length.toLocaleString()} genres
				{#if pending > 0}
					· <span style="color:var(--td-muted-3);">{pending.toLocaleString()} listens still being enriched</span>
				{/if}
			</div>
			<button class="btn btn-secondary" on:click={() => (trueScale = !trueScale)}>
				{trueScale ? 'Fit to screen' : 'True proportions'}
			</button>
		</div>

		<div style="{trueScale ? 'max-height:74vh;overflow-y:auto;' : ''}border-radius:14px;background:var(--td-track);">
			<svg viewBox="0 0 {data.map.width} {viewH}" role="img"
				aria-label="Your listening plotted on the Every Noise genre map"
				style="width:100%;height:auto;display:block;">
				{#each backdrop as p}
					<circle cx={p[1]} cy={p[2] * yScale} r={unit * 1.1} fill={p[3]} opacity="0.2" />
				{/each}
				{#each data.mine as m}
					<circle cx={m.x} cy={m.y * yScale} r={radius(m.plays, maxPlays, unit)} fill={m.color}
						stroke="var(--td-bg)" stroke-width={unit * 0.45} opacity="0.95"
						on:mouseenter={() => (hovered = m)} on:mouseleave={() => (hovered = null)}
						on:focus={() => (hovered = m)} on:blur={() => (hovered = null)}
						role="button" aria-label="{m.name}, {m.plays} plays" tabindex="0"
						style="cursor:pointer;" />
				{/each}
			</svg>
		</div>
		{#if !trueScale}
			<p style="font-size:12px;color:var(--td-muted-3);margin:8px 0 0;">
				Vertically compressed about 20× so the whole map fits — the axis still runs organic to
				mechanical, in less room. “True proportions” shows it undistorted.
			</p>
		{/if}

		<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:10px;min-height:1.4em;">
			{#if hovered}
				<strong style="color:var(--td-text);">{hovered.name}</strong>
				{#if hovered.plays}· {hovered.plays.toLocaleString()} plays{/if}
			{:else}
				Hover a lit point for its genre.
			{/if}
		</div>
	</div>

	{#if top.length}
		<div class="panel" style="border-radius:22px;padding:22px;">
			<h3 class="h-display" style="font-size:19px;margin:0 0 12px;">Where you live</h3>
			<div style="display:flex;flex-wrap:wrap;gap:8px;">
				{#each top as g}
					<span style="display:inline-flex;align-items:center;gap:7px;padding:5px 11px;border-radius:999px;background:var(--td-track);font-size:12.5px;">
						<span style="width:10px;height:10px;border-radius:50%;background:{g.color};flex:none;"></span>
						{g.name}<span style="color:var(--td-muted-3);">{g.plays.toLocaleString()}</span>
					</span>
				{/each}
			</div>
			{#if data.unplaced_plays > 0}
				<p style="font-size:12px;color:var(--td-muted-3);margin:12px 0 0;">
					{data.unplaced_plays.toLocaleString()} plays sit on tags with no place on the map —
					things like “saxophone”, “french” or “film composer”, which describe an instrument,
					a nationality or a role rather than a genre.
				</p>
			{/if}
		</div>
	{/if}
{/if}
