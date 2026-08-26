<script lang="ts">
	/** A month — the fullest chapter, and the one the design draws in most
	 *  detail. A month has enough listening to support an hour-of-day clock and
	 *  a genre split, which a week does not. */
	import type { ReportPeriod } from '$lib/api';
	import { openAlbum, openArtist } from '$lib/entities';
	import PeriodBars from '$lib/components/charts/PeriodBars.svelte';
	import ListeningClock from '$lib/components/charts/ListeningClock.svelte';
	import Fingerprint from '$lib/components/charts/Fingerprint.svelte';
	import ChapterHead from './ChapterHead.svelte';
	import TrendTrio from './TrendTrio.svelte';
	import ArtistHero from './ArtistHero.svelte';
	import FeatureCard from './FeatureCard.svelte';
	import CrushBand from './CrushBand.svelte';
	import { nf, delta, deltaText, barPct } from './bits';

	export let p: ReportPeriod;
	export let onShare: (() => void) | null = null;

	/** One line each, because a radar with unexplained axes is decoration. */
	const AXIS_COPY: Record<string, string> = {
		Variety: 'how many different artists, rather than a few on rotation',
		Repeat: 'how much of it was hearing something again',
		Discovery: 'how many of the artists you had never played before',
		Spread: 'how evenly it fell across the hours of the day',
		Runs: 'how much was three or more in a row from one record'
	};

	$: counts = [
		{ n: p.artists, unit: 'artists', before: p.prev?.artists },
		{ n: p.albums, unit: 'albums', before: p.prev?.albums },
		{ n: p.tracks, unit: 'tracks', before: p.prev?.tracks }
	];

	$: peak = (() => {
		if (!p.clock?.length) return null;
		const h = p.clock.indexOf(Math.max(...p.clock));
		return `${String(h).padStart(2, '0')}:00`;
	})();
</script>

