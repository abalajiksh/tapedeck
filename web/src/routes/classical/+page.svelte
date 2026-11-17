<script lang="ts">
	/**
	 * Classical: pieces, composers, performances and who played them, kept out
	 * of Top Charts (Settings → "Keep classical in its own tab").
	 *
	 * Filters stack — Bach → Partita No. 2 → violinists — and every one is in
	 * the URL through `syncUrl`, so a view can be bookmarked and Back works.
	 */
	import { onMount } from 'svelte';
	import { syncUrl } from '$lib/url';
	import { api, humanError, type ClassicalChart, type ClassicalMetric, type ClassicalRank, type ChartRange, type InsightRow, type ClassicalStats as Stats, type ClassicalTimeline as TimelineData } from '$lib/api';
	import ClassicalStats from '$lib/components/insights/ClassicalStats.svelte';
	import ClassicalTimeline from '$lib/components/insights/ClassicalTimeline.svelte';
	import { openAlbum } from '$lib/entities';
	import Seg from '$lib/components/insights/Seg.svelte';
	import RankedRows from '$lib/components/insights/RankedRows.svelte';
	import ToTop from '$lib/components/ToTop.svelte';

	const ranks: { k: ClassicalRank; label: string; noun: string }[] = [
		{ k: 'pieces', label: 'Pieces', noun: 'pieces' },
		{ k: 'composers', label: 'Composers', noun: 'composers' },
		{ k: 'performances', label: 'Performances', noun: 'performances' },
		{ k: 'conductor', label: 'Conductors', noun: 'conductors' },
		{ k: 'orchestra', label: 'Orchestras', noun: 'orchestras' },
		{ k: 'ensemble', label: 'Ensembles', noun: 'ensembles' },
		{ k: 'soloist', label: 'Soloists', noun: 'soloists' }
	];
	const timeframes: { k: ChartRange; label: string; word: string }[] = [
		{ k: 'today', label: 'Today', word: 'today' },
		{ k: 'd7', label: '7 Days', word: 'in the last 7 days' },
		{ k: 'month', label: '30 Days', word: 'in the last 30 days' },
		{ k: 'year', label: 'This Year', word: 'this year' },
		{ k: 'all', label: 'All Time', word: 'all time' }
	];

	type View = 'rankings' | 'stats' | 'timeline';
	const views = [
		{ k: 'rankings', label: 'Rankings' },
		{ k: 'stats', label: 'Statistics' },
		{ k: 'timeline', label: 'Timeline' }
	];
	let view: View = 'rankings';
	let stats: Stats | null = null;
	let timeline: TimelineData | null = null;
	let rank: ClassicalRank = 'pieces';
	let range: ChartRange = 'all';
	let metric: ClassicalMetric = 'hours';
	/** Each filter keeps the name it was picked by, for its chip. */
	let composer: { key: string; name: string } | null = null;
	let piece: { key: string; name: string } | null = null;
	let performer: { key: string; name: string } | null = null;
	let instrument: string | null = null;

	let data: ClassicalChart | null = null;
	let loading = true;
	let error = '';
	let expanded: Record<string, InsightRow[] | 'loading'> = {};

	$: performerRank = ['conductor', 'orchestra', 'ensemble', 'soloist'].includes(rank);
	$: if (performerRank && metric === 'hearings') metric = 'hours';
	$: metrics = [
		{ k: 'hours', label: 'Hours' },
		{ k: 'plays', label: 'Plays' },
		{ k: 'hearings', label: 'Hearings', disabled: !['pieces', 'composers'].includes(rank),
			hint: 'Complete hearings: every movement of a piece in one sitting' }
	];
	$: rangeWord = timeframes.find((t) => t.k === range)?.word ?? '';
	$: noun = ranks.find((r) => r.k === rank)?.noun ?? 'pieces';
	$: rows = data?.rows ?? [];
	$: hours = Math.round(rows.reduce((n, r) => n + r.seconds, 0) / 360) / 10;

	function url() {
		const q = new URLSearchParams({ view, rank, range, metric });
		if (composer) q.set('composer', `${composer.key}|${composer.name}`);
		if (piece) q.set('piece', `${piece.key}|${piece.name}`);
		if (performer) q.set('performer', `${performer.key}|${performer.name}`);
		if (instrument) q.set('instrument', instrument);
		return `/classical?${q}`;
	}

	async function load() {
		loading = true;
		error = '';
		expanded = {};
		try {
			if (view === 'stats') {
				stats = await api.getClassicalStats(range);
				loading = false;
				syncUrl(url());
				return;
			}
			if (view === 'timeline') {
				timeline = await api.getClassicalTimeline(range);
				loading = false;
				syncUrl(url());
				return;
			}
			data = await api.getClassical({
				rank, range, metric,
				composer: composer?.key, piece: piece?.key, performer: performer?.key,
				instrument: rank === 'soloist' ? instrument : null
			});
		} catch (e) {
			error = humanError(e, 'Classical could not be loaded');
			data = null;
		}
		loading = false;
		syncUrl(url());
	}

	/** A row narrows the view rather than leaving it: a composer opens their
	 *  pieces, a piece its performances, a performer what they played. */
	function open(e: CustomEvent<InsightRow>) {
		const r = e.detail;
		if (rank === 'composers') {
			composer = { key: r.key, name: r.name };
			rank = 'pieces';
		} else if (rank === 'pieces') {
			piece = { key: r.key, name: r.name };
			rank = 'performances';
		} else if (rank === 'performances') {
			if (r.album) return openAlbum(r.album_artist ?? r.sub ?? '', r.album);
			return;
		} else if (rank === 'movements') {
			return;
		} else {
			performer = { key: r.key, name: r.name };
			rank = 'pieces';
		}
		load();
	}

	/** An opera, a symphony, a suite: its movements and numbers in place. */
	async function toggle(e: CustomEvent<InsightRow>) {
		const r = e.detail;
		if (expanded[r.key]) {
			const { [r.key]: _, ...rest } = expanded;
			expanded = rest;
			return;
		}
		expanded = { ...expanded, [r.key]: 'loading' };
		try {
			const m = await api.getClassical({ rank: 'movements', range, metric: 'plays', piece: r.key, limit: 60 });
			expanded = { ...expanded, [r.key]: m.rows };
		} catch {
			const { [r.key]: _, ...rest } = expanded;
			expanded = rest;
		}
	}

	async function action(e: CustomEvent<{ id: string; row: InsightRow }>) {
		if (e.detail.id !== 'not') return;
		try {
			await api.setFacetOverride('work', e.detail.row.key, 'classical', false);
			await load();
		} catch (err) {
			error = humanError(err, 'That could not be changed');
		}
	}

	function toComposer(e: CustomEvent<{ key: string; name: string }>) {
		composer = { key: e.detail.key, name: e.detail.name };
		view = 'rankings';
		rank = 'pieces';
		load();
	}
	function toPiece(e: CustomEvent<{ key: string; title: string }>) {
		piece = { key: e.detail.key, name: e.detail.title };
		view = 'rankings';
		rank = 'performances';
		load();
	}

	function clear(which: 'composer' | 'piece' | 'performer') {
		if (which === 'composer') composer = null;
		if (which === 'piece') piece = null;
		if (which === 'performer') performer = null;
		load();
	}

	function parse(v: string | null): { key: string; name: string } | null {
		if (!v) return null;
		const i = v.lastIndexOf('|');
		return i < 0 ? { key: v, name: v } : { key: v.slice(0, i), name: v.slice(i + 1) };
	}

	onMount(() => {
		const q = new URLSearchParams(window.location.search);
		const v = q.get('view');
		if (v === 'stats' || v === 'timeline') view = v;
		const r = q.get('rank') as ClassicalRank | null;
		if (r && ranks.some((x) => x.k === r)) rank = r;
		const t = q.get('range') as ChartRange | null;
		if (t && timeframes.some((x) => x.k === t)) range = t;
		const m = q.get('metric');
		if (m === 'hours' || m === 'plays' || m === 'hearings') metric = m;
		composer = parse(q.get('composer'));
		piece = parse(q.get('piece'));
		performer = parse(q.get('performer'));
		instrument = q.get('instrument');
		load();
	});

	function extra(r: InsightRow): string {
		if (rank === 'pieces' && r.versions && r.versions > 1) return `${r.versions} recordings`;
		if (rank === 'composers' && r.versions) return `${r.versions} ${r.versions === 1 ? 'piece' : 'pieces'}`;
		if (performerRank && r.versions) return `${r.versions} ${r.versions === 1 ? 'piece' : 'pieces'}`;
		if (metric !== 'hearings' && r.hearings) return `${r.hearings} complete ${r.hearings === 1 ? 'hearing' : 'hearings'}`;
		return '';
	}
	const pct = (n: number, of: number) => (of > 0 ? Math.round((n / of) * 100) : 0);
