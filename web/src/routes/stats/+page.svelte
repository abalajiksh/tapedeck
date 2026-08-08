<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type RangedStats, type StatsRange, type Personality } from '$lib/api';
	import CoverArt from '$lib/components/CoverArt.svelte';
	import BarChart from '$lib/components/BarChart.svelte';
	import { openAlbum, openArtist } from '$lib/entities';
	import { fidelity, fidelityLabel } from '$lib/fidelity';

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

	const days = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
	/** First day of the week, 0=Sun..6=Sat. The backend builds the heatmap in a
	 *  fixed Sun..Sat order (it's a grid, not a list), so the rotation happens
	 *  here — the grid keeps its geometry and only the row order changes. */
	let weekStart = 1;
	$: heatOrder = Array.from({ length: 7 }, (_, i) => (weekStart + i) % 7);

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

	// ── The four "shape" cards ──
	//
	// Two of them follow the range control and two are facts about the whole
	// history. That mix is fine as long as each says which it is: a number that
	// silently ignores the control the user just moved reads as a bug.

	const RANGE_WORD: Record<StatsRange, string> = {
		week: 'last 7 days',
		month: 'last 30 days',
		last30: 'last 30 days',
		year: 'last year',
		all: 'whole history'
	};
	const rangeLabel = (r: StatsRange) => RANGE_WORD[r] ?? 'range';

	/** Share of *everything played*, so the two counts share a population —
	 *  `plays` already excludes skips, hence the sum in the denominator. */
	const skipShare = (skips: number, plays: number) =>
		plays + skips > 0 ? Math.round((skips / (plays + skips)) * 100) : 0;

	const sinceLabel = (ts: number | null) =>
		ts
			? new Date(ts * 1000).toLocaleDateString(undefined, { month: 'short', year: 'numeric' })
			: '—';

	/** How long that is, in the unit that reads naturally at this distance. */
	function sinceSub(ts: number | null): string {
		if (!ts) return 'nothing logged yet';
		const days = Math.max(0, (Date.now() / 1000 - ts) / 86400);
		if (days < 60) return `${Math.round(days)} days of listening, all time`;
		const years = days / 365.25;
		return years < 1
			? `${Math.round(days / 30.4)} months of listening, all time`
			: `${years.toFixed(1)} years of listening, all time`;
	}

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
			[stats, persona] = await Promise.all([
				api.getRangedStats(range, metric),
				api.getPersonality(range)
			]);
		} catch (e) { console.warn('stats', e); }
		loading = false;
	}

	function setRange(r: StatsRange) { range = r; load(); }
	/** Refetches rather than re-sorting locally: the server cuts each list to
	 *  50 rows, so the top six by hours can contain a record that is nowhere
	 *  near the top 50 by plays. */
	function setMetric(m: 'plays' | 'hours') { metric = m; load(); }

	/** Statistics and Top Charts have different range vocabularies — Charts has
	 *  a Today that Statistics doesn't, and Statistics splits Month from
	 *  Last 30. Handing the ranking off means translating, so the answer you
	 *  land on is the one you asked for here. */
	const RANGE_TO_CHART: Record<StatsRange, string> = {
		week: 'd7',
		month: 'month',
		last30: 'month',
		year: 'year',
		all: 'all'
	};
	/** "See more" hands off to Top Charts rather than expanding in place. Charts
	 *  is the ranking surface — 100 rows, a podium, artwork — so growing a
	 *  second, shorter copy of it inside a panel was the duplication, not the
	 *  fix. Metric travels too, so switching to Hours here keeps meaning there.
	 *
	 *  Reactive, not a plain `const`: Svelte tracks the dependencies of the
	 *  *expression* in the markup, which would just be the function identity.
	 *  A const closing over `range` and `metric` never changes identity, so the
	 *  hrefs would silently keep whatever they were built with on first render.
	 */
	$: chartsHref = (kind: 'artists' | 'albums' | 'tracks') =>
		`/charts?kind=${kind}&range=${RANGE_TO_CHART[range]}&metric=${metric}`;

	onMount(async () => {
		// Not awaited together: a missing preference must not stop the stats
		// themselves loading, and the default order is right for most people.
		api.getPreferences()
			.then((p) => {
				const w = (p as Record<string, unknown>)?.week_start;
				if (typeof w === 'number') weekStart = ((w % 7) + 7) % 7;
			})
			.catch(() => {});
		await load();
	});
</script>

<svelte:head><title>Statistics — Tapedeck</title></svelte:head>

