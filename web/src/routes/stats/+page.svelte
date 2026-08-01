<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type RangedStats, type StatsRange, type Personality } from '$lib/api';
	import CoverArt from '$lib/components/CoverArt.svelte';
	import BarChart from '$lib/components/BarChart.svelte';
	import { openAlbum, openArtist } from '$lib/entities';

	const ranges: { v: StatsRange; label: string }[] = [
		{ v: 'week', label: 'Week' },
		{ v: 'month', label: 'Month' },
		{ v: 'last30', label: 'Last 30' },
		{ v: 'year', label: 'Year' },
		{ v: 'all', label: 'All' }
	];
	let range: StatsRange = 'all';
	let stats: RangedStats | null = null;
	let persona: Personality | null = null;
	let loading = true;
	let metric: 'plays' | 'hours' = 'plays';

	/** How many rows a top list shows before "See all". Six is what fits beside
	 *  its neighbours without the panels growing unequal. */
	const COLLAPSED = 6;
	/** Computed once rather than re-tested in every row: inside an `{#each}` the
	 *  checker narrows `metric` to its initialiser and flags the comparison. */
	$: unit = metric === 'hours' ? 'h' : '';
	let showAll = { artists: false, albums: false, tracks: false };

	const days = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];

	$: maxArtist = stats ? Math.max(1, ...stats.top_artists.map((a) => (metric === 'plays' ? a.plays : a.hours))) : 1;
	$: maxAlbum = stats ? Math.max(1, ...stats.top_albums.map((a) => (metric === 'plays' ? a.plays : a.hours))) : 1;
	$: maxTrack = stats ? Math.max(1, ...(stats.top_tracks ?? []).map((t) => (metric === 'plays' ? t.plays : t.hours))) : 1;

	/** There is no track page, so a track opens the record it's on — which is
	 *  where its play count sits in context. Without an album there's nothing to
	 *  open, so it falls back to the listens themselves. */
	function openTrack(t: { title: string; artist: string; album: string | null }) {
		if (t.album) return openAlbum(t.artist, t.album);
		window.location.href = `/history?track=${encodeURIComponent(t.title)}`;
	}
	$: maxHeat = stats ? Math.max(1, ...stats.heatmap.flat()) : 1;

	// Match the quality-badge colours so a tier reads the same everywhere.
	const fidelityFill = (tier: string) =>
		({
			DSD: 'linear-gradient(90deg,#c67139,#e07a55)',
			Lossless: 'linear-gradient(90deg,#7a8a5e,#aebf92)',
			Lossy: 'linear-gradient(90deg,#8a8172,#b3a487)'
		})[tier] ?? 'linear-gradient(90deg,#645c50,#9a8f7d)';

	async function load() {
		loading = true;
		try {
			[stats, persona] = await Promise.all([api.getRangedStats(range), api.getPersonality(range)]);
		} catch (e) { console.warn('stats', e); }
		loading = false;
	}

	function setRange(r: StatsRange) { range = r; load(); }

	onMount(load);
</script>

<svelte:head><title>Analytics — Tapedeck</title></svelte:head>

