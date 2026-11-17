<script lang="ts">
	/**
	 * One season — Christmas, Halloween or Easter — as a lens: everything in
	 * it is also still in Top Charts.
	 *
	 * The curve is Glenn McDonald's country chart drawn for one listener: the
	 * share of each day's listening that was the season's, from just before it
	 * starts. "Opened" is the day it lifted and stayed lifted.
	 */
	import { page } from '$app/stores';
	import { syncUrl } from '$lib/url';
	import { api, humanError, type InsightRow, type LensDetail } from '$lib/api';
	import { openAlbum, openArtist } from '$lib/entities';
	import Seg from '$lib/components/insights/Seg.svelte';
	import RankedRows from '$lib/components/insights/RankedRows.svelte';
	import ShareChapter from '$lib/components/reports/ShareChapter.svelte';
	import MixdownPlayer from '$lib/components/reports/MixdownPlayer.svelte';
	import ToTop from '$lib/components/ToTop.svelte';

	type Rank = 'songs' | 'artists' | 'albums';
	type Range = 'season' | 'year' | 'all';

	let data: LensDetail | null = null;
	let loading = true;
	let error = '';
	let season: number | null = null;
	let rank: Rank = 'songs';
	let range: Range = 'season';
	let metric: 'plays' | 'hours' = 'plays';
	let expanded: Record<string, InsightRow[] | 'loading'> = {};
	let sharing = false;
	let playing = false;
	let playlist: { id: number; created: boolean; tracks: number } | null = null;
	let playlistBusy = false;
	let notice = '';

	$: id = $page.params.id ?? '';
	$: label = data?.label ?? '';
	$: lowerLabel = label.toLowerCase();
	$: if (id) firstLoad(id);

	let loadedFor = '';
	function firstLoad(lens: string) {
		if (loadedFor === lens) return;
		loadedFor = lens;
		const q = new URLSearchParams(window.location.search);
		const s = Number(q.get('season'));
		season = Number.isFinite(s) && s > 0 ? s : null;
		const r = q.get('rank');
		rank = r === 'artists' || r === 'albums' ? r : 'songs';
		const g = q.get('range');
		range = g === 'year' || g === 'all' ? g : 'season';
		metric = q.get('metric') === 'hours' ? 'hours' : 'plays';
		playlist = null;
		load();
	}

	async function load() {
		loading = true;
		error = '';
		expanded = {};
		try {
			data = await api.getLens(id, { season, range, rank, metric });
			season = data.season;
		} catch (e) {
			error = humanError(e, 'This season could not be loaded');
			data = null;
		}
		loading = false;
		syncUrl(`/b-sides/${id}?season=${season ?? ''}&rank=${rank}&range=${range}&metric=${metric}`);
	}

	function open(e: CustomEvent<InsightRow>) {
		const r = e.detail;
		if (rank === 'artists') return openArtist(r.name);
		if (rank === 'albums' && r.sub) return openAlbum(r.sub, r.name);
		if (r.album) return openAlbum(r.album_artist ?? r.sub ?? '', r.album);
	}

	async function toggle(e: CustomEvent<InsightRow>) {
		const r = e.detail;
		if (expanded[r.key]) {
			const { [r.key]: _, ...rest } = expanded;
			expanded = rest;
			return;
		}
		expanded = { ...expanded, [r.key]: 'loading' };
		try {
			expanded = { ...expanded, [r.key]: (await api.getVersions(r.key)).rows };
		} catch {
			const { [r.key]: _, ...rest } = expanded;
			expanded = rest;
		}
	}

	async function action(e: CustomEvent<{ id: string; row: InsightRow }>) {
		if (e.detail.id !== 'not') return;
		try {
			await api.setFacetOverride('work', e.detail.row.key, id as 'christmas' | 'halloween' | 'easter', false);
			notice = `“${e.detail.row.name}” is no longer counted as ${lowerLabel}. Every version of it moved.`;
			await load();
		} catch (err) {
			error = humanError(err, 'That could not be changed');
		}
	}

	async function savePlaylist() {
		if (!season) return;
		playlistBusy = true;
		try {
			playlist = await api.saveLensPlaylist(id, season);
		} catch (e) {
			notice = humanError(e, 'The playlist could not be saved');
		}
		playlistBusy = false;
	}

	const fmt = (d: string, opts: Intl.DateTimeFormatOptions = { day: 'numeric', month: 'long' }) =>
		new Date(`${d}T12:00:00`).toLocaleDateString(undefined, opts);

	// The curve, drawn as one bar per day.
	const W = 720;
	const H = 150;
	$: curve = data?.curve ?? [];
	$: peak = Math.max(0.01, ...curve.map((c) => c.share));
	$: step = curve.length ? W / curve.length : 0;
	$: startIdx = data ? curve.findIndex((c) => c.date === data!.starts) : -1;
	$: openedIdx = data?.opened ? curve.findIndex((c) => c.date === data!.opened) : -1;
	$: hours = data ? Math.round(data.seconds / 360) / 10 : 0;
