<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { Scrobble } from '$lib/api';
	import ScrobbleRow from '$lib/components/ScrobbleRow.svelte';

	const PAGE_SIZE = 50;

	let scrobbles: Scrobble[] = [];
	let loading = true;
	let loadingMore = false;
	let error = '';
	let exhausted = false;
	let artistFilter = '';

	async function load(reset: boolean) {
		if (reset) {
			loading = true;
			exhausted = false;
		} else {
			loadingMore = true;
		}
		error = '';

		try {
			const res = await api.getScrobbles({
				limit: PAGE_SIZE,
				offset: reset ? 0 : scrobbles.length,
				artist: artistFilter.trim() || undefined,
			});
			scrobbles = reset ? res.scrobbles : [...scrobbles, ...res.scrobbles];
			exhausted = res.scrobbles.length < PAGE_SIZE;
		} catch (e: any) {
			error = e.message ?? 'Failed to load listening history';
		}

		loading = false;
		loadingMore = false;
	}

	let searchTimer: ReturnType<typeof setTimeout>;
	function onFilterInput() {
		clearTimeout(searchTimer);
		searchTimer = setTimeout(() => load(true), 300);
	}

	onMount(() => load(true));
</script>

<svelte:head>
	<title>History — Tapedeck</title>
</svelte:head>

<div class="max-w-4xl mx-auto px-6 py-8">
	<div class="flex items-end justify-between mb-6 gap-4">
		<div>
			<h1 class="text-2xl font-light text-rp-text">History</h1>
			<p class="text-sm text-rp-muted mt-1">Everything you've listened to</p>
		</div>
		<input
			type="search"
			bind:value={artistFilter}
			on:input={onFilterInput}
			placeholder="Filter by artist…"
			class="w-52 bg-rp-base border border-rp-hl-med rounded-lg px-3 py-2 text-sm text-rp-text
			       placeholder:text-rp-muted focus:outline-none focus:border-rp-iris/50 transition-colors"
		/>
	</div>

	{#if loading}
		<div class="flex items-center justify-center py-20">
			<div class="w-5 h-5 border-2 border-rp-iris/30 border-t-rp-iris rounded-full animate-spin"></div>
		</div>
	{:else if error}
		<div class="rounded-xl bg-rp-surface border border-rp-love/30 p-6 text-center">
			<p class="text-rp-love text-sm">{error}</p>
		</div>
	{:else if scrobbles.length === 0}
		<div class="rounded-xl bg-rp-surface border border-rp-hl-med p-8 text-center">
			<p class="text-rp-muted text-sm">
				{artistFilter.trim() ? `No listens matching “${artistFilter.trim()}”` : 'No scrobbles yet'}
			</p>
		</div>
	{:else}
		<div class="rounded-xl bg-rp-surface border border-rp-hl-med divide-y divide-rp-hl-low">
			{#each scrobbles as scrobble (scrobble.id)}
				<ScrobbleRow {scrobble} />
			{/each}
		</div>

		{#if !exhausted}
			<div class="flex justify-center mt-4">
				<button
					on:click={() => load(false)}
					disabled={loadingMore}
					class="px-4 py-2 rounded-lg text-sm font-medium transition-colors
					       bg-rp-iris/15 text-rp-iris border border-rp-iris/25
					       hover:bg-rp-iris/25 disabled:opacity-40 disabled:cursor-not-allowed"
				>
					{loadingMore ? 'Loading…' : 'Load more'}
				</button>
			</div>
		{/if}
	{/if}
</div>
