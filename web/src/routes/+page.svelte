<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { Scrobble } from '$lib/api';
	import NowPlaying from '$lib/components/NowPlaying.svelte';
	import ScrobbleRow from '$lib/components/ScrobbleRow.svelte';
	import StatsCard from '$lib/components/StatsCard.svelte';

	let nowPlaying: Scrobble | null = null;
	let recentScrobbles: Scrobble[] = [];
	let loading = true;
	let error = '';
	let connected = false;

	// Quick stats (will come from a stats endpoint later)
	let todayCount = 0;
	let weekCount = 0;
	let losslessPct = 0;

	onMount(async () => {
		try {
			const health = await api.health();
			connected = health.status === 'ok';
		} catch {
			connected = false;
			error = 'Cannot reach Tapedeck backend. Is it running?';
		}
		loading = false;

		// TODO: Wire up real endpoints once /api/v1/scrobbles exists
		// For now, the page renders the empty/disconnected states.
	});
</script>

<svelte:head>
	<title>Dashboard — Tapedeck</title>
</svelte:head>

<div class="max-w-4xl mx-auto px-6 py-8">
	<!-- Header -->
	<div class="flex items-end justify-between mb-6">
		<div>
			<h1 class="text-2xl font-light text-rp-text">Dashboard</h1>
			<p class="text-sm text-rp-muted mt-1">Your listening at a glance</p>
		</div>
		<div class="flex items-center gap-2">
			<span class="w-2 h-2 rounded-full {connected ? 'bg-rp-foam' : 'bg-rp-love'}"></span>
			<span class="text-xs text-rp-muted">{connected ? 'Connected' : 'Disconnected'}</span>
		</div>
	</div>

	{#if loading}
		<div class="flex items-center justify-center py-20">
			<div class="w-5 h-5 border-2 border-rp-iris/30 border-t-rp-iris rounded-full animate-spin"></div>
		</div>
	{:else if error}
		<div class="rounded-xl bg-rp-surface border border-rp-love/30 p-6 text-center">
			<p class="text-rp-love text-sm mb-2">Connection Error</p>
			<p class="text-rp-muted text-xs">{error}</p>
			<p class="text-rp-subtle text-xs mt-3">
				Make sure <code class="text-rp-foam bg-rp-hl-low px-1.5 py-0.5 rounded">./tapedeck</code> is running
				and configure your token in <a href="/settings" class="text-rp-iris hover:underline">Settings</a>.
			</p>
		</div>
	{:else}
		<!-- Now Playing -->
		<div class="mb-6">
			<NowPlaying scrobble={nowPlaying} />
		</div>

		<!-- Quick stats -->
		<div class="grid grid-cols-3 gap-3 mb-8">
			<StatsCard label="Today" value={todayCount} accent="foam" />
			<StatsCard label="This Week" value={weekCount} accent="iris" />
			<StatsCard label="Lossless" value="{losslessPct}%" sub="of all listens" accent="pine" />
		</div>

		<!-- Recent scrobbles -->
		<section>
			<div class="flex items-center justify-between mb-3">
				<h2 class="text-sm font-medium text-rp-subtle uppercase tracking-wider">Recent Listens</h2>
				<a href="/history" class="text-xs text-rp-iris hover:underline">View all →</a>
			</div>

			{#if recentScrobbles.length > 0}
				<div class="rounded-xl bg-rp-surface border border-rp-hl-med divide-y divide-rp-hl-low">
					{#each recentScrobbles as scrobble}
						<ScrobbleRow {scrobble} />
					{/each}
				</div>
			{:else}
				<div class="rounded-xl bg-rp-surface border border-rp-hl-med p-8 text-center">
					<p class="text-rp-muted text-sm">No scrobbles yet</p>
					<p class="text-rp-subtle text-xs mt-2">
						Point <a href="https://github.com/kawaiiDango/pano-scrobbler" target="_blank" rel="noopener" class="text-rp-iris hover:underline">Pano Scrobbler</a>
						at your Tapedeck instance to start tracking.
					</p>
				</div>
			{/if}
		</section>
	{/if}
</div>
