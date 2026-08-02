<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { ChartRow, ChartKind, ChartRange } from '$lib/api';
	import { openAlbum, openArtist } from '$lib/entities';
	import CoverArt from '$lib/components/CoverArt.svelte';

	const kinds: { k: ChartKind; label: string }[] = [
		{ k: 'artists', label: 'Artists' },
		{ k: 'albums', label: 'Albums' },
		{ k: 'tracks', label: 'Tracks' }
	];
	// Matches `range_after` on the server. "Genres" is in the design but not
	// here: Tapedeck has no genre data worth ranking yet, and a tab that always
	// reads "Unknown 100%" is worse than no tab.
	const timeframes: { k: ChartRange; label: string; word: string }[] = [
		{ k: 'today', label: 'Today', word: 'today' },
		{ k: 'd7', label: '7 Days', word: 'in the last 7 days' },
		{ k: 'month', label: '30 Days', word: 'in the last 30 days' },
		{ k: 'year', label: 'This Year', word: 'this year' },
		{ k: 'all', label: 'All Time', word: 'all time' }
	];

	let kind: ChartKind = 'artists';
	let range: ChartRange = 'all';
	let metric: 'plays' | 'hours' = 'plays';
	let rows: ChartRow[] = [];
	let loading = true;

	$: rangeWord = timeframes.find((t) => t.k === range)?.word ?? '';
	$: kindLabel = kinds.find((k) => k.k === kind)?.label ?? '';
	$: unit = metric === 'hours' ? 'h' : '';
	$: value = (r: ChartRow) =>
		metric === 'plays' ? r.plays : Math.round((r.seconds / 3600) * 10) / 10;
	$: max = Math.max(1, ...rows.map((r) => value(r)));
	$: podium = rows.slice(0, 3);
	$: rest = rows.slice(3);
	$: totalPlays = rows.reduce((n, r) => n + r.plays, 0);

	/** There is no track page, so a track opens its *artist*; an album opens
	 *  itself. Same rule as the Statistics top lists. */
	function openRow(r: ChartRow) {
		if (kind === 'artists') return openArtist(r.name);
		if (kind === 'albums' && r.sub) return openAlbum(r.sub, r.name);
		if (kind === 'tracks' && r.sub) return openArtist(r.sub);
		return undefined;
	}

	async function load() {
		loading = true;
		try {
			rows = (await api.getChart(kind, range, 100)).rows;
		} catch (e) {
			console.warn('charts', e);
			rows = [];
		}
		loading = false;
		// Linkable: a chart you're looking at should survive a reload.
		history.replaceState(null, '', `/charts?kind=${kind}&range=${range}`);
	}

	function setKind(k: ChartKind) { kind = k; load(); }
	function setRange(r: ChartRange) { range = r; load(); }

	const medals = ['#c9a227', '#a8a8ad', '#a3703c'];

	onMount(() => {
		const q = new URLSearchParams(window.location.search);
		const k = q.get('kind') as ChartKind | null;
		const r = q.get('range') as ChartRange | null;
		if (k && kinds.some((x) => x.k === k)) kind = k;
		if (r && timeframes.some((x) => x.k === r)) range = r;
		load();
	});
</script>

<svelte:head><title>Top Charts — Tapedeck</title></svelte:head>

<div style="display:flex;align-items:flex-end;justify-content:space-between;gap:16px;flex-wrap:wrap;margin-bottom:18px;">
	<div style="min-width:0;">
		<div class="eyebrow">Top Charts</div>
		<h1 class="h-display" style="font-size:38px;margin:0;">Your Top {kindLabel}</h1>
		<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;">
			{rows.length ? `${rows.length} charted · ${totalPlays.toLocaleString()} plays ${rangeWord}` : `Nothing charted ${rangeWord}`}
		</p>
	</div>
	<div style="display:inline-flex;background:var(--td-sidebar);border-radius:999px;padding:3px;gap:2px;">
		<button class="seg-btn" class:on={metric === 'plays'} on:click={() => (metric = 'plays')}>Plays</button>
		<button class="seg-btn" class:on={metric === 'hours'} on:click={() => (metric = 'hours')}>Hours</button>
	</div>
</div>