<article class="panel card">
	<ChapterHead {p} unit="month" {onShare}>
		<p slot="blurb" class="blurb"><slot name="blurb" /></p>
		<svelte:fragment slot="chart">
			<div class="chart-k">Scrobbles by week</div>
			<PeriodBars
				values={p.sub}
				labels={p.sub_labels}
				previous={p.prev?.sub ?? null}
				currentLabel={p.label}
				previousLabel={p.prev?.label ?? 'Previous'}
			/>
		</svelte:fragment>
	</ChapterHead>

	{#if p.milestones.length}
		<div class="milestones">
			{#each p.milestones as m}<span class="milestone">{m}</span>{/each}
		</div>
	{/if}

	<TrendTrio
		rows={[
			{ label: 'Avg. daily', value: nf(p.avg_daily), now: p.avg_daily, before: p.prev?.avg_daily, was: p.prev ? nf(p.prev.avg_daily) : null },
			{ label: 'Listening time', value: `${p.hours}h`, now: p.hours, before: p.prev?.hours, was: p.prev ? `${p.prev.hours}h` : null },
			{ label: 'New artists', value: nf(p.new_artists), now: p.new_artists, before: null, was: null }
		]}
	/>

	{#if p.top_artist || p.top_album || p.top_track}
		<h3 class="sec h-display">How you listened</h3>
		<div class="feature-grid">
			{#if p.top_artist}
				<ArtistHero artist={p.top_artist} />
			{/if}
			<div class="stack">
				{#if p.top_album}
					<FeatureCard
						kind="album"
						label="Record of the month"
						title={p.top_album.name}
						sub={`${p.top_album.artist} · ${nf(p.top_album.plays)} plays`}
						onOpen={() => openAlbum(p.top_album?.artist ?? '', p.top_album?.name ?? '')}
					/>
				{/if}
				{#if p.top_track}
					<FeatureCard
						kind="track"
						label="Track of the month"
						title={p.top_track.title}
						sub={`${p.top_track.artist} · ${nf(p.top_track.plays)} plays`}
						onOpen={p.top_album
							? () => openAlbum(p.top_album?.artist ?? '', p.top_album?.name ?? '')
							: () => openArtist(p.top_track?.artist ?? '')}
					/>
				{/if}
			</div>
		</div>
	{/if}

	<div class="counts">
		{#each counts as c}
			{@const d = delta(c.n, c.before)}
			<div class="count">
				<div class="cn"><span class="h-display">{nf(c.n)}</span><span class="cu">{c.unit}</span></div>
				{#if d != null}
					<div class="cd" class:up={d > 0} class:down={d < 0}>
						{deltaText(d)} <span class="vs">vs {p.prev?.label}</span>
					</div>
				{/if}
			</div>
		{/each}
	</div>

	<div class="two-up">
		<div>
			<h3 class="sec sm h-display">Listening clock</h3>
			{#if peak}<p class="note">Busiest at {peak}.</p>{/if}
			<ListeningClock hours={p.clock} size={240} />
		</div>
		<div>
			<h3 class="sec sm h-display">Top genres</h3>
			{#if p.colour}
				<p class="note">This month's colour was <code>{p.colour}</code>.</p>
			{/if}
			{#if p.genres.length}
				{#each p.genres as g}
					<div class="g">
						<div class="grow"><span>{g.name}</span><span class="muted">{g.pct}%</span></div>
						<div class="bar"><div class="fill" style="width:{g.pct}%"></div></div>
					</div>
				{/each}
				<p class="note tight">
					A track usually carries several tags, so these are shares of tag mentions —
					they total 100% across tags, not across plays.
				</p>
			{:else}
				<p class="note">Genres fill in as listens are enriched from MusicBrainz.</p>
			{/if}
		</div>
	</div>

	{#if p.on_repeat.length}
		<h3 class="sec h-display">On repeat</h3>
		<p class="note" style="margin-top:-6px;">The heaviest single day each track had.</p>
		<div class="repeats">
			{#each p.on_repeat as r}
				<div class="repeat">
					<div class="ini h-display">{(r.artist || r.title).charAt(0).toUpperCase()}</div>
					<div style="flex:1;min-width:0;">
						<div class="rt">{r.title}</div>
						<div class="ra">{r.artist}</div>
					</div>
					<div class="rn"><div class="h-display">{r.plays}</div><div class="rl">in one day</div></div>
				</div>
			{/each}
		</div>
	{/if}

	{#if p.crush}
		<CrushBand crush={p.crush} unit="month" />
	{/if}

	{#if p.shelf.sides > 0 || p.top_chain}
		<h3 class="sec h-display">Off the shelf</h3>
		<div class="two-up">
			{#if p.shelf.sides > 0}
				<div class="tile">
					<div class="cn" style="margin-bottom:12px;">
						<span class="h-display" style="font-size:28px;">{p.shelf.sides}</span>
						<span class="cu">played by hand</span>
					</div>
					{#each p.shelf.rows as r}
						<div class="shelf-row">
							<span><strong>{r.title}</strong> <span class="muted">· {r.side}</span></span>
							<span class="muted">{r.spins} {r.spins === 1 ? 'spin' : 'spins'}</span>
						</div>
					{/each}
				</div>
			{/if}
			{#if p.top_chain}
				<div class="tile">
					<div class="k">Mostly heard through</div>
					<div class="chain">{p.top_chain}</div>
				</div>
			{/if}
		</div>
	{/if}

	<!-- The shape of the month, against the shape of your months. Beside the
	     clock because both answer "what was this period *like*" rather than
	     "what was in it" — and the comparison is to your own history, never to
	     other people. -->
	<h3 class="sec h-display">The shape of it</h3>
	<div class="two-up">
		<div class="tile">
			<Fingerprint
				axes={p.fingerprint.axes}
				you={p.fingerprint.you}
				usual={p.fingerprint.usual}
				usualPeriods={p.fingerprint.usual_periods}
			/>
		</div>
		<div class="tile">
			<div class="k" style="margin-bottom:10px;">What the axes mean</div>
			{#each p.fingerprint.axes as a, i}
				<div class="axrow">
					<span class="axn">{a}</span>
					<span class="axd">{AXIS_COPY[a] ?? ''}</span>
					<span class="axv">{Math.round(p.fingerprint.you[i] * 100)}</span>
				</div>
			{/each}
			<p class="caveat">
				Each is a share, out of 100, of what it could be — not a score. They come
				from times, artists, titles and albums, so every listen counts towards
				them whatever else it carries.
			</p>
		</div>
	</div>

	<!-- What you listened *through*. Every other card on this page is about
	     what played; this is the only one about how, which is the pillar the
	     rest of the application is built on. -->
	{#if p.gear.chains.length}
		<h3 class="sec h-display">Through what</h3>
		<div class="two-up">
			<div class="tile">
				<div class="k" style="margin-bottom:10px;">Chains</div>
				{#each p.gear.chains.slice(0, 6) as c}
					<div class="gearrow">
						<span class="gname">{c.name}</span>
						<span class="gbar"><span style="width:{barPct(c.hours, p.gear.chains[0].hours)}%"></span></span>
						<span class="gval">{c.hours >= 0.1 ? `${c.hours}h` : `${c.plays}×`}</span>
					</div>
				{/each}
			</div>
			{#if p.gear.items.length}
				<div class="tile">
					<div class="k" style="margin-bottom:10px;">Gear</div>
					{#each p.gear.items.slice(0, 6) as g}
						<div class="gearrow">
							<span class="gname">{g.name} <span class="muted">· {g.type}</span></span>
							<span class="gbar"><span style="width:{barPct(g.hours, p.gear.items[0].hours)}%"></span></span>
							<span class="gval">{g.hours >= 0.1 ? `${g.hours}h` : `${g.plays}×`}</span>
						</div>
					{/each}
				</div>
			{/if}
		</div>
		<!-- Said rather than implied. Most listens on most histories carry no
		     chain, and hours are a floor while any source reports no length —
		     a panel that hid both would read as a complete account. -->
		{#if p.gear.unattributed > 0 || p.gear.no_duration > 0}
			<p class="caveat">
				{#if p.gear.unattributed > 0}
					{p.gear.unattributed.toLocaleString()} of {p.plays.toLocaleString()} listens
					this month ran through nothing anybody recorded, so they are not here.
				{/if}
				{#if p.gear.no_duration > 0}
					{p.gear.no_duration.toLocaleString()} more reported no length, so these hours
					are a floor.
				{/if}
			</p>
		{/if}
	{/if}

	<!-- Who you heard it with. **Counted, never a share** — `company` is null
	     on everything nobody has tagged, so a percentage here would be about
	     how the listening was recorded rather than about the listening. -->
	{#if p.company.marked_plays > 0}
		<h3 class="sec h-display">In company</h3>
		<div class="tile">
			<div class="cn" style="margin-bottom:10px;">
				<span class="h-display" style="font-size:28px;">
					{p.company.marked_plays.toLocaleString()}
				</span>
				<span class="cu">
					of {p.company.of_plays.toLocaleString()} listens are marked as heard with
					somebody{p.company.marked_hours >= 0.1 ? ` · ${p.company.marked_hours}h` : ''}
				</span>
			</div>
			<div class="who">
				{#each p.company.people as w}
					<span class="wchip">{w.name} <span class="muted">{w.plays}</span></span>
				{/each}
			</div>
			<p class="caveat" style="margin-top:10px;">
				Only what has been marked — nothing here says the rest was spent alone.
			</p>
		</div>
	{/if}

	<slot name="footer" />
</article>

<style>
	.card { border-radius: 24px; padding: 26px; }
	.blurb { font-size: 14px; color: var(--td-muted); margin: 4px 0 0; max-width: 52ch; }
	.chart-k { font-size: 12px; color: var(--td-muted-2); margin-bottom: 8px; }
	.sec { font-size: 19px; margin: 0 0 12px; }
	.sec.sm { margin-bottom: 4px; }
	.note { font-size: 12.5px; color: var(--td-muted); margin: 0 0 10px; }
	.note.tight { color: var(--td-muted-3); font-size: 11.5px; line-height: 1.5; margin-top: 6px; }
	.milestones { display: flex; gap: 8px; flex-wrap: wrap; margin-bottom: 18px; }
	.milestone {
		background: var(--td-good-bg); color: var(--td-good-fg);
		font-size: 12px; font-weight: 600; border-radius: 999px; padding: 5px 12px;
	}
	.feature-grid { display: grid; grid-template-columns: 1.3fr 1fr; gap: 14px; margin-bottom: 24px; }
	.stack { display: flex; flex-direction: column; gap: 12px; }
	.counts { display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; margin-bottom: 26px; }
	.count { border: 1px solid var(--td-border); border-radius: 16px; padding: 15px 18px; min-width: 0; }
	.cn { display: flex; align-items: baseline; gap: 8px; flex-wrap: wrap; }
	.cn .h-display { font-size: 26px; }
	.cu { font-size: 12.5px; color: var(--td-muted-2); }
	.cd { font-size: 12px; font-weight: 600; margin-top: 3px; color: var(--td-muted-2); }
	.cd.up { color: var(--td-good-fg); }
	.cd.down { color: #b2622d; }
	.vs { color: var(--td-muted-3); font-weight: 400; }
	.two-up { display: grid; grid-template-columns: 1fr 1fr; gap: 14px; margin-bottom: 26px; }
	.g { margin-bottom: 12px; }
	.grow { display: flex; justify-content: space-between; font-size: 13px; margin-bottom: 5px; font-weight: 600; }
	.muted { color: var(--td-muted-2); font-weight: 400; }
	.bar { height: 9px; border-radius: 999px; background: var(--td-track); overflow: hidden; }
	.fill {
		height: 100%; border-radius: 999px;
		background: linear-gradient(90deg, var(--td-accent-2), var(--td-accent));
	}
	.repeats { border: 1px solid var(--td-border); border-radius: 18px; overflow: hidden; margin-bottom: 26px; }
	.repeat {
		display: flex; align-items: center; gap: 14px;
		padding: 13px 16px; border-bottom: 1px solid var(--td-border);
	}
	.repeat:last-child { border-bottom: none; }
	.ini {
		width: 44px; height: 44px; flex: none; border-radius: 11px; font-size: 18px;
		background: linear-gradient(135deg, #8a5a2f, #c67139); color: rgba(255, 255, 255, 0.85);
		display: flex; align-items: center; justify-content: center;
	}
	.rt { font-weight: 700; font-size: 14.5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.ra { font-size: 12.5px; color: var(--td-muted-2); }
	.rn { text-align: right; flex: none; }
	.rn .h-display { font-size: 22px; }
	.rl { font-size: 11px; color: var(--td-muted-3); }
	.tile { background: var(--td-track); border-radius: 18px; padding: 18px; min-width: 0; }
	.k {
		font-size: 11px; letter-spacing: 0.1em; text-transform: uppercase;
		color: var(--td-muted-2); margin-bottom: 8px;
	}
	.chain { font-weight: 700; font-size: 14.5px; line-height: 1.5; }
	.gearrow {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 64px auto;
		align-items: center;
		gap: 10px;
		font-size: 13px;
		margin-bottom: 7px;
	}
	.gname { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.gbar { height: 5px; border-radius: 999px; background: var(--td-track); overflow: hidden; }
	.gbar > span {
		display: block;
		height: 100%;
		border-radius: 999px;
		background: linear-gradient(90deg, var(--td-accent-2), var(--td-accent));
	}
	.gval { font-variant-numeric: tabular-nums; color: var(--td-muted-2); }
	.axrow {
		display: grid;
		grid-template-columns: 78px minmax(0, 1fr) 32px;
		gap: 8px;
		align-items: baseline;
		font-size: 12.5px;
		margin-bottom: 8px;
	}
	.axn { font-weight: 600; }
	.axd { color: var(--td-muted-2); line-height: 1.45; }
	.axv {
		text-align: right;
		font-variant-numeric: tabular-nums;
		color: var(--td-accent);
		font-weight: 700;
	}
	.who { display: flex; flex-wrap: wrap; gap: 7px; }
	.wchip {
		font-size: 12.5px;
		padding: 3px 10px;
		border-radius: 999px;
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
	}
	.caveat {
		font-size: 12px;
		color: var(--td-muted-3);
		margin: 10px 0 0;
		max-width: 66ch;
		line-height: 1.5;
	}
	.shelf-row { display: flex; justify-content: space-between; gap: 10px; font-size: 13px; margin-bottom: 7px; }
	@media (max-width: 760px) {
		.feature-grid, .two-up, .counts { grid-template-columns: 1fr; }
	}
</style>
