<script lang="ts">
	/**
	 * Scores & Ambient: instrumental film, game and TV score, and albums whose
	 * top genre is ambient — kept out of Top Charts (Settings → "Keep scores
	 * and ambient in their own tab").
	 *
	 * A song on a soundtrack is a song and stays in Top Charts. Score only
	 * moves on evidence; an album that merely looks like score is offered
	 * here to confirm, never moved on a guess.
	 */
	import { onMount } from 'svelte';
	import { syncUrl } from '$lib/url';
	import { api, humanError, type ChartKind, type ChartRange, type ChartRow, type InsightRow, type ScoresChart } from '$lib/api';
	import { openAlbum, openArtist } from '$lib/entities';
	import CoverArt from '$lib/components/CoverArt.svelte';
	import ArtistLinks from '$lib/components/ArtistLinks.svelte';
	import Seg from '$lib/components/insights/Seg.svelte';
	import ToTop from '$lib/components/ToTop.svelte';

	type Section = 'all' | 'score' | 'ambient';
	const sections = [
		{ k: 'all', label: 'Both' },
		{ k: 'score', label: 'Score' },
		{ k: 'ambient', label: 'Ambient' }
	];
	const kinds = [
		{ k: 'tracks', label: 'Tracks' },
		{ k: 'albums', label: 'Albums' },
		{ k: 'artists', label: 'Composers & artists' }
	];
	const timeframes: { k: ChartRange; label: string; word: string }[] = [
		{ k: 'today', label: 'Today', word: 'today' },
		{ k: 'd7', label: '7 Days', word: 'in the last 7 days' },
		{ k: 'month', label: '30 Days', word: 'in the last 30 days' },
		{ k: 'year', label: 'This Year', word: 'this year' },
		{ k: 'all', label: 'All Time', word: 'all time' }
	];

	let section: Section = 'all';
	let kind: ChartKind = 'tracks';
	let range: ChartRange = 'all';
	let metric: 'plays' | 'hours' = 'plays';
	let data: ScoresChart | null = null;
	let loading = true;
	let error = '';
	let busy = '';

	$: rows = data?.rows ?? [];
	$: rangeWord = timeframes.find((t) => t.k === range)?.word ?? '';
	$: value = (r: ChartRow) => (metric === 'plays' ? r.plays : Math.round((r.seconds / 3600) * 10) / 10);
	$: max = Math.max(1, ...rows.map((r) => value(r)));
	$: unit = metric === 'hours' ? 'h' : '';

	async function load() {
		loading = true;
		error = '';
		try {
			data = await api.getScores(section, kind, range, metric);
		} catch (e) {
			error = humanError(e, 'Scores could not be loaded');
			data = null;
		}
		loading = false;
		syncUrl(`/scores?section=${section}&kind=${kind}&range=${range}&metric=${metric}`);
	}

	function openRow(r: ChartRow) {
		if (kind === 'artists') return openArtist(r.name);
		if (kind === 'albums' && r.sub) return openAlbum(r.sub, r.name);
		if (r.album && r.sub) return openAlbum(r.album_artist ?? r.sub, r.album);
		if (r.sub) return openArtist(r.sub);
	}

	/** The key an album override is filed under: the album's folded name and
	 *  who it is filed under, as the server builds it. */
	const albumKey = (album: string, filed: string) => `${album.trim().toLowerCase()}\u001f${filed.trim().toLowerCase()}`;

	async function decide(s: InsightRow, isScore: boolean) {
		busy = s.key;
		try {
			if (isScore) await api.setFacetOverride('album', s.key, 'score', true);
			else await api.setFacetOverride('album', s.key, 'score_suggested', false);
			await load();
		} catch (e) {
			error = humanError(e, 'That could not be changed');
		}
		busy = '';
	}

	async function notScore(r: ChartRow) {
		if (!r.sub) return;
		const key = albumKey(r.name, r.sub);
		busy = key;
		try {
			await api.setFacetOverride('album', key, 'score', false);
			await api.setFacetOverride('album', key, 'ambient', false);
			await load();
		} catch (e) {
			error = humanError(e, 'That could not be changed');
		}
		busy = '';
	}

	onMount(() => {
		const q = new URLSearchParams(window.location.search);
		const s = q.get('section');
		if (s === 'all' || s === 'score' || s === 'ambient') section = s;
		const k = q.get('kind');
		if (k === 'tracks' || k === 'albums' || k === 'artists') kind = k;
		const t = q.get('range') as ChartRange | null;
		if (t && timeframes.some((x) => x.k === t)) range = t;
		if (q.get('metric') === 'hours') metric = 'hours';
		load();
	});
</script>

<svelte:head><title>Scores & Ambient — Tapedeck</title></svelte:head>

