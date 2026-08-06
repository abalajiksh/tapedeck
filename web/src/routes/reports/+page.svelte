<script lang="ts">
	/**
	 * Reports — a feed of period cards, newest first.
	 *
	 * Its own surface rather than a tab under Statistics (Ashwin's call). The
	 * cost is named in the roadmap: this is a second analytics home whose period
	 * control can drift out of step with Statistics and Top Charts. Organising
	 * the four of them is deliberately later work.
	 *
	 * House rules held from the design package: no play buttons (Tapedeck has no
	 * playback), the genre split is labelled a share of tag mentions, and
	 * nothing here points at music you don't own.
	 */
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { Reports, ReportPeriod } from '$lib/api';
	import { openArtist, openAlbum } from '$lib/entities';
	import PeriodBars from '$lib/components/charts/PeriodBars.svelte';
	import ListeningClock from '$lib/components/charts/ListeningClock.svelte';
	import ToTop from '$lib/components/ToTop.svelte';

	const UNITS: { k: 'week' | 'month' | 'year'; label: string }[] = [
		{ k: 'week', label: 'Week' },
		{ k: 'month', label: 'Month' },
		{ k: 'year', label: 'Year' }
	];

	let unit: 'week' | 'month' | 'year' = 'month';
	let data: Reports | null = null;
	let loading = true;
	let error = '';

	async function load() {
		loading = true;
		try {
			data = await api.getReports(unit, unit === 'year' ? 8 : 12);
			error = '';
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		loading = false;
		history.replaceState(null, '', `/reports?unit=${unit}`);
	}

	function setUnit(u: 'week' | 'month' | 'year') {
		unit = u;
		load();
	}

	/** A signed change against the previous period, or null when there isn't one. */
	function delta(now: number, before: number | undefined | null) {
		if (before == null || before === 0) return null;
		return Math.round(((now - before) / before) * 100);
	}

	const nf = (n: number) => n.toLocaleString();

	/** The three headline tiles, built as typed rows — an inline array of mixed
	 *  tuples widens to `string | number` and loses every comparison. */
	function headline(p: ReportPeriod) {
		return [
			{ label: 'Scrobbles', value: nf(p.plays), d: delta(p.plays, p.prev?.plays), was: p.prev ? nf(p.prev.plays) : null },
			{ label: 'Avg. daily', value: nf(p.avg_daily), d: delta(p.avg_daily, p.prev?.avg_daily), was: p.prev ? nf(p.prev.avg_daily) : null },
			{ label: 'Listening time', value: `${p.hours}h`, d: delta(p.hours, p.prev?.hours), was: p.prev ? `${p.prev.hours}h` : null }
		];
	}

	function counts(p: ReportPeriod) {
		return [
			{ label: 'artists', n: p.artists, before: p.prev?.artists },
			{ label: 'albums', n: p.albums, before: p.prev?.albums },
			{ label: 'tracks', n: p.tracks, before: p.prev?.tracks }
		];
	}

	/** The busiest hour, named. The clock shows the shape; this says the point. */
	function peakHour(clock: number[]) {
		if (!clock?.length) return null;
		const h = clock.indexOf(Math.max(...clock));
		return `${String(h).padStart(2, '0')}:00`;
	}

	/**
	 * One line of editorial, built only from what the data actually says.
	 *
	 * Deliberately conservative: it names the top artist and, when there is one,
	 * the discovery. No adjectives the numbers don't support — a generated
	 * sentence claiming a "loud, loyal month" from a play count is the kind of
	 * thing that reads well once and wrong every time after.
	 */
	function blurb(p: ReportPeriod): string {
		const parts: string[] = [];
		if (p.top_artist) parts.push(`Mostly ${p.top_artist.name}`);
		if (p.crush) parts.push(`and you found ${p.crush.name}`);
		const d = delta(p.plays, p.prev?.plays);
		if (d != null && Math.abs(d) >= 15) {
			parts.push(`— ${d > 0 ? 'up' : 'down'} ${Math.abs(d)}% on ${p.prev?.label}`);
		}
		return parts.join(' ');
	}

	onMount(() => {
		const q = new URLSearchParams(window.location.search).get('unit');
		if (q === 'week' || q === 'month' || q === 'year') unit = q;
		load();
	});
</script>

<svelte:head><title>Reports — Tapedeck</title></svelte:head>

<div style="display:flex;align-items:flex-end;justify-content:space-between;gap:16px;flex-wrap:wrap;margin-bottom:18px;">
	<div>
		<div class="eyebrow">Reports</div>
		<h1 class="h-display" style="font-size:38px;margin:0;">Your listening, period by period</h1>
		<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;">
			Newest first. Everything below is bucketed in your own timezone{data?.timezone
				? ` (${data.timezone.replace(/_/g, ' ')})`
				: ''}.
		</p>
	</div>
	<div style="display:inline-flex;background:var(--td-sidebar);border-radius:999px;padding:4px;gap:2px;">
		{#each UNITS as u}
			<button class="seg-btn" class:on={unit === u.k} on:click={() => setUnit(u.k)}>{u.label}</button>
		{/each}
	</div>
</div>

{#if loading && !data}
	<div style="display:flex;justify-content:center;padding:80px;">
		<div class="spin" style="width:22px;height:22px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div>
	</div>
{:else if error}
	<div class="panel" style="border-radius:22px;padding:24px;">
		<p style="font-size:13.5px;color:var(--td-muted);margin:0;">{error}</p>
	</div>
{:else if !data?.periods.length}
	<div class="panel" style="border-radius:22px;padding:44px;text-align:center;color:var(--td-muted);">
		Nothing to report yet — press play on something.
	</div>
{:else}
	{#if data.capped}
		<p style="font-size:12px;color:var(--td-muted-3);margin:0 0 14px;">
			Your history is longer than the analytics cap, so the oldest periods here are partial.
		</p>
	{/if}

	<div class="feed">
		{#each data.periods as p (p.key)}
			{@const dPlays = delta(p.plays, p.prev?.plays)}
			<article class="panel card">
				<!-- Header -->
				<header class="card-head">
					<div style="min-width:0;">
						<h2 class="h-display" style="font-size:28px;margin:0;">{p.label}</h2>
						{#if blurb(p)}
							<p class="blurb">{blurb(p)}</p>
						{/if}
					</div>
					{#if p.colour}
						<!-- The period's own colour, the same linear blend the genre map
						     uses so the two agree. -->
						<div class="swatch" style="background:{p.colour};" title="This period sounded like {p.colour}"></div>
					{/if}
				</header>

				{#if p.milestones.length}
					<div class="milestones">
						{#each p.milestones as m}<span class="milestone">{m}</span>{/each}
					</div>
				{/if}

				<!-- Headline numbers -->
				<div class="tiles">
					{#each headline(p) as t}
						{@const label = t.label}
						{@const value = t.value}
						{@const d = t.d}
						{@const was = t.was}
						<div class="tile">
							<div class="tile-k">{label}</div>
							<div class="tile-v h-display">{value}</div>
							{#if d != null}
								<div class="tile-d" class:up={d > 0} class:down={d < 0}>
									{d > 0 ? '▲' : d < 0 ? '▼' : '—'} {Math.abs(d)}%
									<span class="was">was {was}</span>
								</div>
							{/if}
						</div>
					{/each}
				</div>

				<!-- Shape of the period -->
				<section class="block">
					<h3 class="block-h">
						{unit === 'week' ? 'Scrobbles by day' : unit === 'month' ? 'Scrobbles by week' : 'Scrobbles by month'}
					</h3>
					<PeriodBars
						values={p.sub}
						labels={p.sub_labels}
						previous={p.prev?.sub ?? null}
						currentLabel={p.label}
						previousLabel={p.prev?.label ?? 'Previous'}
					/>
				</section>

				<!-- How you listened -->
				<section class="block">
					<h3 class="block-h">How you listened</h3>
					<div class="how">
						{#if p.top_artist}
							<button class="feature" on:click={() => openArtist(p.top_artist?.name ?? '')}>
								<span class="feature-k">Top artist</span>
								<span class="feature-v">{p.top_artist.name}</span>
								<span class="feature-s">{nf(p.top_artist.plays)} plays · {p.top_artist.hours}h</span>
							</button>
						{/if}
						{#if p.top_album}
							<button class="feature" on:click={() => openAlbum(p.top_album?.artist ?? '', p.top_album?.name ?? '')}>
								<span class="feature-k">Record of the {unit}</span>
								<span class="feature-v">{p.top_album.name}</span>
								<span class="feature-s">{p.top_album.artist} · {nf(p.top_album.plays)} plays</span>
							</button>
						{/if}
						{#if p.top_track}
							<div class="feature static">
								<span class="feature-k">Track of the {unit}</span>
								<span class="feature-v">{p.top_track.title}</span>
								<span class="feature-s">{p.top_track.artist} · {nf(p.top_track.plays)} plays</span>
							</div>
						{/if}
					</div>
					<div class="counts">
						{#each counts(p) as c}
							{@const label = c.label}
							{@const n = c.n}
							{@const d = delta(c.n, c.before)}
							<div class="count">
								<div class="count-n h-display">{nf(n)}</div>
								<div class="count-k">{label}</div>
								{#if d != null}
									<div class="tile-d" class:up={d > 0} class:down={d < 0}>
										{d > 0 ? '▲' : d < 0 ? '▼' : '—'} {Math.abs(d)}%
									</div>
								{/if}
							</div>
						{/each}
					</div>
				</section>

				<div class="two-up">
					<!-- Clock -->
					<section class="block">
						<h3 class="block-h">Listening clock</h3>
						{#if peakHour(p.clock)}
							<p class="note">Busiest at {peakHour(p.clock)}.</p>
						{/if}
						<ListeningClock hours={p.clock} size={200} />
					</section>

					<!-- Genres -->
					<section class="block">
						<h3 class="block-h">Top genres</h3>
						{#if p.genres.length}
							{#each p.genres as g}
								<div style="margin-bottom:10px;">
									<div class="bar-row"><span>{g.name}</span><span class="muted">{g.pct}%</span></div>
									<div class="bar"><div class="fill" style="width:{g.pct}%"></div></div>
								</div>
							{/each}
							<p class="note">
								A track usually carries several tags, so these are shares of tag mentions —
								they total 100% across tags, not across plays.
							</p>
						{:else}
							<p class="note">Genres fill in as listens are enriched from MusicBrainz.</p>
						{/if}
					</section>
				</div>

				<!-- On repeat + crush -->
				<div class="two-up">
					{#if p.on_repeat.length}
						<section class="block">
							<h3 class="block-h">On repeat</h3>
							<p class="note">The heaviest single day each track had.</p>
							{#each p.on_repeat as r}
								<div class="repeat">
									<span class="repeat-t">{r.title}</span>
									<span class="repeat-a">{r.artist}</span>
									<span class="repeat-n">{r.plays}<small> in one day</small></span>
								</div>
							{/each}
						</section>
					{/if}

					{#if p.crush}
						<section class="block">
							<h3 class="block-h">Discovery of the {unit}</h3>
							<div class="crush">
								<div>
									<div class="crush-name h-display">{p.crush.name}</div>
									<p class="note" style="margin-top:4px;">
										{nf(p.crush.plays)} plays since first hearing them on
										{new Date(p.crush.since * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short' })}
										— your most-played new artist this {unit}.
									</p>
								</div>
								<div class="crush-n h-display">{nf(p.crush.plays)}</div>
							</div>
						</section>
					{:else if p.new_artists > 0}
						<section class="block">
							<h3 class="block-h">New to you</h3>
							<div class="crush">
								<p class="note" style="margin:0;">
									{nf(p.new_artists)} artist{p.new_artists === 1 ? '' : 's'} you had never played before.
								</p>
							</div>
						</section>
					{/if}
				</div>

				<!-- Off the shelf + chain -->
				{#if p.shelf.sides > 0 || p.top_chain}
					<div class="two-up">
						{#if p.shelf.sides > 0}
							<section class="block">
								<h3 class="block-h">Off the shelf</h3>
								<p class="note">{p.shelf.sides} side{p.shelf.sides === 1 ? '' : 's'} played by hand.</p>
								{#each p.shelf.rows as r}
									<div class="repeat">
										<span class="repeat-t">{r.title} · Side {r.side}</span>
										<span class="repeat-a">{r.artist}</span>
										<span class="repeat-n">{r.spins}<small> {r.spins === 1 ? 'spin' : 'spins'}</small></span>
									</div>
								{/each}
							</section>
						{/if}
						{#if p.top_chain}
							<section class="block">
								<h3 class="block-h">Mostly heard through</h3>
								<div class="chain">{p.top_chain}</div>
							</section>
						{/if}
					</div>
				{/if}
			</article>
		{/each}
	</div>

	<ToTop />
{/if}

<style>
	/* Same segmented control as Statistics and Top Charts. Svelte scopes styles
	   per component, so this is repeated rather than inherited — matching the
	   existing definition exactly matters more than deduplicating it. */
	.seg-btn {
		border: none;
		background: transparent;
		cursor: pointer;
		color: var(--td-nav-fg);
		font-size: 12.5px;
		padding: 6px 14px;
		border-radius: 999px;
		font-family: inherit;
	}
	.seg-btn.on {
		background: var(--td-accent);
		color: #fbf5ea;
	}
	.eyebrow {
		font-size: 11px;
		letter-spacing: 0.16em;
		text-transform: uppercase;
		color: var(--td-muted-2);
		margin-bottom: 6px;
	}

	.feed {
		display: flex;
		flex-direction: column;
		gap: 22px;
	}
	.card {
		border-radius: 24px;
		padding: 26px;
	}
	.card-head {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 16px;
		margin-bottom: 14px;
	}
	.blurb {
		margin: 6px 0 0;
		font-size: 14px;
		color: var(--td-muted);
	}
	.swatch {
		width: 44px;
		height: 44px;
		border-radius: 13px;
		flex: none;
		box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.12);
	}
	.milestones {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
		margin-bottom: 16px;
	}
	.milestone {
		padding: 5px 12px;
		border-radius: 999px;
		background: var(--td-good-bg);
		color: var(--td-good-fg);
		font-size: 12px;
		font-weight: 600;
	}

	.tiles {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
		gap: 12px;
		margin-bottom: 22px;
	}
	.tile {
		background: var(--td-track);
		border-radius: 16px;
		padding: 14px 16px;
	}
	.tile-k {
		font-size: 10.5px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--td-muted-2);
		margin-bottom: 5px;
	}
	.tile-v {
		font-size: 28px;
		line-height: 1;
	}
	.tile-d {
		font-size: 11.5px;
		margin-top: 6px;
		color: var(--td-muted-2);
	}
	.tile-d.up {
		color: var(--td-good-fg);
	}
	.tile-d.down {
		color: var(--td-accent);
	}
	.was {
		color: var(--td-muted-3);
		margin-left: 6px;
	}

	.block {
		margin-bottom: 22px;
		min-width: 0;
	}
	.block-h {
		font-family: 'Caprasimo', serif;
		font-weight: 400;
		font-size: 17px;
		margin: 0 0 8px;
	}
	.note {
		font-size: 12px;
		color: var(--td-muted-3);
		margin: 6px 0 10px;
		max-width: 62ch;
		line-height: 1.5;
	}
	.two-up {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
		gap: 22px;
	}

	.how {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(190px, 1fr));
		gap: 10px;
		margin-bottom: 14px;
	}
	.feature {
		display: flex;
		flex-direction: column;
		gap: 3px;
		text-align: left;
		background: var(--td-track);
		border: none;
		border-radius: 16px;
		padding: 13px 15px;
		font: inherit;
		color: inherit;
		cursor: pointer;
		min-width: 0;
	}
	.feature.static {
		cursor: default;
	}
	.feature-k {
		font-size: 10px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--td-muted-2);
	}
	.feature-v {
		font-size: 15px;
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.feature-s {
		font-size: 12px;
		color: var(--td-muted-2);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.counts {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(110px, 1fr));
		gap: 10px;
	}
	.count {
		padding: 10px 0;
	}
	.count-n {
		font-size: 24px;
		line-height: 1;
	}
	.count-k {
		font-size: 12px;
		color: var(--td-muted-2);
		margin-top: 3px;
	}

	.bar-row {
		display: flex;
		justify-content: space-between;
		gap: 10px;
		font-size: 13px;
		margin-bottom: 5px;
	}
	.bar-row span:first-child {
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.muted {
		color: var(--td-muted-2);
		flex: none;
	}
	.bar {
		height: 9px;
		border-radius: 999px;
		background: var(--td-track);
		overflow: hidden;
	}
	.fill {
		height: 100%;
		border-radius: 999px;
		background: linear-gradient(90deg, var(--td-accent-2), var(--td-accent));
	}

	.repeat {
		display: flex;
		align-items: baseline;
		gap: 8px;
		padding: 7px 0;
		border-bottom: 1px solid var(--td-border);
		font-size: 13px;
	}
	.repeat-t {
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.repeat-a {
		color: var(--td-muted-2);
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.repeat-n {
		flex: none;
		font-weight: 600;
	}
	.repeat-n small {
		font-weight: 400;
		color: var(--td-muted-3);
	}

	.crush {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 16px;
		background: var(--td-track);
		border-radius: 16px;
		padding: 16px 18px;
	}
	.crush-name {
		font-size: 21px;
	}
	.crush-n {
		font-size: 34px;
		line-height: 1;
		color: var(--td-accent);
		flex: none;
	}
	.chain {
		background: var(--td-track);
		border-radius: 16px;
		padding: 14px 16px;
		font-size: 14px;
		font-weight: 600;
	}

	@media (max-width: 640px) {
		.card {
			padding: 20px 16px;
		}
	}
</style>
