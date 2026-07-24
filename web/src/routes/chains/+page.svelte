<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { SignalChain, Equipment } from '$lib/api';
	import { formatHours } from '$lib/utils';

	let chains: SignalChain[] = [];
	let equipment: Equipment[] = [];
	let loading = true;
	let error = '';

	function ctxClass(ctx: string): string {
		const map: Record<string, string> = { active: 'tag-accent-2', passive: 'tag-accent' };
		return 'tag ' + (map[ctx] ?? 'tag-neutral');
	}

	onMount(async () => {
		try {
			const [c, g] = await Promise.all([api.getChains(), api.getEquipment()]);
			chains = c.chains;
			equipment = g.equipment;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
		}
		loading = false;
	});
</script>

<svelte:head><title>Signal Chains — Tapedeck</title></svelte:head>

<h1 class="h-display" style="font-size:38px;margin-bottom:6px;">Signal Chains</h1>
<p style="margin:0 0 24px;color:var(--td-muted);font-size:15px;">The path your music takes, from source to ears.</p>

{#if loading}
	<div style="display:flex;justify-content:center;padding:60px;"><div class="spin" style="width:22px;height:22px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
{:else if error}
	<div class="panel" style="border-radius:22px;padding:24px;text-align:center;color:#b2622d;">{error}</div>
{:else}
	<div style="display:flex;flex-direction:column;gap:16px;margin-bottom:34px;">
		{#if chains.length}
			{#each chains as c (c.id)}
				<div class="panel" style="border-radius:22px;padding:22px;">
					<div style="display:flex;align-items:flex-start;justify-content:space-between;gap:12px;margin-bottom:16px;">
						<div style="flex:1;min-width:0;">
							<div class="h-display" style="font-size:20px;line-height:1.15;">{c.name}</div>
							{#if c.description}<div style="font-size:13px;color:var(--td-muted);margin-top:4px;">{c.description}</div>{/if}
						</div>
						<span class={ctxClass(c.listening_context)} style="flex:none;">{c.listening_context}</span>
					</div>
					{#if c.components?.length}
						<div style="display:flex;align-items:center;gap:6px;flex-wrap:wrap;">
							{#each c.components as step, i}
								<span style="display:inline-flex;flex-direction:column;background:var(--td-surface2);border:1px solid var(--td-border);border-radius:12px;padding:7px 12px;">
									<span style="font-size:10px;letter-spacing:.08em;text-transform:uppercase;color:var(--td-muted-3);">{step.type}</span>
									<span style="font-size:13.5px;font-weight:600;">{step.name}</span>
								</span>
								{#if i < c.components.length - 1}<span style="color:var(--td-muted-3);">→</span>{/if}
							{/each}
						</div>
					{/if}
					<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:14px;">{formatHours(c.total_hours)} logged on this chain</div>
				</div>
			{/each}
		{:else}
			<div class="panel" style="border-radius:22px;padding:44px;text-align:center;color:var(--td-muted);">
				No signal chains yet. Create one via <code style="background:var(--td-sidebar);padding:2px 6px;border-radius:6px;font-size:12px;">POST /api/v1/chains</code>.
			</div>
		{/if}
	</div>

	<h3 class="h-display" style="font-size:19px;margin:0 0 12px;">Equipment</h3>
	<div class="panel" style="border-radius:22px;overflow:hidden;padding:6px 20px 14px;">
		{#if equipment.length}
			<table class="table">
				<thead><tr><th>Gear</th><th>Type</th><th>Brand</th><th style="text-align:right;">Hours</th></tr></thead>
				<tbody>
					{#each equipment as e (e.id)}
						<tr>
							<td style="font-weight:600;">{e.name}</td>
							<td style="color:var(--td-muted);text-transform:capitalize;">{e.equipment_type}</td>
							<td style="color:var(--td-muted);">{e.brand ?? '—'}</td>
							<td style="text-align:right;color:var(--td-good-fg);font-weight:600;">{formatHours(e.total_hours)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{:else}
			<p style="color:var(--td-muted);font-size:14px;padding:16px 0;margin:0;">No equipment tracked yet.</p>
		{/if}
	</div>
{/if}

<style>
	.spin { animation: td-spin 0.7s linear infinite; }
</style>
