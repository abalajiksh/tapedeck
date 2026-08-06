<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { GenreMap, GenreTrajectory, GenreNeighbours } from '$lib/api';

	let data: GenreMap | null = null;
	let traj: GenreTrajectory | null = null;
	let nbrs: GenreNeighbours | null = null;
	let showPath = true;
	/** The lit points crowd the trajectory — they are the densest thing on the
	 *  plot and the path runs straight through the middle of them. Hiding them
	 *  leaves the faint backdrop, which is still the shape of the whole map, so
	 *  the path keeps its context without competing with it. */
	let showMine = true;
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
		// Optional: the map is useful on its own, so a trajectory failure must
		// not take the page down with it.
		try { traj = await api.getGenreTrajectory(); } catch { /* optional */ }
		try { nbrs = await api.getGenreNeighbours(); } catch { /* optional */ }
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

	// A year whose listens are only partly enriched is drawn faintly: the
	// backfill runs newest-first, so a trajectory is the view most distorted by
	// a half-done pass — it would look like the listening only started recently.
	const coverage = (y: { listens: number; enriched: number }) =>
		y.listens > 0 ? y.enriched / y.listens : 0;
	$: years = traj?.years ?? [];
	$: path = years.map((y) => `${y.x},${(y.y * yScale).toFixed(1)}`).join(' ');
	$: thinYears = years.filter((y) => coverage(y) < 0.9).length;

	// `genre_artists` is only filled by the tier-2 walk, so an instance that has
	// only fetched the one-request map has adjacency but no exemplars. Say so
	// rather than showing bare cards that look like a bug.
	$: anyExemplars = !!nbrs
		&& [...nbrs.unexplored, ...nbrs.opposite].some((g) => g.exemplar_artists?.length);

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
			<div style="display:flex;gap:8px;flex-wrap:wrap;">
				{#if years.length > 1}
					<button class="btn btn-secondary" on:click={() => (showPath = !showPath)}>
						{showPath ? 'Hide trajectory' : 'Show trajectory'}
					</button>
				{/if}
				<!-- Clearing `hovered` matters: the circle is removed rather than
				     moused out, so no `mouseleave` fires and the caption would keep
				     naming a genre that is no longer drawn. -->
				<button class="btn btn-secondary" on:click={() => { showMine = !showMine; hovered = null; }}>
					{showMine ? 'Hide my genres' : 'Show my genres'}
				</button>
				<button class="btn btn-secondary" on:click={() => (trueScale = !trueScale)}>
					{trueScale ? 'Fit to screen' : 'True proportions'}
				</button>
			</div>
		</div>

		<div style="{trueScale ? 'max-height:74vh;overflow-y:auto;' : ''}border-radius:14px;background:var(--td-track);">
			<svg viewBox="0 0 {data.map.width} {viewH}" role="img"
				aria-label="Your listening plotted on the Every Noise genre map"
				style="width:100%;height:auto;display:block;">
				{#each backdrop as p}
					<circle cx={p[1]} cy={p[2] * yScale} r={unit * 1.1} fill={p[3]} opacity="0.2" />
				{/each}
				{#each showMine ? data.mine : [] as m}
					<circle cx={m.x} cy={m.y * yScale} r={radius(m.plays, maxPlays, unit)} fill={m.color}
						stroke="var(--td-bg)" stroke-width={unit * 0.45} opacity="0.95"
						on:mouseenter={() => (hovered = m)} on:mouseleave={() => (hovered = null)}
						on:focus={() => (hovered = m)} on:blur={() => (hovered = null)}
						role="button" aria-label="{m.name}, {m.plays} plays" tabindex="0"
						style="cursor:pointer;" />
				{/each}
				{#if showPath && years.length > 1}
					<!-- Drawn over the genres but under nothing: the path is the
					     point of this view when it is on. -->
					<polyline points={path} fill="none" stroke="var(--td-text)" stroke-opacity="0.5"
						stroke-width={unit * 0.55} stroke-linejoin="round" stroke-linecap="round"
						stroke-dasharray="{unit * 2} {unit * 1.6}" />
					{#each years as y, i}
						<circle cx={y.x} cy={y.y * yScale} r={unit * 2.2}
							fill={y.colour ?? 'var(--td-accent)'} stroke="var(--td-text)"
							stroke-width={unit * 0.5} opacity={coverage(y) < 0.9 ? 0.45 : 1}
							on:mouseenter={() => (hovered = { name: `${y.year} — centre of ${y.plays.toLocaleString()} placed plays` })}
							on:mouseleave={() => (hovered = null)}
							role="button" aria-label="{y.year}, centre of {y.plays} plays" tabindex="0"
							style="cursor:pointer;" />
						{#if i === 0 || i === years.length - 1}
							<!-- `paint-order: stroke` puts a halo behind the glyphs so the
							     label survives landing on a dense patch of dots. -->
							<text x={y.x + unit * 3.2} y={y.y * yScale + unit * 1.6}
								font-size={unit * 5} fill="var(--td-text)"
								stroke="var(--td-track)" stroke-width={unit * 1.2}
								style="paint-order:stroke;">{y.year}</text>
						{/if}
					{/each}
				{/if}
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
			{:else if !showMine}
				Your genres are hidden — the faint backdrop is the whole map.
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

	{#if nbrs && (nbrs.next_door.length || nbrs.unexplored.length)}
		<div class="panel" style="border-radius:22px;padding:22px;margin-top:18px;">
			<h3 class="h-display" style="font-size:19px;margin:0 0 4px;">Next door</h3>
			<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;max-width:70ch;">
				Genres bordering the {nbrs.seeded_from} you play most, using Every Noise's own
				adjacency. Nothing here is a recommendation — the first list is music
				<strong>you already own</strong> and rarely reach for; the lists below it are the
				shape of the map next to you, named by Every Noise rather than by your library.
			</p>

			{#if nbrs.next_door.length}
				<div style="font-size:12.5px;color:var(--td-muted-2);margin-bottom:9px;">
					You have music here and barely play it
				</div>
				<div style="display:grid;grid-template-columns:repeat(auto-fill,minmax(240px,1fr));gap:10px;">
					{#each nbrs.next_door as g}
						<div style="padding:11px 13px;border-radius:14px;background:var(--td-track);">
							<div style="display:flex;align-items:center;gap:8px;margin-bottom:4px;">
								<span style="width:10px;height:10px;border-radius:50%;background:{g.color};flex:none;"></span>
								<strong style="font-size:13.5px;">{g.name}</strong>
								<span style="font-size:11.5px;color:var(--td-muted-3);margin-left:auto;">{g.your_plays.toLocaleString()} plays</span>
							</div>
							<div style="font-size:12px;color:var(--td-muted-2);">
								{g.your_artists.map((a) => a.artist).join(' \u00b7 ')}
							</div>
						</div>
					{/each}
				</div>
			{/if}

			{#if nbrs.unexplored.length}
				<div style="font-size:12.5px;color:var(--td-muted-2);margin:18px 0 3px;">
					Bordering regions you have nothing in
				</div>
				{#if anyExemplars}
					<!-- The provenance line is not decoration. These names are the
					     one place on the page that is *not* drawn from the user's
					     own history, and the card style alone can't say that. -->
					<div style="font-size:11.5px;color:var(--td-muted-3);margin:0 0 9px;">
						Artists here are Every Noise's own exemplars for the genre, not from your library.
					</div>
				{:else}
					<div style="height:6px;"></div>
				{/if}
				<div style="display:grid;grid-template-columns:repeat(auto-fill,minmax(240px,1fr));gap:10px;">
					{#each nbrs.unexplored as g}
						<div style="padding:11px 13px;border-radius:14px;border:1px dashed var(--td-border);background:transparent;">
							<div style="display:flex;align-items:center;gap:8px;">
								<span style="width:10px;height:10px;border-radius:50%;background:{g.color};flex:none;opacity:.8;"></span>
								<strong style="font-size:13.5px;color:var(--td-muted);">{g.name}</strong>
							</div>
							{#if g.exemplar_artists?.length}
								<div style="font-size:12px;color:var(--td-muted-3);margin-top:4px;font-style:italic;">
									{g.exemplar_artists.join(' · ')}
								</div>
							{/if}
						</div>
					{/each}
				</div>
			{/if}

			{#if nbrs.opposite.length}
				<div style="font-size:12.5px;color:var(--td-muted-2);margin:18px 0 9px;">
					Furthest from your listening
				</div>
				<div style="display:grid;grid-template-columns:repeat(auto-fill,minmax(240px,1fr));gap:10px;opacity:.85;">
					{#each nbrs.opposite.slice(0, 10) as g}
						<div style="padding:11px 13px;border-radius:14px;border:1px dashed var(--td-border);background:transparent;">
							<div style="display:flex;align-items:center;gap:8px;">
								<span style="width:10px;height:10px;border-radius:50%;background:{g.color};flex:none;opacity:.6;"></span>
								<strong style="font-size:13.5px;color:var(--td-muted-3);">{g.name}</strong>
							</div>
							{#if g.exemplar_artists?.length}
								<div style="font-size:12px;color:var(--td-muted-3);margin-top:4px;font-style:italic;">
									{g.exemplar_artists.join(' · ')}
								</div>
							{:else if g.your_artists?.length}
								<!-- Rare but real: a genre maximally far from your
								     listening that you nonetheless own something in.
								     Those are yours, so they are not italicised. -->
								<div style="font-size:12px;color:var(--td-muted-2);margin-top:4px;">
									{g.your_artists.map((a) => a.artist).join(' · ')}
								</div>
							{/if}
						</div>
					{/each}
				</div>
			{/if}

			{#if !anyExemplars && (nbrs.unexplored.length || nbrs.opposite.length)}
				<p style="font-size:11.5px;color:var(--td-muted-3);margin:12px 0 0;">
					Example artists for these regions come from Every Noise's per-genre pages, which
					only the full scrape collects. Run the deep scrape in Settings → Genre Map to fill
					them in.
				</p>
			{/if}
		</div>
	{/if}
{/if}
