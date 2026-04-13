<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { SignalChain, Equipment } from '$lib/api';
	import ChainCard from '$lib/components/ChainCard.svelte';
	import { formatHours } from '$lib/utils';

	let chains: SignalChain[] = [];
	let equipment: Equipment[] = [];
	let loading = true;
	let error = '';

	onMount(async () => {
		try {
			const [chainsRes, gearRes] = await Promise.all([
				api.getChains(),
				api.getEquipment(),
			]);
			chains = chainsRes.chains;
			equipment = gearRes.equipment;
		} catch (e: any) {
			error = e.message ?? 'Failed to load data';
		}
		loading = false;
	});
</script>

<svelte:head>
	<title>Signal Chains — Tapedeck</title>
</svelte:head>

<div class="max-w-4xl mx-auto px-6 py-8">
	<div class="flex items-end justify-between mb-6">
		<div>
			<h1 class="text-2xl font-light text-rp-text">Signal Chains</h1>
			<p class="text-sm text-rp-muted mt-1">Your audio signal paths from source to ears</p>
		</div>
	</div>

	{#if loading}
		<div class="flex items-center justify-center py-20">
			<div class="w-5 h-5 border-2 border-rp-iris/30 border-t-rp-iris rounded-full animate-spin"></div>
		</div>
	{:else if error}
		<div class="rounded-xl bg-rp-surface border border-rp-love/30 p-6 text-center">
			<p class="text-rp-love text-sm">{error}</p>
		</div>
	{:else}
		<!-- Chains -->
		{#if chains.length > 0}
			<div class="grid gap-3 mb-10">
				{#each chains as chain (chain.id)}
					<ChainCard {chain} />
				{/each}
			</div>
		{:else}
			<div class="rounded-xl bg-rp-surface border border-rp-hl-med p-8 text-center mb-10">
				<p class="text-rp-muted text-sm">No signal chains defined</p>
				<p class="text-rp-subtle text-xs mt-2">
					Create chains via the API:
					<code class="text-rp-foam bg-rp-hl-low px-1.5 py-0.5 rounded ml-1">POST /api/v1/chains</code>
				</p>
			</div>
		{/if}

		<!-- Equipment tracker -->
		<section>
			<h2 class="text-sm font-medium text-rp-subtle uppercase tracking-wider mb-3">Equipment Usage</h2>

			{#if equipment.length > 0}
				<div class="rounded-xl bg-rp-surface border border-rp-hl-med overflow-hidden">
					<table class="w-full text-sm">
						<thead>
							<tr class="border-b border-rp-hl-med text-rp-muted text-xs uppercase tracking-wider">
								<th class="text-left px-4 py-2.5 font-medium">Equipment</th>
								<th class="text-left px-4 py-2.5 font-medium">Type</th>
								<th class="text-right px-4 py-2.5 font-medium">Hours</th>
							</tr>
						</thead>
						<tbody class="divide-y divide-rp-hl-low">
							{#each equipment as gear (gear.id)}
								<tr class="hover:bg-rp-hl-low transition-colors">
									<td class="px-4 py-2.5">
										<span class="text-rp-text">{gear.name}</span>
										{#if gear.brand}
											<span class="text-rp-muted text-xs ml-1">({gear.brand})</span>
										{/if}
									</td>
									<td class="px-4 py-2.5 text-rp-subtle capitalize">{gear.equipment_type}</td>
									<td class="px-4 py-2.5 text-right text-rp-foam">{formatHours(gear.total_hours)}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{:else}
				<div class="rounded-xl bg-rp-surface border border-rp-hl-med p-8 text-center">
					<p class="text-rp-muted text-sm">No equipment tracked yet</p>
					<p class="text-rp-subtle text-xs mt-2">
						Equipment usage is tracked automatically when scrobbles include chain data.
					</p>
				</div>
			{/if}
		</section>
	{/if}
</div>