<div style="display:flex;gap:8px;flex-wrap:wrap;margin-bottom:16px;">
	{#each kinds as k}
		<button class="kind" class:on={kind === k.k} on:click={() => setKind(k.k)}>{k.label}</button>
	{/each}
</div>

<div style="display:inline-flex;flex-wrap:wrap;background:var(--td-sidebar);border-radius:999px;padding:4px;gap:2px;margin-bottom:26px;">
	{#each timeframes as tf}
		<button class="seg-btn" class:on={range === tf.k} on:click={() => setRange(tf.k)}>{tf.label}</button>
	{/each}
</div>

{#if loading}
	<div style="display:flex;justify-content:center;padding:70px;"><div class="spin"></div></div>
{:else if !rows.length}
	<div class="panel" style="border-radius:22px;padding:44px;text-align:center;color:var(--td-muted);">
		Nothing charted {rangeWord} yet — press play on something.
	</div>
{:else}
	{#if podium.length === 3}
		<div class="podium">
			{#each podium as p, i}
				<button class="pod" on:click={() => openRow(p)}>
					<div class="pod-art">
						<CoverArt url={p.artwork_url} caaId={p.caa_id} releaseMbid={p.caa_release_mbid}
							seed={p.name} size={200} radius={16} />
					</div>
					<div style="display:flex;align-items:center;gap:9px;margin-bottom:4px;">
						<span class="medal" style="background:{medals[i]};">{i + 1}</span>
						<div class="h-display pod-name">{p.name}</div>
					</div>
					{#if p.sub}<div class="pod-sub">{p.sub}</div>{/if}
					<div style="display:flex;align-items:center;gap:10px;margin-top:10px;">
						<div class="bar" style="flex:1;"><div class="fill" style="width:{(value(p) / max) * 100}%"></div></div>
						<span class="pod-val">{value(p)}{unit}</span>
					</div>
				</button>
			{/each}
		</div>
	{/if}

	<div class="panel" style="border-radius:22px;overflow:hidden;">
		{#each rest as r, i}
			<button class="row" on:click={() => openRow(r)}>
				<span class="rank h-display">{i + 4}</span>
				<CoverArt url={r.artwork_url} caaId={r.caa_id} releaseMbid={r.caa_release_mbid}
					seed={r.name} size={38} radius={kind === 'artists' ? 999 : 9} />
				<div style="flex:1;min-width:0;">
					<div class="name">{r.name}</div>
					{#if r.sub}<div class="sub">{r.sub}</div>{/if}
					<div class="bar" style="max-width:260px;margin-top:6px;"><div class="fill" style="width:{(value(r) / max) * 100}%"></div></div>
				</div>
				<span class="val">{value(r)}{unit}</span>
			</button>
		{/each}
	</div>

	<div style="text-align:center;margin-top:22px;font-size:12.5px;color:var(--td-muted-3);">
		Top {rows.length} {kindLabel.toLowerCase()} {rangeWord}
	</div>
{/if}

<style>
	.spin {
		width: 22px; height: 22px; border: 2px solid var(--td-track);
		border-top-color: var(--td-accent); border-radius: 50%;
		animation: td-spin 0.7s linear infinite;
	}
	.eyebrow {
		font-size: 11px; letter-spacing: .16em; text-transform: uppercase;
		color: #c67139; font-weight: 600; margin-bottom: 8px;
	}
	.seg-btn {
		border: none; background: transparent; cursor: pointer; color: var(--td-nav-fg);
		font-size: 12.5px; padding: 6px 14px; border-radius: 999px; font-family: inherit;
	}
	.seg-btn.on { background: var(--td-accent); color: #fbf5ea; }
	/* Kind tabs are a separate control from the timeframe pills, so they read as
	   outlined chips rather than a second segmented group. */
	.kind {
		border: 1px solid var(--td-border);
		background: var(--td-surface);
		color: var(--td-nav-fg);
		font: inherit;
		font-size: 13px;
		font-weight: 600;
		padding: 8px 18px;
		border-radius: 999px;
		cursor: pointer;
	}
	.kind:hover { border-color: var(--td-accent); }
	.kind.on { background: var(--td-panel); color: #f4e7cf; border-color: transparent; }

	.podium {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(210px, 1fr));
		gap: 16px;
		margin-bottom: 26px;
	}
	.pod {
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 22px;
		padding: 16px;
		text-align: left;
		font: inherit;
		color: inherit;
		cursor: pointer;
	}
	.pod:hover { border-color: var(--td-accent); }
	.pod-art {
		aspect-ratio: 1;
		border-radius: 16px;
		overflow: hidden;
		margin-bottom: 12px;
		background: var(--td-track);
	}
	.pod-art :global(img), .pod-art :global(div) {
		width: 100% !important; height: 100% !important; border-radius: 16px !important;
	}
	.medal {
		flex: none;
		width: 22px; height: 22px;
		border-radius: 50%;
		display: inline-flex; align-items: center; justify-content: center;
		font-size: 12px; font-weight: 700; color: #1b1712;
	}
	.pod-name {
		font-size: 19px; line-height: 1.05; min-width: 0;
		white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
	}
	.pod-sub {
		font-size: 12.5px; color: var(--td-muted-2);
		white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
	}
	.pod-val { font-size: 13px; font-weight: 600; font-variant-numeric: tabular-nums; white-space: nowrap; }

	.row {
		display: flex; align-items: center; gap: 14px;
		width: 100%; padding: 12px 18px;
		border: none; border-bottom: 1px solid var(--td-border);
		background: transparent; color: inherit; font: inherit;
		text-align: left; cursor: pointer;
	}
	.row:last-child { border-bottom: none; }
	.row:hover { background: var(--td-nav-hover); }
	.rank {
		width: 30px; flex: none; text-align: center;
		font-size: 16px; color: var(--td-muted-3); font-variant-numeric: tabular-nums;
	}
	.name { font-size: 14.5px; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.sub { font-size: 12.5px; color: var(--td-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.bar { height: 6px; border-radius: 999px; background: var(--td-track); overflow: hidden; }
	.fill { height: 100%; border-radius: 999px; background: linear-gradient(90deg, var(--td-accent-2), var(--td-accent)); }
	.val {
		font-size: 13px; font-weight: 600; width: 76px;
		text-align: right; flex: none; font-variant-numeric: tabular-nums;
	}
</style>
