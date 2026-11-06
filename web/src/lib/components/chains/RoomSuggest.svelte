<script lang="ts">
	// "Group your chains by room?" — asked once, when chains exist and none has
	// a room yet. Never assigns silently: it proposes, the person names and
	// accepts (Ashwin, 2026-11-06: let the inbox suggest rather than hard-code
	// rooms). Until the inbox exists (phase 2) the question sits on the page;
	// "Not now" is remembered in this browser.
	//
	// The proposal: chains played from a phone or a music player go On the go;
	// the rest are grouped by the gear they share past the source (a DAC, an
	// amp, a pair of speakers), because two chains through the same speakers
	// are in the same room.
	import { createEventDispatcher, onMount } from 'svelte';
	import { api, type Equipment, type Room, type SignalChain } from '$lib/api';
	import { columnOf, stepKey } from './layout';

	export let chains: SignalChain[] = [];
	export let equipment: Equipment[] = [];
	export let rooms: Room[] = [];

	const dispatch = createEventDispatcher<{ done: void }>();
	const KEY = 'td-rooms-suggestion';
	let dismissed = true;
	let busy = false;
	let error = '';
	onMount(() => {
		dismissed = localStorage.getItem(KEY) === 'dismissed';
	});

	const PORTABLE_GEAR = new Set(['phone', 'dap']);
	const GUESS: Record<string, string> = { desk: 'Study', laptop: 'Study', tv: 'Living room', radio: 'Kitchen', car: 'Car' };

	type Group = { key: string; portable: boolean; chains: SignalChain[]; shared: string[]; name: string };

	function propose(cs: SignalChain[], gear: Equipment[]): Group[] {
		const typeOf = new Map(gear.map((e) => [e.id, e.equipment_type]));
		const portable = (c: SignalChain) =>
			c.listening_context === 'active-mobile' ||
			(c.components ?? []).some(
				(s) => columnOf(s.type) === 0 && s.equipment_id != null && PORTABLE_GEAR.has(typeOf.get(s.equipment_id) ?? '')
			);
		const go = cs.filter(portable);
		const rest = cs.filter((c) => !portable(c));
		// Union chains that share any box past the source column.
		const parent = new Map(rest.map((c) => [c.id, c.id]));
		const find = (x: number): number => (parent.get(x) === x ? x : find(parent.get(x)!));
		const owner = new Map<string, number>();
		const named = new Map<string, string>();
		for (const c of rest)
			for (const s of c.components ?? []) {
				if (!s.name.trim() || columnOf(s.type) === 0) continue;
				const k = stepKey(s);
				named.set(k, s.name);
				const o = owner.get(k);
				if (o == null) owner.set(k, c.id);
				else parent.set(find(c.id), find(o));
			}
		const byRoot = new Map<number, SignalChain[]>();
		for (const c of rest) byRoot.set(find(c.id), [...(byRoot.get(find(c.id)) ?? []), c]);
		const used = new Set<string>();
		const groups: Group[] = [...byRoot.values()]
			.sort((a, b) => b.reduce((t, c) => t + c.total_hours, 0) - a.reduce((t, c) => t + c.total_hours, 0))
			.map((list, i) => {
				const counts = new Map<string, number>();
				for (const c of list)
					for (const k of new Set((c.components ?? []).filter((s) => s.name.trim() && columnOf(s.type) > 0).map(stepKey)))
						counts.set(k, (counts.get(k) ?? 0) + 1);
				const shared = [...counts.entries()].filter(([, n]) => n > 1).map(([k]) => named.get(k) ?? k);
				const guess = list.map((c) => GUESS[c.icon ?? '']).find(Boolean) ?? '';
				const name = guess && !used.has(guess) ? guess : '';
				if (name) used.add(name);
				return { key: `g${i}`, portable: false, chains: list, shared, name };
			});
		if (go.length) groups.push({ key: 'go', portable: true, chains: go, shared: [], name: 'On the go' });
		return groups;
	}

	$: show = !dismissed && chains.length >= 2 && rooms.every((r) => r.chain_ids.length === 0);
	let groups: Group[] = [];
	let proposedFor = '';
	$: {
		const sig = chains.map((c) => c.id).join(',');
		if (show && sig !== proposedFor) {
			proposedFor = sig;
			groups = propose(chains, equipment);
		}
	}
	$: ready = groups.some((g) => g.name.trim());

	function notNow() {
		localStorage.setItem(KEY, 'dismissed');
		dismissed = true;
	}

	async function accept() {
		busy = true;
		error = '';
		try {
			let current = (await api.getRooms()).rooms;
			for (const g of groups) {
				const name = g.name.trim();
				if (!name) continue;
				let room = g.portable
					? current.find((r) => r.kind === 'portable')
					: current.find((r) => r.name.toLowerCase() === name.toLowerCase());
				if (!room) {
					await api.createRoom(name, null);
					current = (await api.getRooms()).rooms;
					room = current.find((r) => r.name.toLowerCase() === name.toLowerCase());
				}
				if (!room) continue;
				for (const c of g.chains) await api.setChainRooms(c.id, [room.id]);
			}
			localStorage.setItem(KEY, 'dismissed');
			dismissed = true;
			dispatch('done');
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not make those rooms';
		} finally {
			busy = false;
		}
	}
