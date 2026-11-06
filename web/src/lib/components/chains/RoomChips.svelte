<script lang="ts">
	// The room filter on Signal Chains: "All rooms" and one chip per room. It
	// scrolls sideways on a phone rather than wrapping into a wall of chips.
	import { createEventDispatcher } from 'svelte';
	import type { Room } from '$lib/api';
	import RoomIcon from './RoomIcon.svelte';
	import { roomGlyph } from './layout';

	export let rooms: Room[] = [];
	export let chainCount = 0;
	export let roomId: number | null = null;

	const dispatch = createEventDispatcher<{ pick: number | null }>();
</script>

<div class="rooms" role="group" aria-label="Filter by room">
	<span class="kicker">Rooms</span>
	<button class="room" class:on={roomId == null} aria-pressed={roomId == null} on:click={() => dispatch('pick', null)}>
		All rooms<span class="c">{chainCount}</span>
	</button>
	{#each rooms as r (r.id)}
		<button class="room" class:on={roomId === r.id} aria-pressed={roomId === r.id} on:click={() => dispatch('pick', r.id)}>
			<RoomIcon name={roomGlyph(r)} size={14} title="" />{r.name}<span class="c">{r.chain_ids.length}</span>
		</button>
	{/each}
	<a class="btn btn-ghost manage" href="/chains/rooms">+ Room · Manage</a>
</div>

<style>
	.rooms {
		display: flex;
		gap: 6px;
		align-items: center;
		overflow-x: auto;
		padding: 2px 0 6px;
		margin-bottom: 12px;
		scrollbar-width: thin;
	}
	.kicker {
		font-size: 11px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		font-weight: 700;
		color: var(--td-muted-2);
		margin-right: 6px;
		flex: none;
	}
	.room {
		flex: none;
		display: inline-flex;
		align-items: center;
		gap: 7px;
		min-height: 38px;
		padding: 0 14px;
		border-radius: 999px;
		border: 1px solid var(--td-border);
		background: var(--td-surface);
		color: var(--td-text);
		font: inherit;
		font-size: 13.5px;
		font-weight: 600;
		cursor: pointer;
		white-space: nowrap;
	}
	.room:hover {
		border-color: var(--td-accent);
	}
	.room.on {
		background: var(--td-accent);
		border-color: var(--td-accent);
		color: var(--td-on-accent);
	}
	.c {
		font-size: 11.5px;
		color: var(--td-muted);
		font-weight: 600;
	}
	.room.on .c {
		color: inherit;
		opacity: 0.85;
	}
	.manage {
		flex: none;
		margin-left: 4px;
		white-space: nowrap;
	}
</style>