<div style="display:flex;align-items:flex-end;justify-content:space-between;gap:16px;flex-wrap:wrap;margin-bottom:20px;">
	<div>
		<h1 class="h-display" style="font-size:38px;">Analytics</h1>
		<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;">The shape of your listening.</p>
	</div>
	<!-- Plays/Hours drove all three top lists but sat inside the Top Artists
	     header; side by side there is no one panel it belongs to, so it moves up
	     next to the range it works with. -->
	<div style="display:flex;gap:10px;flex-wrap:wrap;">
		<div style="display:inline-flex;background:var(--td-sidebar);border-radius:999px;padding:4px;gap:2px;">
			{#each ranges as r}
				<button class="seg-btn" class:on={range === r.v} on:click={() => setRange(r.v)}>{r.label}</button>
			{/each}
		</div>
		<div style="display:inline-flex;background:var(--td-sidebar);border-radius:999px;padding:4px;gap:2px;">
			<button class="seg-btn" class:on={metric === 'plays'} on:click={() => (metric = 'plays')}>Plays</button>
			<button class="seg-btn" class:on={metric === 'hours'} on:click={() => (metric = 'hours')}>Hours</button>
		</div>
	</div>
</div>

{#if loading && !stats}
	<div style="display:flex;justify-content:center;padding:80px;"><div class="spin" style="width:22px;height:22px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
{:else if stats}
	<!-- Musical personality -->
	{#if persona && persona.traits.length}
		<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:24px;background:linear-gradient(135deg, var(--td-panel), var(--td-panel2));color:#f4e7cf;">
			<div style="font-size:11px;letter-spacing:.16em;text-transform:uppercase;color:#c9a678;margin-bottom:8px;">Your musical personality</div>
			<div class="h-display" style="font-size:32px;margin-bottom:20px;color:#f4e7cf;">{persona.verdict}</div>
			<div style="display:flex;flex-direction:column;gap:16px;">
				{#each persona.traits as tr}
					<div>
						<div style="display:flex;justify-content:space-between;font-size:12.5px;margin-bottom:6px;color:#e9dcc4;">
							<span style="opacity:{tr.score < 50 ? 1 : .5};font-weight:600;">{tr.low}</span>
							<span style="color:#c9a678;">{tr.evidence}</span>
							<span style="opacity:{tr.score > 50 ? 1 : .5};font-weight:600;">{tr.high}</span>
						</div>
						<div style="position:relative;height:8px;border-radius:999px;background:rgba(244,231,207,.14);">
							<div style="position:absolute;top:50%;left:50%;width:1px;height:14px;transform:translate(-50%,-50%);background:rgba(244,231,207,.3);"></div>
							<div style="position:absolute;top:-3px;width:14px;height:14px;border-radius:50%;background:var(--td-accent-hi);box-shadow:0 0 0 3px rgba(224,122,85,.25);left:calc({tr.score}% - 7px);"></div>
						</div>
					</div>
				{/each}
			</div>
			{#if persona.loyalty.length}
				<div style="margin-top:22px;padding-top:16px;border-top:1px solid rgba(244,231,207,.14);">
					<div style="font-size:11px;letter-spacing:.14em;text-transform:uppercase;color:#c9a678;margin-bottom:10px;">Most loyal to</div>
					<div style="display:flex;gap:8px;flex-wrap:wrap;">
						{#each persona.loyalty.slice(0, 6) as l}
							<button class="tag loyal-tag" on:click={() => openArtist(l.artist)} title={`Open ${l.artist}`}>{l.artist} · {Math.round(l.loyalty_score)}</button>
						{/each}
					</div>
				</div>
			{/if}
		</div>
	{/if}

	{@const t = stats.totals}
	<div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(140px,1fr));gap:14px;margin-bottom:14px;">
		{#each [['Total Plays', t.plays], ['Artists', t.artists], ['Albums', t.albums], ['Tracks', t.tracks]] as [label, val]}
			<div class="panel" style="border-radius:20px;padding:18px 20px;">
				<div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--td-muted-2);margin-bottom:6px;">{label}</div>
				<div class="h-display" style="font-size:30px;line-height:1;">{val}</div>
			</div>
		{/each}
	</div>
	<div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(140px,1fr));gap:14px;margin-bottom:28px;">
		<div class="panel" style="border-radius:20px;padding:18px 20px;"><div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--td-muted-2);margin-bottom:6px;">Hours</div><div class="h-display" style="font-size:30px;line-height:1;">{t.hours}</div></div>
		<div class="panel" style="border-radius:20px;padding:18px 20px;"><div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--td-muted-2);margin-bottom:6px;">Avg Quality</div><div class="h-display" style="font-size:30px;line-height:1;">{t.avg_quality}<span style="font-size:16px;color:var(--td-muted-2);">/100</span></div></div>
		<div class="panel" style="border-radius:20px;padding:18px 20px;"><div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--td-muted-2);margin-bottom:6px;">Lossless</div><div class="h-display" style="font-size:30px;line-height:1;color:var(--td-good-fg);">{t.lossless_pct}%</div></div>
		<div class="panel" style="border-radius:20px;padding:18px 20px;"><div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--td-muted-2);margin-bottom:6px;">DSD</div><div class="h-display" style="font-size:30px;line-height:1;color:#b2622d;">{t.dsd_pct}%</div></div>
	</div>

	<!-- Top artists / albums / tracks.
	     Side by side, as the redesign has them: three stacked full-width panels
	     of six bars each was a lot of vertical space for very little. `auto-fit`
	     drops them to two columns and then one as the viewport narrows. -->
	<div class="tops">
		<div class="panel top-panel">
			<div class="top-head">
				<h3 class="h-display" style="font-size:19px;margin:0;">Top Artists</h3>
				{#if stats.top_artists.length > COLLAPSED}
					<button class="see-all" on:click={() => (showAll.artists = !showAll.artists)}>
						{showAll.artists ? 'Show less' : `See all ${stats.top_artists.length} →`}
					</button>
				{/if}
			</div>
			{#if stats.top_artists.length}
				<div class="top-list" class:expanded={showAll.artists}>
					{#each showAll.artists ? stats.top_artists : stats.top_artists.slice(0, COLLAPSED) as a, i}
						{@const v = metric === 'plays' ? a.plays : a.hours}
						<button class="rank-row" on:click={() => openArtist(a.name)} title={`Open ${a.name}`}>
							<span class="rank-n">{i + 1}</span>
							<CoverArt url={a.image_url} seed={a.name} size={30} radius={999} />
							<div style="flex:1;min-width:0;">
								<div class="rank-line"><span class="rank-name">{a.name}</span><span class="rank-val">{v}{unit}</span></div>
								<div class="rank-track"><div style="width:{(v / maxArtist * 100).toFixed(0)}%;height:100%;background:linear-gradient(90deg,#c67139,#e07a55);border-radius:999px;"></div></div>
							</div>
						</button>
					{/each}
				</div>
			{:else}
				<p style="color:var(--td-muted);font-size:14px;margin:0;">No plays in this range yet.</p>
			{/if}
		</div>

		<div class="panel top-panel">
			<div class="top-head">
				<h3 class="h-display" style="font-size:19px;margin:0;">Top Albums</h3>
				{#if stats.top_albums.length > COLLAPSED}
					<button class="see-all" on:click={() => (showAll.albums = !showAll.albums)}>
						{showAll.albums ? 'Show less' : `See all ${stats.top_albums.length} →`}
					</button>
				{/if}
			</div>
			{#if stats.top_albums.length}
				<div class="top-list" class:expanded={showAll.albums}>
					{#each showAll.albums ? stats.top_albums : stats.top_albums.slice(0, COLLAPSED) as al, i}
						{@const v = metric === 'plays' ? al.plays : al.hours}
						<button class="rank-row block" on:click={() => openAlbum(al.artist, al.name)} title={`Open ${al.name}`}>
							<div class="rank-line">
								<span class="rank-label"><span class="rank-n inline">{i + 1}</span><span class="rank-name">{al.name}</span><span class="rank-sub"> — {al.artist}</span></span>
								<span class="rank-val">{v}{unit}</span>
							</div>
							<div class="rank-track"><div style="width:{(v / maxAlbum * 100).toFixed(0)}%;height:100%;background:linear-gradient(90deg,#7a8a5e,#aebf92);border-radius:999px;"></div></div>
						</button>
					{/each}
				</div>
			{:else}
				<p style="color:var(--td-muted);font-size:14px;margin:0;">No album data in this range yet.</p>
			{/if}
		</div>

		<div class="panel top-panel">
			<div class="top-head">
				<h3 class="h-display" style="font-size:19px;margin:0;">Top Tracks</h3>
				{#if (stats.top_tracks?.length ?? 0) > COLLAPSED}
					<button class="see-all" on:click={() => (showAll.tracks = !showAll.tracks)}>
						{showAll.tracks ? 'Show less' : `See all ${stats.top_tracks.length} →`}
					</button>
				{/if}
			</div>
			{#if stats.top_tracks?.length}
				<div class="top-list" class:expanded={showAll.tracks}>
					{#each showAll.tracks ? stats.top_tracks : stats.top_tracks.slice(0, COLLAPSED) as t, i}
						{@const v = metric === 'plays' ? t.plays : t.hours}
						<button class="rank-row block" on:click={() => openTrack(t)} title={t.album ? `Open ${t.album}` : `Listens of ${t.title}`}>
							<div class="rank-line">
								<span class="rank-label"><span class="rank-n inline">{i + 1}</span><span class="rank-name">{t.title}</span><span class="rank-sub"> — {t.artist}</span></span>
								<span class="rank-val">{v}{unit}</span>
							</div>
							<div class="rank-track"><div style="width:{(v / maxTrack * 100).toFixed(0)}%;height:100%;background:linear-gradient(90deg,#b2622d,#e0a06a);border-radius:999px;"></div></div>
						</button>
					{/each}
				</div>
			{:else}
				<p style="color:var(--td-muted);font-size:14px;margin:0;">No plays in this range yet.</p>
			{/if}
		</div>
	</div>

	<!-- Fidelity mix -->
	{#if stats.fidelity.length}
		<h3 class="h-display" style="font-size:19px;margin:0 0 12px;">Fidelity Mix</h3>
		<div class="panel" style="border-radius:22px;padding:22px;margin-bottom:16px;display:flex;flex-direction:column;gap:14px;">
			{#each stats.fidelity as f}
				<div>
					<div style="display:flex;justify-content:space-between;font-size:13px;margin-bottom:6px;">
						<span style="font-weight:600;">{f.name}</span>
						<span style="color:var(--td-muted-2);">{f.pct}% · {f.plays}</span>
					</div>
					<div style="height:10px;border-radius:999px;background:var(--td-track);overflow:hidden;"><div style="width:{f.pct}%;height:100%;background:{fidelityFill(f.name)};border-radius:999px;"></div></div>
				</div>
			{/each}
		</div>
	{/if}

	<!-- Decades -->
	{#if stats.decades.length}
		<h3 class="h-display" style="font-size:19px;margin:0 0 12px;">By Decade</h3>
		<BarChart bars={stats.decades.map((d) => ({ label: d.name, value: d.plays }))} />
	{/if}

	<!-- Heatmap -->
	<h3 class="h-display" style="font-size:19px;margin:0 0 12px;">Listening Clock</h3>
	<div class="panel" style="border-radius:22px;padding:22px;overflow-x:auto;">
		<!-- Header and rows share one grid template. They used to be separate flex
		     rows, and because the cells were capped at 20px while the hour labels
		     were not, the two drifted apart as the panel got wider — on a desktop
		     the "18" ended up well past the last cell. -->
		<div class="heat">
			<div class="heat-row heat-head">
				<span></span>
				{#each Array(24) as _, h}<span class="heat-hour">{h % 6 === 0 ? h : ''}</span>{/each}
			</div>
			{#each stats.heatmap as row, di}
				<div class="heat-row">
					<span class="heat-day">{days[di]}</span>
					{#each row as c}
						<span title={`${c} plays`} class="heat-cell" style="background:rgba(198,113,57,{c === 0 ? 0.06 : 0.15 + 0.8 * (c / maxHeat)});"></span>
					{/each}
				</div>
			{/each}
			<div class="heat-legend">Less
				<span style="background:rgba(198,113,57,.12);"></span>
				<span style="background:rgba(198,113,57,.38);"></span>
				<span style="background:rgba(198,113,57,.64);"></span>
				<span style="background:rgba(198,113,57,.92);"></span>More</div>
		</div>
	</div>
	<!-- Languages -->
	{#if stats.languages.length}
		<h3 class="h-display" style="font-size:19px;margin:24px 0 12px;">Languages</h3>
		<div class="panel" style="border-radius:22px;padding:22px;">
			{#each stats.languages as l}
				<div style="margin-bottom:12px;">
					<div style="display:flex;justify-content:space-between;font-size:13px;margin-bottom:5px;"><span style="font-weight:600;">{l.name}</span><span style="color:var(--td-muted-2);">{l.pct}%</span></div>
					<div style="height:10px;border-radius:999px;background:var(--td-track);overflow:hidden;"><div style="width:{l.pct}%;height:100%;background:linear-gradient(90deg,#7a8a5e,#aebf92);border-radius:999px;"></div></div>
				</div>
			{/each}
		</div>
	{:else}
		<p style="color:var(--td-muted-3);font-size:12.5px;margin:16px 0 0;">Language breakdown fills in as listens are classified (Settings → run enrichment).</p>
	{/if}
{/if}

<style>
	/* One template for the labels and the cells, so they can't drift apart.
	   The columns are `1fr`, not a fixed cap: capping them left most of the
	   panel empty on a desktop while the hour labels still spanned its full
	   width. `aspect-ratio` on the cell keeps them square as they grow, and
	   `min-width` on the block keeps it scrollable rather than crushed on a
	   phone. */
	.heat { min-width: 560px; }
	.heat-row {
		display: grid;
		grid-template-columns: 38px repeat(24, minmax(0, 1fr));
		gap: 4px;
		align-items: center;
		margin-bottom: 4px;
	}
	.heat-head { margin-bottom: 6px; }
	.heat-hour {
		font-size: 9px;
		color: var(--td-muted-3);
		text-align: center;
	}
	.heat-day {
		font-size: 10.5px;
		color: var(--td-muted-3);
		text-align: right;
		padding-right: 6px;
	}
	.heat-cell { aspect-ratio: 1; border-radius: 3px; }
	.heat-legend {
		display: flex;
		align-items: center;
		gap: 6px;
		margin-top: 14px;
		margin-left: 41px;
		font-size: 10.5px;
		color: var(--td-muted-3);
	}
	.heat-legend span { width: 13px; height: 13px; border-radius: 3px; }

	.tops {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
		gap: 16px;
		margin-bottom: 16px;
	}
	.top-panel {
		border-radius: 22px;
		padding: 22px;
		/* Equal height regardless of how many rows each list has. */
		display: flex;
		flex-direction: column;
	}
	.top-head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 10px;
		margin-bottom: 16px;
	}
	.see-all {
		border: none;
		background: transparent;
		padding: 0;
		cursor: pointer;
		font: inherit;
		font-size: 12.5px;
		color: #c67139;
		white-space: nowrap;
	}
	.see-all:hover { color: #e07a55; text-decoration: underline; }
	/* Expanded, the list scrolls inside the panel rather than pushing the page
	   down by forty rows. */
	.top-list.expanded {
		max-height: 420px;
		overflow-y: auto;
		margin-right: -6px;
		padding-right: 6px;
	}

	/* The ranked rows are the most obvious place to want an entity page from,
	   and until now they were the only lists in the app you couldn't click.
	   Buttons, not anchors: the id is resolved on click. */
	.rank-row {
		display: flex;
		align-items: center;
		gap: 10px;
		width: 100%;
		margin-bottom: 13px;
		padding: 4px 6px;
		border: none;
		border-radius: 12px;
		background: transparent;
		color: inherit;
		font: inherit;
		text-align: left;
		cursor: pointer;
	}
	.rank-row.block { display: block; }
	.rank-row:hover { background: var(--td-nav-hover); }
	.rank-n {
		flex: none;
		width: 16px;
		font-size: 11.5px;
		font-variant-numeric: tabular-nums;
		color: var(--td-muted-3);
		text-align: right;
	}
	.rank-n.inline { display: inline-block; margin-right: 7px; }
	.rank-line {
		display: flex;
		justify-content: space-between;
		gap: 10px;
		font-size: 13px;
		margin-bottom: 5px;
	}
	.rank-label {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.rank-sub { color: var(--td-muted-3); }
	.rank-val { color: var(--td-muted-2); flex: none; }
	.rank-track {
		height: 10px;
		border-radius: 999px;
		background: var(--td-track);
		overflow: hidden;
	}
	.rank-name { font-weight: 600; }
	/* The loyalty chips sit on the dark personality panel, so they carry their
	   own colours rather than the page's tag defaults. */
	.loyal-tag {
		background: rgba(244, 231, 207, .1);
		color: #e9dcc4;
		border: none;
		cursor: pointer;
		font: inherit;
		font-size: 12.5px;
	}
	.loyal-tag:hover { background: rgba(244, 231, 207, .2); }
	.rank-row:hover .rank-name { color: var(--td-accent); }

	.spin { animation: td-spin 0.7s linear infinite; }
	.seg-btn {
		border: none; background: transparent; cursor: pointer; color: var(--td-nav-fg);
		font-size: 12.5px; padding: 6px 14px; border-radius: 999px; font-family: inherit;
	}
	.seg-btn.on { background: var(--td-accent); color: #fbf5ea; }
</style>