<div class="head">
	<div style="min-width:0;">
		<div class="eyebrow">Scores & Ambient</div>
		<h1 class="h-display" style="font-size:38px;margin:0;">Music for somewhere else</h1>
		<p class="lede">
			{#if rows.length}{rows.length} charted {rangeWord}{:else}Nothing here {rangeWord}{/if} · film, game and TV score, and ambient records.
			Songs from soundtracks stay in Top Charts.
		</p>
	</div>
	<Seg options={[{ k: 'plays', label: 'Plays' }, { k: 'hours', label: 'Hours' }]} value={metric}
		on:change={(e) => { metric = e.detail === 'hours' ? 'hours' : 'plays'; load(); }} />
</div>

<div class="controls">
	<Seg options={sections} value={section} on:change={(e) => { section = e.detail as Section; load(); }} />
	<Seg chips options={kinds} value={kind} on:change={(e) => { kind = e.detail as ChartKind; load(); }} />
</div>
<div style="margin-bottom:22px;">
	<Seg options={timeframes} value={range} on:change={(e) => { range = e.detail as ChartRange; load(); }} />
</div>

{#if data?.suggestions.length}
	<div class="panel suggest">
		<div class="s-head">
			<div class="h-display" style="font-size:19px;">Looks like score?</div>
			<p>Soundtracks credited to their composer alone, with nothing saying the tracks are instrumental. Nothing moves until you say so.</p>
		</div>
		{#each data.suggestions as s (s.key)}
			<div class="s-row">
				<CoverArt url={s.artwork_url} caaId={s.caa_id ?? null} releaseMbid={s.caa_release_mbid ?? null} seed={s.name} size={34} radius={8} />
				<div class="s-text">
					<div class="name">{s.name}</div>
					<div class="sub">{s.sub} · {s.plays} {s.plays === 1 ? 'play' : 'plays'}</div>
				</div>
				<button class="btn primary" disabled={busy === s.key} on:click={() => decide(s, true)}>It's score</button>
				<button class="btn" disabled={busy === s.key} on:click={() => decide(s, false)}>It's songs</button>
			</div>
		{/each}
	</div>
{/if}

{#if loading}
	<div class="centre"><div class="spin"></div></div>
{:else if error}
	<div class="panel empty">{error}</div>
{:else if !rows.length}
	<div class="panel empty">
		Nothing here {rangeWord} yet.
		{#if data?.pending}Tapedeck is still sorting {data.pending.toLocaleString()} listens.{/if}
	</div>
{:else}
	<div class="panel list">
		{#each rows as r, i}
			<div class="row">
				<span class="rank h-display">{i + 1}</span>
				<button class="main" on:click={() => openRow(r)}>
					<CoverArt url={r.artwork_url} caaId={r.caa_id} releaseMbid={r.caa_release_mbid} seed={r.name} size={38} radius={kind === 'artists' ? 999 : 9} />
					<div style="flex:1;min-width:0;">
						<div class="name">{r.name}</div>
						{#if r.sub}<div class="sub"><ArtistLinks artist={r.sub} credit={r.artist_credit} nested /></div>{/if}
						<div class="bar"><div class="fill" style="width:{(value(r) / max) * 100}%"></div></div>
					</div>
				</button>
				{#if kind === 'albums'}
					<button class="btn small" disabled={busy !== ''} on:click={() => notScore(r)}>Not score</button>
				{/if}
				<span class="val">{value(r)}{unit}</span>
			</div>
		{/each}
	</div>
	<ToTop />
{/if}

<style>
	.head { display: flex; align-items: flex-end; justify-content: space-between; gap: 16px; flex-wrap: wrap; margin-bottom: 18px; }
	.eyebrow { font-size: 11px; letter-spacing: .16em; text-transform: uppercase; color: var(--td-accent); font-weight: 600; margin-bottom: 8px; }
	.lede { margin: 6px 0 0; color: var(--td-muted); font-size: 15px; max-width: 720px; }
	.controls { display: flex; gap: 12px; flex-wrap: wrap; align-items: center; margin-bottom: 12px; }
	.suggest { border-radius: 22px; padding: 18px 20px; margin-bottom: 22px; }
	.s-head p { margin: 4px 0 12px; color: var(--td-muted); font-size: 13px; }
	.s-row { display: flex; align-items: center; gap: 12px; padding: 8px 0; border-top: 1px solid var(--td-border); }
	.s-text { flex: 1; min-width: 0; }
	.list { border-radius: 22px; overflow: hidden; }
	.row { display: flex; align-items: center; gap: 10px; padding: 10px 16px; border-bottom: 1px solid var(--td-border); }
	.row:last-child { border-bottom: none; }
	.row:hover { background: var(--td-nav-hover); }
	.rank { width: 28px; flex: none; text-align: center; font-size: 16px; color: var(--td-muted-3); }
	.main { flex: 1; min-width: 0; display: flex; align-items: center; gap: 14px; border: none; background: transparent; color: inherit; font: inherit; text-align: left; cursor: pointer; padding: 0; }
	.name { font-size: 14.5px; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.sub { font-size: 12.5px; color: var(--td-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.bar { height: 6px; border-radius: 999px; background: var(--td-track); overflow: hidden; max-width: 260px; margin-top: 6px; }
	.fill { height: 100%; border-radius: 999px; background: linear-gradient(90deg, var(--td-accent-2), var(--td-accent)); }
	.val { font-size: 13px; font-weight: 600; width: 64px; text-align: right; flex: none; font-variant-numeric: tabular-nums; }
	.btn { flex: none; border: 1px solid var(--td-border); background: var(--td-surface); color: var(--td-text); border-radius: 999px; cursor: pointer; font: inherit; font-size: 12.5px; padding: 6px 14px; }
	.btn.small { font-size: 11.5px; padding: 4px 10px; color: var(--td-muted); }
	.btn.primary { background: var(--td-accent); color: var(--td-on-accent); border-color: transparent; }
	.btn:disabled { opacity: 0.5; cursor: default; }
	.empty { border-radius: 22px; padding: 44px; text-align: center; color: var(--td-muted); }
	.centre { display: flex; justify-content: center; padding: 70px; }
	.spin { width: 22px; height: 22px; border: 2px solid var(--td-track); border-top-color: var(--td-accent); border-radius: 50%; animation: td-spin 0.7s linear infinite; }
	@media (max-width: 640px) { .btn.small { display: none; } }
</style>