</script>

{#if show && groups.length}
	<section class="ask" aria-label="A question from Tapedeck">
		<div class="ahead">
			<div>
				<div class="atitle">Group your chains by room?</div>
				<p class="abody">
					Rooms sort this page and the graph; they never change where a play lands. Here is a
					first guess from the gear your chains share. Name the ones you want, leave the rest blank.
				</p>
			</div>
		</div>
		<div class="groups">
			{#each groups as g (g.key)}
				<div class="group">
					{#if g.portable}
						<div class="gname builtin">On the go <span>built in</span></div>
					{:else}
						<input class="input gname-in" bind:value={g.name} placeholder="Name this room" aria-label="Name for the room holding {g.chains.map((c) => c.name).join(', ')}" />
					{/if}
					<div class="gchains">{g.chains.map((c) => c.name).join(' · ')}</div>
					{#if g.shared.length}<div class="gwhy">Shares {g.shared.slice(0, 3).join(', ')}</div>
					{:else if g.portable}<div class="gwhy">Played from a phone or a music player</div>{/if}
				</div>
			{/each}
		</div>
		{#if error}<p class="aerr">{error}</p>{/if}
		<div class="aacts">
			<button class="btn btn-primary" disabled={!ready || busy} on:click={accept}>
				{busy ? 'Making rooms…' : 'Make these rooms'}
			</button>
			<button class="btn btn-secondary" disabled={busy} on:click={notNow}>Not now</button>
			<a class="btn btn-ghost" href="/chains/rooms">Set them up myself</a>
		</div>
	</section>
{/if}

<style>
	.ask {
		border-radius: 20px;
		padding: 16px 18px;
		margin-bottom: 14px;
		background: color-mix(in srgb, var(--td-accent) 8%, var(--td-surface));
		border: 1px solid color-mix(in srgb, var(--td-accent) 35%, transparent);
	}
	.atitle {
		font-weight: 700;
		font-size: 15px;
	}
	.abody {
		margin: 2px 0 0;
		font-size: 13px;
		color: var(--td-muted);
		max-width: 70ch;
	}
	.groups {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(230px, 1fr));
		gap: 8px;
		margin-top: 12px;
	}
	.group {
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 14px;
		padding: 10px 12px;
		display: flex;
		flex-direction: column;
		gap: 5px;
	}
	.gname-in {
		font-weight: 600;
	}
	.gname {
		font-weight: 700;
		font-size: 14px;
		min-height: 36px;
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.gname span {
		font-size: 11px;
		font-weight: 600;
		color: var(--td-muted-2);
	}
	.gchains {
		font-size: 13px;
	}
	.gwhy {
		font-size: 12px;
		color: var(--td-muted-2);
	}
	.aerr {
		margin: 10px 0 0;
		font-size: 13px;
		color: var(--td-bad-fg);
	}
	.aacts {
		display: flex;
		gap: 8px;
		flex-wrap: wrap;
		margin-top: 12px;
	}
</style>