</script>

<svelte:head><title>Classical — Tapedeck</title></svelte:head>

<div class="head">
	<div style="min-width:0;">
		<div class="eyebrow">Classical</div>
		<h1 class="h-display" style="font-size:38px;margin:0;">{view === 'stats' ? 'Your classical, in numbers' : view === 'timeline' ? 'Your classical, in time' : `Your ${ranks.find((r) => r.k === rank)?.label ?? 'Pieces'}`}</h1>
		<p class="lede">
			{#if view === 'stats' && stats}{Math.round(stats.seconds / 360) / 10} hours of classical {rangeWord}
			{:else if view === 'timeline' && timeline}{timeline.pieces.length} pieces {rangeWord}
			{:else if rows.length}{rows.length} {noun} · {hours.toLocaleString()} hours {rangeWord}{:else}Nothing classical {rangeWord}{/if}
		</p>
	</div>
	<div style="display:flex;gap:10px;flex-wrap:wrap;">
		<Seg options={views} value={view} on:change={(e) => { view = e.detail as View; load(); }} />
		{#if view === 'rankings'}
			<Seg options={metrics} value={metric} on:change={(e) => { metric = e.detail as ClassicalMetric; load(); }} />
		{/if}
	</div>
</div>

{#if view === 'rankings'}
<div style="margin-bottom:12px;">
	<Seg chips options={ranks} value={rank} on:change={(e) => { rank = e.detail as ClassicalRank; load(); }} />
</div>

{/if}

{#if view === 'rankings' && rank === 'soloist' && data?.instruments.length}
	<div style="margin-bottom:12px;">
		<Seg chips small value={instrument ?? ''} on:change={(e) => { instrument = e.detail || null; load(); }}
			options={[{ k: '', label: 'Every instrument' }, ...data.instruments.map((i) => ({ k: i.name, label: i.name[0].toUpperCase() + i.name.slice(1) }))]} />
	</div>
{/if}

<div style="margin-bottom:18px;">
	<Seg options={timeframes} value={range} on:change={(e) => { range = e.detail as ChartRange; load(); }} />
</div>

{#if view === 'rankings' && (composer || piece || performer)}
	<div class="filters">
		{#if composer}<button class="filter" on:click={() => clear('composer')}>Composer: {composer.name} <span aria-hidden="true">×</span></button>{/if}
		{#if piece}<button class="filter" on:click={() => clear('piece')}>Piece: {piece.name} <span aria-hidden="true">×</span></button>{/if}
		{#if performer}<button class="filter" on:click={() => clear('performer')}>Played by {performer.name} <span aria-hidden="true">×</span></button>{/if}
	</div>
{/if}

{#if view === 'rankings' && data?.coverage && data.coverage.seconds > 0}
	<p class="note">
		Roles stated by MusicBrainz for {pct(data.coverage.stated, data.coverage.seconds)}% of these hours,
		inferred for {pct(data.coverage.inferred, data.coverage.seconds)}% (marked •), unknown for the rest.
		A concerto counts in full for its soloist, conductor and orchestra.
	</p>
{/if}

{#if view === 'stats'}
	{#if loading && !stats}
		<div class="centre"><div class="spin"></div></div>
	{:else if error}
		<div class="panel empty">{error}</div>
	{:else if stats && stats.seconds > 0}
		<ClassicalStats {stats} on:composer={toComposer} />
	{:else}
		<div class="panel empty">Nothing classical {rangeWord} yet.</div>
	{/if}
{:else if view === 'timeline'}
	{#if loading && !timeline}
		<div class="centre"><div class="spin"></div></div>
	{:else if error}
		<div class="panel empty">{error}</div>
	{:else if timeline && timeline.pieces.length}
		<ClassicalTimeline {timeline} on:piece={toPiece} />
	{:else}
		<div class="panel empty">Nothing classical {rangeWord} to place on the timeline yet.</div>
	{/if}
{:else if loading}
	<div class="centre"><div class="spin"></div></div>
{:else if error}
	<div class="panel empty">{error}</div>
{:else if !rows.length}
	<div class="panel empty">
		Nothing classical {rangeWord} yet.
		{#if data?.pending}Tapedeck is still sorting {data.pending.toLocaleString()} listens — they appear here as it reaches them.{/if}
	</div>
{:else}
	<RankedRows {rows} metric={metric} round={performerRank || rank === 'composers'} {expanded} {extra}
		expandable={(r) => rank === 'pieces' && r.key.startsWith('w:')}
		actions={rank === 'pieces' ? [{ id: 'not', label: 'Not classical' }] : []}
		on:open={open} on:toggle={toggle} on:action={action} />
	{#if data?.pending}
		<p class="foot">Still sorting {data.pending.toLocaleString()} listens; until then they stay in Top Charts.</p>
	{/if}
	<ToTop />
{/if}

<style>
	.head { display: flex; align-items: flex-end; justify-content: space-between; gap: 16px; flex-wrap: wrap; margin-bottom: 18px; }
	.eyebrow { font-size: 11px; letter-spacing: .16em; text-transform: uppercase; color: var(--td-accent); font-weight: 600; margin-bottom: 8px; }
	.lede { margin: 6px 0 0; color: var(--td-muted); font-size: 15px; }
	.filters { display: flex; gap: 8px; flex-wrap: wrap; margin-bottom: 14px; }
	.filter {
		border: 1px solid var(--td-accent); background: var(--td-surface); color: var(--td-text);
		font: inherit; font-size: 12.5px; padding: 5px 12px; border-radius: 999px; cursor: pointer;
	}
	.note { font-size: 12.5px; color: var(--td-muted); margin: 0 0 14px; max-width: 760px; }
	.foot { text-align: center; margin-top: 18px; font-size: 12.5px; color: var(--td-muted-3); }
	.empty { border-radius: 22px; padding: 44px; text-align: center; color: var(--td-muted); }
	.centre { display: flex; justify-content: center; padding: 70px; }
	.spin { width: 22px; height: 22px; border: 2px solid var(--td-track); border-top-color: var(--td-accent); border-radius: 50%; animation: td-spin 0.7s linear infinite; }
</style>
