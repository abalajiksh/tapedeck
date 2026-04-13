<script lang="ts">
	import { onMount } from 'svelte';
	import StatsCard from '$lib/components/StatsCard.svelte';

	let loading = true;

	// Placeholder stats — will come from /api/v1/stats once built
	let stats = {
		totalScrobbles: 0,
		uniqueArtists: 0,
		uniqueAlbums: 0,
		uniqueTracks: 0,
		totalHours: 0,
		avgQuality: 0,
		pctLossless: 0,
		pctDsd: 0,
		topArtist: '—',
		topAlbum: '—',
	};

	// Listening hours heatmap placeholder (hour × day-of-week)
	let heatmap: number[][] = Array.from({ length: 7 }, () => Array(24).fill(0));

	const dayLabels = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];

	onMount(async () => {
		// TODO: fetch from /api/v1/stats
		loading = false;
	});

	function heatColor(val: number, max: number): string {
		if (max === 0 || val === 0) return 'bg-rp-hl-low';
		const pct = val / max;
		if (pct > 0.75) return 'bg-rp-iris';
		if (pct > 0.5) return 'bg-rp-iris/60';
		if (pct > 0.25) return 'bg-rp-iris/35';
		return 'bg-rp-iris/15';
	}

	$: heatMax = Math.max(1, ...heatmap.flat());
</script>

<svelte:head>
	<title>Statistics — Tapedeck</title>
</svelte:head>

<div class="max-w-4xl mx-auto px-6 py-8">
	<h1 class="text-2xl font-light text-rp-text mb-6">Statistics</h1>

	{#if loading}
		<div class="flex items-center justify-center py-20">
			<div class="w-5 h-5 border-2 border-rp-iris/30 border-t-rp-iris rounded-full animate-spin"></div>
		</div>
	{:else}
		<!-- Overview cards -->
		<div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-8">
			<StatsCard label="Total Scrobbles" value={stats.totalScrobbles} accent="foam" />
			<StatsCard label="Artists" value={stats.uniqueArtists} accent="iris" />
			<StatsCard label="Albums" value={stats.uniqueAlbums} accent="rose" />
			<StatsCard label="Tracks" value={stats.uniqueTracks} accent="gold" />
		</div>

		<div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-8">
			<StatsCard label="Listening Hours" value={stats.totalHours} accent="pine" />
			<StatsCard label="Avg Quality" value={stats.avgQuality} sub="out of 100" accent="iris" />
			<StatsCard label="Lossless" value="{stats.pctLossless}%" accent="foam" />
			<StatsCard label="DSD" value="{stats.pctDsd}%" accent="iris" />
		</div>

		<!-- Top artist / album -->
		<div class="grid grid-cols-2 gap-3 mb-8">
			<div class="rounded-lg bg-rp-surface border border-rp-hl-med p-4">
				<p class="text-xs text-rp-muted uppercase tracking-wider mb-1">Top Artist</p>
				<p class="text-lg text-rp-rose font-light truncate">{stats.topArtist}</p>
			</div>
			<div class="rounded-lg bg-rp-surface border border-rp-hl-med p-4">
				<p class="text-xs text-rp-muted uppercase tracking-wider mb-1">Top Album</p>
				<p class="text-lg text-rp-gold font-light truncate">{stats.topAlbum}</p>
			</div>
		</div>

		<!-- Listening heatmap -->
		<section>
			<h2 class="text-sm font-medium text-rp-subtle uppercase tracking-wider mb-3">Listening Heatmap</h2>
			<div class="rounded-xl bg-rp-surface border border-rp-hl-med p-4 overflow-x-auto">
				<div class="min-w-[600px]">
					<!-- Hour labels -->
					<div class="flex items-center mb-1 ml-10">
						{#each Array(24) as _, h}
							<span class="flex-1 text-center text-[9px] text-rp-muted">
								{h % 6 === 0 ? `${h}` : ''}
							</span>
						{/each}
					</div>
					<!-- Grid -->
					{#each heatmap as row, dayIdx}
						<div class="flex items-center gap-1 mb-0.5">
							<span class="w-8 text-[10px] text-rp-muted text-right mr-1">{dayLabels[dayIdx]}</span>
							{#each row as val}
								<div
									class="flex-1 h-3 rounded-sm {heatColor(val, heatMax)} transition-colors"
									title="{val} listens"
								></div>
							{/each}
						</div>
					{/each}
				</div>
			</div>
		</section>
	{/if}
</div>
