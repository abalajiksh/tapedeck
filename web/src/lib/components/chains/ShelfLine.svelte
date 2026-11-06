<script lang="ts">
	// Gear no chain or setup names, as one line you can open.
	//
	// Everything wired is already on the graph with its hours, so the only gear
	// left to show is what sits on the shelf: new, spare, or lent out. One
	// collapsed line by default, because on most visits it is not what you came
	// for. Clicking a card opens the gear editor the page already has.
	import { createEventDispatcher } from 'svelte';
	import type { Equipment } from '$lib/api';

	export let equipment: Equipment[] = [];
	/** Gear a chain or a setup names: everything not on the shelf. */
	export let inUse: Set<number> = new Set();
	/** Gear a chain step names, which is what the graph draws as a box. */
	export let onGraph: Set<number> = new Set();

	const dispatch = createEventDispatcher<{ edit: Equipment }>();
	let open = false;

	const STATUS: Record<string, { label: string; cls: string }> = {
		new: { label: 'New', cls: 'tag-accent-2' },
		spare: { label: 'Spare', cls: 'tag-neutral' },
		lent: { label: 'Lent out', cls: 'tag-accent' }
	};

	$: shelf = equipment.filter((e) => !inUse.has(e.id)).sort((a, b) => a.name.localeCompare(b.name));
	$: lent = shelf.filter((e) => e.status === 'lent').length;
	$: drawn = equipment.filter((e) => onGraph.has(e.id)).length;
	// In use but not a box of its own: gear only a setup swaps in.
	$: swappedIn = equipment.length - shelf.length - drawn;
</script>

<section class="panel shelf" aria-label="Gear on the shelf">
	<div class="shead">
		<div class="stext">
			<span class="stitle">Gear</span>
			<span class="smeta">
				· {equipment.length} {equipment.length === 1 ? 'piece' : 'pieces'} · {drawn} on the graph above{swappedIn
					? ` · ${swappedIn} swapped in by a setup`
					: ''} · {shelf.length} on the shelf{lent ? `, ${lent} of them lent out` : ''}
			</span>
		</div>
		{#if shelf.length}
			<button class="btn btn-ghost" aria-expanded={open} on:click={() => (open = !open)}>
				{open ? 'Hide the shelf' : 'Show the shelf'}
			</button>
		{/if}
	</div>
	{#if open}
		<div class="sgrid">
			{#each shelf as e (e.id)}
				<button class="scard" on:click={() => dispatch('edit', e)} title="Edit {e.name}">
					<span class="sbody">
						<span class="sname">{e.name}</span>
						<span class="ssub">{[e.brand, e.equipment_type].filter(Boolean).join(' · ')}</span>
					</span>
					{#if e.status && STATUS[e.status]}<span class="tag {STATUS[e.status].cls}">{STATUS[e.status].label}</span>{/if}
				</button>
			{/each}
		</div>
		<p class="snote">Everything else is on the graph. Open a chain for the hours on its gear.</p>
	{/if}
</section>

<style>
	.shelf {
		border-radius: 20px;
		padding: 14px 18px 14px 22px;
	}
	.shead {
		display: flex;
		align-items: center;
		gap: 12px;
		flex-wrap: wrap;
	}
	.stext {
		flex: 1 1 360px;
		min-width: 0;
	}
	.stitle {
		font-weight: 700;
	}
	.smeta {
		font-size: 13.5px;
		color: var(--td-muted);
	}
	.sgrid {
		margin-top: 12px;
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
		gap: 8px;
	}
	.scard {
		display: flex;
		align-items: center;
		gap: 10px;
		text-align: left;
		padding: 10px 14px;
		border-radius: 14px;
		border: 1px solid var(--td-border);
		background: var(--td-surface2);
		font: inherit;
		color: inherit;
		cursor: pointer;
	}
	.scard:hover {
		border-color: var(--td-accent);
	}
	.sbody {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
	}
	.sname {
		font-weight: 600;
		font-size: 13.5px;
	}
	.ssub {
		font-size: 12px;
		color: var(--td-muted);
	}
	.snote {
		margin: 10px 0 0;
		font-size: 12.5px;
		color: var(--td-muted);
	}
</style>
