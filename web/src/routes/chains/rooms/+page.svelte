<script lang="ts">
	// Rooms: where chains live. Make one, rename it, give it an icon, put chains
	// in it, change the order the page shows them in.
	//
	// A chain may be in several rooms, so "move chains in" adds the room to a
	// chain rather than taking it out of its others. Rooms never touch where a
	// play is attributed; they group chains on Signal Chains and nothing else.
	import { onMount } from 'svelte';
	import { api, type Room, type SignalChain } from '$lib/api';
	import { formatHours } from '$lib/utils';
	import RoomIcon, { ROOM_ICONS, ROOM_ICON_LABELS } from '$lib/components/chains/RoomIcon.svelte';
	import { roomGlyph } from '$lib/components/chains/layout';

	let rooms: Room[] = [];
	let chains: SignalChain[] = [];
	let loading = true;
	let error = '';

	async function load() {
		try {
			const [r, c] = await Promise.all([api.getRooms(), api.getChains()]);
			rooms = r.rooms;
			chains = c.chains;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not load your rooms';
		}
		loading = false;
	}
	onMount(load);

	$: chainById = new Map(chains.map((c) => [c.id, c]));
	$: mine = rooms.filter((r) => r.kind === 'room');
	function meta(r: Room): string {
		const cs = r.chain_ids.map((id) => chainById.get(id)).filter(Boolean) as SignalChain[];
		const gear = new Set(cs.flatMap((c) => (c.components ?? []).map((s) => s.equipment_id).filter((x) => x != null)));
		const hours = cs.reduce((a, c) => a + c.total_hours, 0);
		const bits = [
			`${cs.length} ${cs.length === 1 ? 'chain' : 'chains'}`,
			...(gear.size ? [`${gear.size} ${gear.size === 1 ? 'piece' : 'pieces'} of gear`] : []),
			hours > 0 ? formatHours(hours) : 'nothing logged yet'
		];
		return (r.kind === 'room' ? '' : 'Built in · ') + bits.join(' · ');
	}
	function roomsOf(chainId: number): string {
		const names = rooms.filter((r) => r.chain_ids.includes(chainId)).map((r) => r.name);
		return names.length ? `in ${names.join(', ')}` : 'in no room yet';
	}

	// ── Editing one room ──
	let editing: number | null = null;
	let eName = '';
	let eIcon: string | null = null;
	let eErr = '';
	let busy = false;
	function edit(r: Room) {
		editing = r.id;
		eName = r.name;
		eIcon = r.icon;
		eErr = '';
	}
	async function save(r: Room) {
		busy = true;
		eErr = '';
		try {
			await api.updateRoom(r.id, { ...(r.kind === 'room' && eName.trim() !== r.name ? { name: eName } : {}), icon: eIcon });
			editing = null;
			await load();
		} catch (e) {
			eErr = e instanceof Error ? e.message : 'Could not save';
		} finally {
			busy = false;
		}
	}
	async function move(r: Room, by: -1 | 1) {
		const order = [...rooms];
		const i = order.findIndex((x) => x.id === r.id);
		const j = i + by;
		if (j < 0 || j >= order.length) return;
		[order[i], order[j]] = [order[j], order[i]];
		busy = true;
		try {
			// Rewrite every position so ties left by older rows cannot reorder
			// anything behind the person's back.
			for (const [k, room] of order.entries()) if (room.sort !== k) await api.updateRoom(room.id, { sort: k });
			await load();
		} finally {
			busy = false;
		}
	}
	async function remove(r: Room) {
		if (!confirm(`Delete “${r.name}”? Its chains stay; they just leave this room.`)) return;
		busy = true;
		try {
			await api.deleteRoom(r.id);
			editing = null;
			await load();
		} finally {
			busy = false;
		}
	}

	// ── Adding one ──
	const SUGGESTIONS = ['Study', 'Living room', 'Bedroom', 'Kitchen', 'Office', 'Studio flat', 'Car', 'Garden'];
	const SUGGESTED_ICON: Record<string, string> = {
		Study: 'desk', 'Living room': 'sofa', Bedroom: 'bed', Kitchen: 'pot', Office: 'desk',
		'Studio flat': 'tv', Car: 'car', Garden: 'plant'
	};
	$: free = SUGGESTIONS.filter((n) => !rooms.some((r) => r.name.toLowerCase() === n.toLowerCase()));
	let nName = '';
	let nIcon: string | null = null;
	let nChains = new Set<number>();
	let nErr = '';
	function suggest(n: string) {
		nName = n;
		nIcon = SUGGESTED_ICON[n] ?? null;
	}
	function toggleChain(id: number) {
		nChains.has(id) ? nChains.delete(id) : nChains.add(id);
		nChains = nChains;
	}
	async function create() {
		busy = true;
		nErr = '';
		try {
			const { id } = await api.createRoom(nName, nIcon);
			for (const cid of nChains) {
				const now = rooms.filter((r) => r.chain_ids.includes(cid)).map((r) => r.id);
				await api.setChainRooms(cid, [...now, id]);
			}
			nName = '';
			nIcon = null;
			nChains = new Set();
			await load();
		} catch (e) {
			nErr = e instanceof Error && e.message.includes('409') ? 'You already have a room with that name.' : e instanceof Error ? e.message : 'Could not make the room';
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>Rooms — Tapedeck</title></svelte:head>

<a class="back" href="/chains">‹ Signal Chains</a>
<h1 class="h-display">Rooms</h1>
<p class="intro">
	Where your chains live. A room can hold several: a work table and a TV in one studio flat, or
	two chains in the car. A chain can be in more than one room. Pick a room on Signal Chains to see
	only what is in it. Rooms never change where a play lands.
</p>

{#if loading}
	<div class="panel note">Loading…</div>
{:else if error}
	<div class="panel note">{error}</div>
{:else}
	<div class="grid">
		{#each rooms as r, i (r.id)}
			<section class="panel rc" class:builtin={r.kind !== 'room'}>
				<div class="rtop">
					<span class="ri"><RoomIcon name={roomGlyph(r)} size={18} title="" /></span>
					<div class="rtext">
						<h2 class="h-display">{r.name}</h2>
						<div class="rmeta">{meta(r)}</div>
					</div>
					{#if editing !== r.id}<button class="btn btn-ghost" on:click={() => edit(r)}>Edit</button>{/if}
				</div>
				{#if editing === r.id}
					<div class="edit">
						{#if r.kind === 'room'}
							<label class="field"><span>Name</span><input class="input" bind:value={eName} maxlength="60" /></label>
						{/if}
						<div class="field">
							<span>Icon</span>
							<div class="icons">
								{#each ROOM_ICONS as ic}
									<button class="ico" class:on={eIcon === ic} aria-pressed={eIcon === ic} aria-label={ROOM_ICON_LABELS[ic]}
										on:click={() => (eIcon = eIcon === ic ? null : ic)}><RoomIcon name={ic} size={18} title="" /></button>
								{/each}
							</div>
						</div>
						{#if eErr}<p class="err">{eErr}</p>{/if}
						<div class="eacts">
							<button class="btn btn-secondary" disabled={busy || i === 0} on:click={() => move(r, -1)} aria-label="Move {r.name} earlier">↑</button>
							<button class="btn btn-secondary" disabled={busy || i === rooms.length - 1} on:click={() => move(r, 1)} aria-label="Move {r.name} later">↓</button>
							<span style="flex:1"></span>
							{#if r.kind === 'room'}<button class="btn btn-ghost del" disabled={busy} on:click={() => remove(r)}>Delete</button>{/if}
							<button class="btn btn-secondary" disabled={busy} on:click={() => (editing = null)}>Cancel</button>
							<button class="btn btn-primary" disabled={busy} on:click={() => save(r)}>Save</button>
						</div>
					</div>
				{/if}
				<div class="cchips">
					{#each r.chain_ids as cid}
						{#if chainById.get(cid)}<a class="cc" href="/chains?chain={cid}&room={r.id}">{chainById.get(cid)?.name}</a>{/if}
					{:else}
						<span class="none">No chains yet. Open a chain on Signal Chains to put it here.</span>
					{/each}
				</div>
				{#if r.kind === 'portable'}<p class="rnote">For chains that go with you. Not a room you can rename or delete.</p>{/if}
			</section>
		{/each}

		<section class="panel rc add">
			<div class="rtop">
				<span class="ri on"><RoomIcon name={nIcon} size={18} title="" /></span>
				<div class="rtext">
					<h2 class="h-display">Add a room</h2>
					<div class="rmeta">Pick one to start, or type your own</div>
				</div>
			</div>
			{#if free.length}
				<div class="sugs">
					{#each free as n}<button class="sug" class:on={nName === n} on:click={() => suggest(n)}>{n}</button>{/each}
				</div>
			{/if}
			<label class="field"><span>Name</span><input class="input" bind:value={nName} maxlength="60" placeholder={free[0] ?? "Name it"} /></label>
			<div class="field">
				<span>Icon</span>
				<div class="icons">
					{#each ROOM_ICONS as ic}
						<button class="ico" class:on={nIcon === ic} aria-pressed={nIcon === ic} aria-label={ROOM_ICON_LABELS[ic]}
							on:click={() => (nIcon = nIcon === ic ? null : ic)}><RoomIcon name={ic} size={18} title="" /></button>
					{/each}
				</div>
			</div>
			{#if chains.length}
				<div class="field">
					<span>Add chains · optional</span>
					<div class="moves">
						{#each chains as c (c.id)}
							<button class="mv" class:on={nChains.has(c.id)} aria-pressed={nChains.has(c.id)} on:click={() => toggleChain(c.id)}>
								<span class="box">{nChains.has(c.id) ? '✓' : ''}</span>
								<span class="mvt"><b>{c.name}</b><small>{roomsOf(c.id)}</small></span>
							</button>
						{/each}
					</div>
				</div>
			{/if}
			{#if nErr}<p class="err">{nErr}</p>{/if}
			<div class="eacts">
				<span style="flex:1"></span>
				<button class="btn btn-primary" disabled={busy || !nName.trim()} on:click={create}>
					Make {nName.trim() ? `“${nName.trim()}”` : 'the room'}
				</button>
			</div>
		</section>
	</div>

	<section class="panel moving">
		<div style="flex:1 1 420px;">
			<div style="font-weight:700;">Gear that moves between rooms</div>
			<p class="rnote" style="margin-top:2px;">
				Rooms hold chains, not gear, so a pair of headphones can be on the desk one evening and on
				the commute the next morning. On the graph it is drawn once, in the room where it gets the
				most hours, with wires reaching out to the others.
			</p>
		</div>
		{#if !mine.length}<span class="tag tag-neutral">No rooms of your own yet</span>{/if}
	</section>
{/if}

<style>
	.back {
		font-size: 13.5px;
		color: var(--td-accent);
	}
	h1 {
		font-size: 38px;
		margin: 4px 0 0;
	}
	.intro {
		margin: 6px 0 20px;
		max-width: 66ch;
		color: var(--td-muted);
		font-size: 14px;
	}
	.note {
		border-radius: 22px;
		padding: 24px;
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
		gap: 14px;
		margin-bottom: 16px;
	}
	.rc {
		border-radius: 22px;
		padding: 18px;
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.rc.builtin {
		border-style: dashed;
	}
	.rc.add {
		border: 1.5px dashed var(--td-accent);
		background: color-mix(in srgb, var(--td-accent) 6%, var(--td-surface));
	}
	.rtop {
		display: flex;
		gap: 12px;
		align-items: center;
	}
	.ri {
		flex: none;
		width: 40px;
		height: 40px;
		border-radius: 999px;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--td-surface2);
		color: var(--td-muted);
	}
	.ri.on {
		background: var(--td-accent);
		color: var(--td-on-accent);
	}
	.rtext {
		flex: 1;
		min-width: 0;
	}
	.rtext h2 {
		font-size: 21px;
	}
	.rmeta {
		font-size: 12.5px;
		color: var(--td-muted);
	}
	.cchips {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}
	.cc {
		font-size: 12.5px;
		padding: 3px 10px;
		border-radius: 999px;
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
		color: var(--td-text);
		text-decoration: none;
	}
	.cc:hover {
		border-color: var(--td-accent);
	}
	.none,
	.rnote {
		margin: 0;
		font-size: 12.5px;
		color: var(--td-muted);
		line-height: 1.45;
	}
	.edit {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 10px;
		border-radius: 14px;
		background: var(--td-bg);
		border: 1px solid var(--td-border);
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.field > span {
		font-size: 12px;
		font-weight: 600;
		color: var(--td-muted);
	}
	.icons {
		display: flex;
		gap: 6px;
		flex-wrap: wrap;
	}
	.ico {
		width: 38px;
		height: 38px;
		border-radius: 12px;
		border: 1px solid var(--td-border);
		background: var(--td-surface);
		color: var(--td-muted);
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.ico.on {
		background: var(--td-accent);
		border-color: var(--td-accent);
		color: var(--td-on-accent);
	}
	.eacts {
		display: flex;
		gap: 8px;
		align-items: center;
		flex-wrap: wrap;
	}
	.del {
		color: var(--td-bad-fg);
	}
	.err {
		margin: 0;
		font-size: 13px;
		color: var(--td-bad-fg);
	}
	.sugs {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}
	.sug {
		font: inherit;
		font-size: 12.5px;
		padding: 5px 12px;
		border-radius: 999px;
		border: 1px solid var(--td-border);
		background: var(--td-surface);
		color: var(--td-text);
		cursor: pointer;
	}
	.sug.on {
		background: var(--td-accent);
		border-color: var(--td-accent);
		color: var(--td-on-accent);
	}
	.moves {
		display: flex;
		flex-direction: column;
		gap: 6px;
		max-height: 260px;
		overflow-y: auto;
	}
	.mv {
		display: flex;
		align-items: center;
		gap: 10px;
		text-align: left;
		padding: 8px 10px;
		border-radius: 12px;
		border: 1px solid var(--td-border);
		background: var(--td-surface);
		font: inherit;
		color: inherit;
		cursor: pointer;
	}
	.mv.on {
		border-color: var(--td-accent);
	}
	.box {
		flex: none;
		width: 20px;
		height: 20px;
		border-radius: 6px;
		border: 1.5px solid var(--td-muted-3);
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: 12px;
		color: var(--td-on-accent);
	}
	.mv.on .box {
		background: var(--td-accent);
		border-color: var(--td-accent);
	}
	.mvt {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
	.mvt small {
		font-size: 12px;
		color: var(--td-muted);
	}
	.moving {
		border-radius: 20px;
		padding: 16px 20px;
		display: flex;
		gap: 16px;
		align-items: center;
		flex-wrap: wrap;
	}
</style>