</script>

<svelte:head><title>{label || 'Season'} — Tapedeck</title></svelte:head>

{#if loading && !data}
	<div class="centre"><div class="spin"></div></div>
{:else if error && !data}
	<div class="panel empty">{error}</div>
{:else if data}
	<div class="head">
		<div style="min-width:0;">
			<div class="eyebrow">B-Sides · {data.in_season ? 'in season' : 'out of season'}</div>
			<h1 class="h-display" style="font-size:38px;margin:0;">{data.label} {data.season}</h1>
			<p class="lede">{fmt(data.starts)} – {fmt(data.ends)}. Everything here is still in Top Charts too.</p>
		</div>
		<div class="actions">
			<button class="btn" disabled={!data.plays} on:click={() => (sharing = true)}>Share card</button>
			<button class="btn" disabled={!data.plays} on:click={() => (playing = true)}>
				<svg viewBox="0 0 24 24" width="13" fill="currentColor" aria-hidden="true"><path d="M7 4.5v15a1 1 0 0 0 1.5.86l12.5-7.5a1 1 0 0 0 0-1.72L8.5 3.64A1 1 0 0 0 7 4.5z"/></svg>
				Play the Mixdown
			</button>
			{#if playlist}
				<a class="btn primary" href="/playlists/saved">{playlist.created ? 'Saved' : 'Already saved'} · {playlist.tracks} songs →</a>
			{:else}
				<button class="btn primary" disabled={!data.plays || playlistBusy} on:click={savePlaylist}>Save as playlist</button>
			{/if}
		</div>
	</div>

	{#if data.seasons.length > 1}
		<div style="margin-bottom:16px;">
			<Seg chips small value={String(data.season)} options={data.seasons.map((s) => ({ k: String(s), label: String(s) }))}
				on:change={(e) => { season = Number(e.detail); load(); }} />
		</div>
	{/if}

	<div class="stats">
		<div class="stat"><div class="n h-display">{data.plays.toLocaleString()}</div><div class="l">plays</div></div>
		<div class="stat"><div class="n h-display">{hours.toLocaleString()}h</div><div class="l">listened</div></div>
		<div class="stat"><div class="n h-display">{data.share_pct ?? '—'}{data.share_pct != null ? '%' : ''}</div><div class="l">of everything you played that season</div></div>
		<div class="stat"><div class="n h-display">{data.opened ? fmt(data.opened, { day: 'numeric', month: 'short' }) : '—'}</div><div class="l">{data.opened ? 'your season opened' : 'not opened yet'}</div></div>
	</div>

	{#if curve.length}
		<div class="panel curve">
			<div class="c-head">
				<span>Share of each day's listening</span>
				<span class="muted">peak {Math.round(peak * 100)}%</span>
			</div>
			<svg viewBox="0 0 {W} {H}" preserveAspectRatio="none" role="img" aria-label="{data.label} share of each day's listening">
				{#if startIdx >= 0}<line x1={startIdx * step} x2={startIdx * step} y1="0" y2={H} class="mark" />{/if}
				{#each curve as c, i}
					{#if c.share > 0}
						<rect x={i * step} width={Math.max(1, step * 0.8)} y={H - (c.share / peak) * (H - 6)} height={(c.share / peak) * (H - 6)}
							class:lifted={openedIdx >= 0 && i >= openedIdx}><title>{fmt(c.date)}: {c.season_plays} of {c.plays} plays</title></rect>
					{/if}
				{/each}
				{#if openedIdx >= 0}<line x1={openedIdx * step} x2={openedIdx * step} y1="0" y2={H} class="opened" />{/if}
			</svg>
			<div class="c-foot">
				<span>{fmt(curve[0].date, { day: 'numeric', month: 'short' })}</span>
				<span>{fmt(curve[curve.length - 1].date, { day: 'numeric', month: 'short' })}</span>
			</div>
		</div>
	{/if}

	{#if data.out_of_season > 0}
		<p class="note">And {data.out_of_season.toLocaleString()} {lowerLabel} {data.out_of_season === 1 ? 'listen' : 'listens'} outside any season — some songs get played every day of the year.</p>
	{/if}
	{#if notice}<p class="note">{notice}</p>{/if}

	<div class="controls">
		<Seg chips value={rank} options={[{ k: 'songs', label: 'Songs' }, { k: 'artists', label: 'Artists' }, { k: 'albums', label: 'Albums' }]}
			on:change={(e) => { rank = e.detail as Rank; load(); }} />
		<Seg value={range} options={[{ k: 'season', label: 'This season' }, { k: 'year', label: 'Last 12 months' }, { k: 'all', label: 'All time' }]}
			on:change={(e) => { range = e.detail as Range; load(); }} />
		<Seg value={metric} options={[{ k: 'plays', label: 'Plays' }, { k: 'hours', label: 'Hours' }]}
			on:change={(e) => { metric = e.detail === 'hours' ? 'hours' : 'plays'; load(); }} />
	</div>

	{#if !data.rows.length}
		<div class="panel empty">
			Nothing {lowerLabel} here yet.
			{#if data.pending}Tapedeck is still sorting {data.pending.toLocaleString()} listens.{/if}
		</div>
	{:else}
		<RankedRows rows={data.rows} {metric} round={rank === 'artists'} {expanded}
			extra={(r) => (rank === 'songs' && r.versions && r.versions > 1 ? `${r.versions} versions` : '')}
			expandable={(r) => rank === 'songs' && (r.versions ?? 0) > 1}
			actions={rank === 'songs' ? [{ id: 'not', label: `Not ${lowerLabel}` }] : []}
			on:open={open} on:toggle={toggle} on:action={action} />
		<ToTop />
	{/if}

	<ShareChapter endpoint={api.lensImageEndpoint(id, data.season)} label="{data.label} {data.season}" bind:open={sharing} />
	<MixdownPlayer unit="year" lens={id} season={data.season} label="{data.label} {data.season}" bind:open={playing} />
{/if}

<style>
	.head { display: flex; align-items: flex-end; justify-content: space-between; gap: 16px; flex-wrap: wrap; margin-bottom: 18px; }
	.eyebrow { font-size: 11px; letter-spacing: .16em; text-transform: uppercase; color: var(--td-accent); font-weight: 600; margin-bottom: 8px; }
	.lede { margin: 6px 0 0; color: var(--td-muted); font-size: 15px; }
	.actions { display: flex; gap: 8px; flex-wrap: wrap; }
	.btn {
		display: inline-flex; align-items: center; gap: 6px; border: 1px solid var(--td-border); background: var(--td-surface);
		color: var(--td-text); border-radius: 999px; cursor: pointer; font: inherit; font-size: 13px; padding: 8px 16px; text-decoration: none;
	}
	.btn:hover { border-color: var(--td-accent); }
	.btn.primary { background: var(--td-accent); color: var(--td-on-accent); border-color: transparent; }
	.btn:disabled { opacity: 0.5; cursor: default; }
	.stats { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 12px; margin-bottom: 16px; }
	.stat { background: var(--td-surface); border: 1px solid var(--td-border); border-radius: 18px; padding: 14px 16px; }
	.stat .n { font-size: 28px; }
	.stat .l { font-size: 12.5px; color: var(--td-muted); margin-top: 2px; }
	.curve { padding: 16px 18px 12px; margin-bottom: 14px; }
	.curve svg { width: 100%; height: 150px; display: block; }
	.curve rect { fill: var(--td-accent-2); }
	.curve rect.lifted { fill: var(--td-accent); }
	.mark { stroke: var(--td-border); stroke-width: 1; stroke-dasharray: 3 3; }
	.opened { stroke: var(--td-accent); stroke-width: 1.5; }
	.c-head, .c-foot { display: flex; justify-content: space-between; font-size: 12px; color: var(--td-muted); }
	.c-head { margin-bottom: 8px; font-weight: 600; color: var(--td-text); }
	.muted { color: var(--td-muted); font-weight: 400; }
	.c-foot { margin-top: 6px; }
	.note { font-size: 13px; color: var(--td-muted); margin: 0 0 14px; }
	.controls { display: flex; gap: 12px; flex-wrap: wrap; align-items: center; margin-bottom: 18px; }
	.empty { border-radius: 22px; padding: 44px; text-align: center; color: var(--td-muted); }
	.centre { display: flex; justify-content: center; padding: 70px; }
	.spin { width: 22px; height: 22px; border: 2px solid var(--td-track); border-top-color: var(--td-accent); border-radius: 50%; animation: td-spin 0.7s linear infinite; }
</style>