<div style="display:flex;align-items:flex-end;justify-content:space-between;gap:16px;flex-wrap:wrap;margin-bottom:20px;">
	<div>
		<h1 class="h-display" style="font-size:38px;">Statistics</h1>
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
			<button class="seg-btn" class:on={metric === 'plays'} on:click={() => setMetric('plays')}>Plays</button>
			<button class="seg-btn" class:on={metric === 'hours'} on:click={() => setMetric('hours')}>Hours</button>
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
	{@const lossless = fidelity(t.lossless_pct, t.fidelity_known, t.plays, 'lossless')}
	{@const dsd = fidelity(t.dsd_pct, t.fidelity_known, t.plays, 'DSD')}
	<!-- Twelve slots, three rows of four.
	     `auto-fit` was fine at four cards and stops being fine at twelve: it
	     packs as many as will fit, so a wide viewport put eight in one line and
	     the rows stopped lining up. The count is fixed at four and steps down
	     with the viewport instead. -->
	<div class="statgrid">
		{#each [['Total Plays', t.plays], ['Artists', t.artists], ['Albums', t.albums], ['Tracks', t.tracks]] as [label, val]}
			<div class="panel stat">
				<div class="stat-k">{label}</div>
				<div class="h-display stat-v">{val}</div>
			</div>
		{/each}
	</div>
	<div class="statgrid">
		<div class="panel stat">
			<div class="stat-k">Hours</div>
			<div class="h-display stat-v">{t.hours}</div>
			<div class="fid-sub">
				{#if t.duration_known < t.plays}
					from the {t.duration_known.toLocaleString()} of {t.plays.toLocaleString()} listens with a known running time — so a floor, not a total
				{:else}
					every listen in this range reported its length
				{/if}
			</div>
		</div>
		<!-- The average is over listens that *have* a quality score, which only a
		     live source ever writes — so on an imported history it is drawn from
		     a sliver and used to say so nowhere at all, sitting next to a card
		     that confesses exactly the same coverage. It keeps its number,
		     because the number is true of the listens it covers, and now states
		     what those are.

		     With *nothing* scored it must not fall back to the number zero: on a
		     wholly imported history that rendered a confident "0/100", which
		     reads as a verdict on the listening rather than an absence of data
		     — precisely the misreading the fidelity cards were fixed for. It
		     shows the same em dash they do. -->
		<div class="panel stat">
			<div class="stat-k">Avg Quality</div>
			{#if t.quality_known}
				<div class="h-display stat-v">{t.avg_quality}<span style="font-size:16px;color:var(--td-muted-2);">/100</span></div>
				<div class="fid-sub">
					across {t.quality_known.toLocaleString()} of {t.plays.toLocaleString()} listens — the ones whose source reported a format
				</div>
			{:else}
				<div class="h-display stat-v" style="color:var(--td-muted-2);">—</div>
				<div class="fid-sub">
					nothing in this range reported a format, so there is nothing to score. The same
					sources that fill Fidelity fill this.
				</div>
			{/if}
		</div>
		<!-- Both shares are over listens with a known format, and both say so.
		     Dividing by every play made these a reading of how much of the
		     history was imported.

		     Below the coverage floor they collapse to *one* card, and that is
		     not a tidy-up. `fidelity()` decides to score on `known / total`,
		     which is a fact about the history rather than about lossless or DSD
		     — so both cards degraded to the same heading, the same percentage
		     and the same sentence, and the page showed "FIDELITY 4%" twice
		     side by side. There is one thing to say, so it is said once.

		     The survivor then takes both columns rather than leaving a gap. It
		     is the card with a paragraph in it, so it is the one that wants the
		     width, and the row stays four wide either way. -->
		{#if lossless.scored}
			<div class="panel stat">
				<div class="stat-k">{fidelityLabel(lossless, 'Lossless')}</div>
				<div class="h-display stat-v" style="color:var(--td-good-fg);">{lossless.value}</div>
				<div class="fid-sub">{lossless.sub}</div>
			</div>
			<div class="panel stat">
				<div class="stat-k">DSD</div>
				<div class="h-display stat-v" style="color:#b2622d;">{dsd.value}</div>
				<div class="fid-sub">{dsd.sub}</div>
			</div>
		{:else}
			<div class="panel stat wide">
				<div class="stat-k">Fidelity</div>
				<div class="h-display stat-v" style="color:var(--td-muted-2);">{lossless.value}</div>
				<div class="fid-sub">
					of your listening has a known format — too little to score lossless or DSD.
					Formats come from a media server or a player that reports them; an imported
					history carries none.
				</div>
			</div>
		{/if}
	</div>
	<!-- The shape of the listening rather than of its metadata: these four are
	     the only cards here that are always answerable, however a history
	     arrived. Two follow the range control and two are facts about the whole
	     history, so those two say so instead of looking like they moved. -->
	<div class="statgrid" style="margin-bottom:28px;">
		<div class="panel stat">
			<div class="stat-k">Per Day</div>
			<div class="h-display stat-v">{t.per_day}</div>
			<div class="fid-sub">plays per day across the {rangeLabel(range)}, counting the quiet ones</div>
		</div>
		<div class="panel stat">
			<div class="stat-k">Skipped</div>
			<div class="h-display stat-v">{t.skips.toLocaleString()}</div>
			<div class="fid-sub">
				{#if t.skips}
					{skipShare(t.skips, t.plays)}% of everything played — stored, never counted
				{:else}
					nothing skipped here; only some sources report it at all
				{/if}
			</div>
		</div>
		<div class="panel stat">
			<div class="stat-k">Listening Since</div>
			<div class="h-display stat-v" style="font-size:26px;">{sinceLabel(t.first_listen)}</div>
			<div class="fid-sub">{sinceSub(t.first_listen)}</div>
		</div>
		<a class="panel stat linkcard" href="/loved">
			<div class="stat-k">Loved</div>
			<div class="h-display stat-v">{t.loves.toLocaleString()}</div>
			<div class="fid-sub">tracks, records and artists, all time →</div>
		</a>
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
					<a class="see-all" href={chartsHref('artists')}>See more →</a>
				{/if}
			</div>
			{#if stats.top_artists.length}
				<div class="top-list">
					{#each stats.top_artists.slice(0, COLLAPSED) as a, i}
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
					<a class="see-all" href={chartsHref('albums')}>See more →</a>
				{/if}
			</div>
			{#if stats.top_albums.length}
				<div class="top-list">
					{#each stats.top_albums.slice(0, COLLAPSED) as al, i}
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
					<a class="see-all" href={chartsHref('tracks')}>See more →</a>
				{/if}
			</div>
			{#if stats.top_tracks?.length}
				<div class="top-list">
					{#each stats.top_tracks.slice(0, COLLAPSED) as t, i}
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
			{#each heatOrder as di}
				<div class="heat-row">
					<span class="heat-day">{days[di]}</span>
					{#each stats.heatmap[di] as c}
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
	<!-- Breadth -->
	{#if stats.breadth && stats.breadth.genres_played > 1}
		<h3 class="h-display" style="font-size:19px;margin:24px 0 12px;">Breadth</h3>
		<div class="panel" style="border-radius:22px;padding:22px;">
			<div style="display:flex;gap:28px;flex-wrap:wrap;">
				<div>
					<div style="font-size:30px;font-weight:600;line-height:1.1;">
						{stats.breadth.effective_genres.toLocaleString(undefined, { maximumFractionDigits: 0 })}
					</div>
					<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:2px;">
						effective genres, of {stats.breadth.genres_played.toLocaleString()} played
					</div>
				</div>
				{#if stats.breadth.reach != null}
					<div>
						<div style="font-size:30px;font-weight:600;line-height:1.1;">
							{Math.round(stats.breadth.reach * 100)}%
						</div>
						<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:2px;">
							of the genre map's own spread
						</div>
					</div>
				{/if}
			</div>
			<p style="font-size:12.5px;color:var(--td-muted);margin:14px 0 0;max-width:70ch;">
				Your listening is as varied as playing
				<strong>{stats.breadth.effective_genres.toLocaleString(undefined, { maximumFractionDigits: 0 })}</strong>
				genres in equal measure — fewer than the {stats.breadth.genres_played.toLocaleString()} you
				actually touched, because you return to some far more than others.
				{#if stats.breadth.reach != null}
					Reach compares how far apart your genres sit on the
					<a href="/map" style="color:var(--td-accent);">genre map</a> against how far apart the
					whole map is, so it says whether that variety is spread out or clustered in one
					corner. Based on {stats.breadth.genres_placed.toLocaleString()} genres with a position.
				{/if}
			</p>
		</div>
	{/if}

	<!-- Genres and Languages, side by side. Both are short lists of labelled
	     percentage bars — at full width each bar was a metre of empty panel with
	     a word at one end. `auto-fit` drops them back to one column under the
	     same breakpoint the top lists use. -->
	<div class="breakdowns">
		<div>
			<h3 class="h-display" style="font-size:19px;margin:24px 0 12px;">Genres</h3>
			{#if stats.genres.length}
				<div class="panel" style="border-radius:22px;padding:22px;">
					{#each stats.genres as g}
						<div style="margin-bottom:12px;">
							<div style="display:flex;justify-content:space-between;gap:10px;font-size:13px;margin-bottom:5px;"><span style="font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">{g.name}</span><span style="color:var(--td-muted-2);flex:none;">{g.pct}%</span></div>
							<div style="height:10px;border-radius:999px;background:var(--td-track);overflow:hidden;"><div style="width:{g.pct}%;height:100%;background:linear-gradient(90deg,var(--td-accent-2),var(--td-accent));border-radius:999px;"></div></div>
						</div>
					{/each}
					<p style="color:var(--td-muted-3);font-size:12px;margin:4px 0 0;">
						A track usually carries several tags, so these are shares of tag mentions
						rather than of listens — they add up to 100% across tags, not across plays.
					</p>
				</div>
			{:else}
				<p style="color:var(--td-muted-3);font-size:12.5px;margin:0;">Genres fill in as listens are enriched from MusicBrainz.</p>
			{/if}
		</div>

		<div>
			<h3 class="h-display" style="font-size:19px;margin:24px 0 12px;">Languages</h3>
			{#if stats.languages.length}
				<div class="panel" style="border-radius:22px;padding:22px;">
					{#each stats.languages as l}
						<div style="margin-bottom:12px;">
							<div style="display:flex;justify-content:space-between;gap:10px;font-size:13px;margin-bottom:5px;"><span style="font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">{l.name}</span><span style="color:var(--td-muted-2);flex:none;">{l.pct}%</span></div>
							<div style="height:10px;border-radius:999px;background:var(--td-track);overflow:hidden;"><div style="width:{l.pct}%;height:100%;background:linear-gradient(90deg,#7a8a5e,#aebf92);border-radius:999px;"></div></div>
						</div>
					{/each}
					<p style="color:var(--td-muted-3);font-size:12px;margin:4px 0 0;">
						One language per listen, so unlike genres these really are shares of
						plays. Non-Latin scripts are certain; a Latin-script title is looked up
						as a work in MusicBrainz and defaults to English when nothing says
						otherwise.
					</p>
				</div>
			{:else}
				<p style="color:var(--td-muted-3);font-size:12.5px;margin:0;">Language breakdown fills in as listens are classified (Settings → run enrichment).</p>
			{/if}
		</div>
	</div>
{/if}

<style>
	/* The line that says what the share is *of*. Small on purpose — it is a
	   qualifier, not a second number. */
	/* Four across, stepping down rather than repacking. `auto-fit` would put
	   eight cards on one line on a wide screen, so the three rows would stop
	   agreeing with each other. */
	.statgrid {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: 14px;
		margin-bottom: 14px;
	}
	.stat {
		border-radius: 20px;
		padding: 18px 20px;
		/* The rows have different amounts of sub-text, so the cards in a row
		   settle at a common height and the numbers stay on one baseline. */
		display: flex;
		flex-direction: column;
	}
	.stat-k {
		font-size: 10.5px; letter-spacing: .1em; text-transform: uppercase;
		color: var(--td-muted-2); margin-bottom: 6px;
	}
	.stat-v { font-size: 30px; line-height: 1; }
	/* The collapsed fidelity card takes the slot its twin left, keeping the row
	   four wide — and it is the one with a paragraph to fit. */
	.wide { grid-column: span 2; }
	.linkcard {
		text-decoration: none; color: inherit;
		transition: border-color .15s ease, transform .15s ease;
	}
	.linkcard:hover { border-color: var(--td-accent); transform: translateY(-1px); }
	@media (max-width: 900px) {
		.statgrid { grid-template-columns: repeat(2, minmax(0, 1fr)); }
	}
	@media (max-width: 460px) {
		.statgrid { grid-template-columns: minmax(0, 1fr); }
		.wide { grid-column: auto; }
	}
	.fid-sub {
		font-size: 11px; line-height: 1.35; color: var(--td-muted-3);
		margin-top: 6px;
	}
	.breakdowns {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
		gap: 18px;
	}
	/* The two panels hold different numbers of bars and footnotes of different
	   lengths, so left alone one ends higher than the other and the pair reads
	   as misaligned. `align-items: start` was what pinned them to their own
	   content heights; letting the column stretch and the panel fill it means
	   they match whatever ends up inside. */
	.breakdowns > div { display: flex; flex-direction: column; min-width: 0; }
	.breakdowns > div > :global(.panel) { flex: 1; }

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
	/* An anchor now, not a button: it hands off to Top Charts rather than
	   growing a second ranking inside this panel. */
	.see-all {
		font-size: 12.5px;
		font-weight: 600;
		color: var(--td-accent);
		white-space: nowrap;
		text-decoration: none;
	}
	.see-all:hover { color: var(--td-accent-hi); text-decoration: underline; }

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
