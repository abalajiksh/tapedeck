<script lang="ts">
	// The chain list beside the graph: the primary way to navigate the page.
	//
	// Choosing a row lights that chain's route on the graph; the drawer opens
	// from the bar under the graph, so scanning the list never pops a panel
	// over what you were looking at. Each row is one button, so the list works
	// with a keyboard however the graph is drawn.
	import { createEventDispatcher } from 'svelte';
	import type { Equipment, Room, SignalChain } from '$lib/api';
	import ChainIcon from '$lib/components/ChainIcon.svelte';
	import { lastUsedLabel, slotPositions } from './format';

	export let chains: SignalChain[] = [];
	export let equipment: Equipment[] = [];
	/** chain id → (step position → gear on now), for chains with slots. */
	export let onNow: Record<string, Record<string, number>> = {};
	export let lastUsed: Record<string, number> = {};
	export let selectedId: number | null = null;
	export let rooms: Room[] = [];
	/** The room filter: only that room's chains are listed. */
	export let roomId: number | null = null;

	const dispatch = createEventDispatcher<{ select: number }>();

	// Grouped by room in the rooms' own order, once any chain has a room. A
	// chain in two rooms is listed under both, because that is where it is;
	// chains with no room come last, under "No room yet".
	$: roomed = new Set(rooms.flatMap((r) => r.chain_ids));
	$: groups =
		roomed.size === 0
			? [{ key: 'all', title: null as string | null, list: chains }]
			: [
					...rooms
						.filter((r) => (roomId == null ? r.chain_ids.length > 0 : r.id === roomId))
						.map((r) => ({
							key: `r${r.id}`,
							title: r.name as string | null,
							list: chains.filter((c) => r.chain_ids.includes(c.id))
						})),
					...(roomId == null
						? [{ key: 'none', title: 'No room yet' as string | null, list: chains.filter((c) => !roomed.has(c.id)) }]
						: [])
				].filter((g) => g.list.length || roomId != null);
</script>

<nav class="clist" aria-label="Your chains">
	{#each groups as g (g.key)}
	{#if g.title}<div class="rhead">{g.title}<span class="n">· {g.list.length}</span></div>{/if}
	{#each g.list as c (`${g.key}:${c.id}`)}
		{@const slots = slotPositions(c)}
		{@const steps = (c.components ?? []).filter((s) => s.name.trim())}
		<button
			class="place"
			class:on={selectedId === c.id}
			aria-pressed={selectedId === c.id}
			on:click={() => dispatch('select', c.id)}
		>
			<span class="pi" aria-hidden="true"><ChainIcon name={c.icon} size={17} title="" /></span>
			<span class="pbody">
				<span class="ptop">
					<span class="pname">{c.name}</span>
					{#if c.shared}<span class="tag tag-accent-2" title="Patched decks see these plays and their format; never the chain">Shared</span>{/if}
				</span>
				<span class="ppath">
					{#each steps as s, i}
						{#if i}<span class="ar">→</span>{/if}
						{@const at = (c.components ?? []).indexOf(s)}
						{#if slots.has(at)}<span class="swap" title="On now; you swap gear at this step"
								>{equipment.find((e) => e.id === onNow[String(c.id)]?.[String(at)])?.name ?? s.name} ▾</span
							>{:else}{s.name}{/if}
					{:else}
						<span class="none">No steps yet</span>
					{/each}
				</span>
				<span class="pwhen">{lastUsedLabel(lastUsed[String(c.id)])}</span>
			</span>
		</button>
	{:else}
		<p class="empty">No chains in this room yet. Open a chain and add it to the room.</p>
	{/each}
	{/each}
</nav>

<style>
	.clist {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.rhead {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 12px 12px 4px;
		font-size: 11px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		font-weight: 700;
		color: var(--td-muted-2);
	}
	.rhead .n {
		letter-spacing: 0;
		text-transform: none;
		font-weight: 600;
		color: var(--td-muted-3);
	}
	.empty {
		margin: 0;
		padding: 8px 12px;
		font-size: 13px;
		color: var(--td-muted);
	}
	.place {
		display: flex;
		gap: 12px;
		align-items: flex-start;
		width: 100%;
		text-align: left;
		padding: 11px 12px;
		border-radius: 16px;
		border: 1.5px solid transparent;
		background: none;
		font: inherit;
		color: var(--td-text);
		cursor: pointer;
	}
	.place:hover {
		background: var(--td-surface);
	}
	.place.on {
		background: var(--td-surface);
		border-color: var(--td-accent);
	}
	.place:focus-visible {
		outline: 2px solid var(--td-accent);
		outline-offset: 2px;
	}
	.pi {
		flex: none;
		width: 34px;
		height: 34px;
		border-radius: 999px;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--td-surface2);
		color: var(--td-muted);
	}
	.place.on .pi {
		background: var(--td-accent);
		color: var(--td-on-accent);
	}
	.pbody {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.ptop {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.pname {
		font-weight: 600;
		font-size: 14px;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.ppath {
		font-size: 12.5px;
		color: var(--td-muted);
		line-height: 1.45;
	}
	.ar {
		color: var(--td-muted-3);
		margin: 0 4px;
	}
	.swap {
		padding: 0 6px;
		border-radius: 999px;
		border: 1px dashed var(--td-muted-3);
	}
	.none {
		font-style: italic;
		color: var(--td-muted-3);
	}
	.pwhen {
		font-size: 12px;
		color: var(--td-muted-3);
	}
</style>
