<script lang="ts">
	// A new chain in three questions: what you hear it on, what plays into
	// it, and what to call it. Picking several things to hear it on makes one
	// chain with a "what's on now" switch, not a chain per pair.
	//
	// Nothing is created until the last step. How plays find it is either
	// left to the inbox (the first play through a new output becomes a
	// question) or made the fallback for every device that reports as the box
	// it plays from.
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { api, type ChainComponent, type Device, type Equipment, type Room } from '$lib/api';
	import RoomIcon from '$lib/components/chains/RoomIcon.svelte';
	import { roomGlyph } from '$lib/components/chains/layout';
	import {
		CHAIN_ICON_FOR_SOURCE,
		NEW_GEAR_TYPES,
		groupOf,
		roleOfGear,
		type GearGroup
	} from '$lib/components/chains/gear';

	let equipment: Equipment[] = [];
	let devices: Device[] = [];
	let rooms: Room[] = [];
	let loading = true;
	let error = '';

	let step = 1;
	let ears: number[] = [];
	let src: number | null = null;
	let mids: number[] = [];
	let name = '';
	let named = false;
	let roomIds: number[] = [];
	let roomsTouched = false;
	let find: 'ask' | 'device' = 'ask';
	let creating = false;

	// Where the first-run screen sent us from: a kind of start, which only
	// changes what is offered first, or a box something already reports as.
	type Start = 'desk' | 'go' | 'records' | 'blank';
	let start: Start | null = null;
	const PREFER: Record<Start, string[]> = {
		desk: ['computer', 'speakers', 'headphones', 'dac', 'dac-amp', 'amp'],
		go: ['phone', 'dap', 'headphones', 'iem', 'earbuds'],
		records: ['turntable', 'phono', 'speakers', 'amp'],
		blank: []
	};
	$: prefer = (t: string) => (start ? PREFER[start].indexOf(t) : -1);

	onMount(async () => {
		try {
			const [g, d, r] = await Promise.all([api.getEquipment(), api.getDevices(), api.getRooms()]);
			equipment = g.equipment;
			devices = d.devices;
			rooms = r.rooms;
			const q = new URLSearchParams(window.location.search);
			const s = q.get('start');
			if (s === 'desk' || s === 'go' || s === 'records' || s === 'blank') start = s;
			const from = Number(q.get('src'));
			if (from && equipment.some((e) => e.id === from)) {
				src = from;
				find = 'device';
			}
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not load your gear';
		}
		loading = false;
	});

	const gear = (id: number | null) => equipment.find((e) => e.id === id);
	const sub = (g: Equipment) => [g.brand, g.model].filter(Boolean).join(' ') || g.equipment_type;
	// What the start suggests first, then by name.
	$: byGroup = (g: GearGroup) =>
		equipment
			.filter((e) => groupOf(e.equipment_type) === g)
			.sort((a, b) => {
				const pa = prefer(a.equipment_type);
				const pb = prefer(b.equipment_type);
				return (pa < 0 ? 99 : pa) - (pb < 0 ? 99 : pb) || a.name.localeCompare(b.name);
			});
	const toggle = (list: number[], id: number) => (list.includes(id) ? list.filter((x) => x !== id) : [...list, id]);

	$: srcGear = gear(src);
	$: earNames = ears.map((id) => gear(id)?.name ?? '');
	$: midNames = mids.map((id) => gear(id)?.name ?? '');
	// A name from the gear until the person types one: where it plays from,
	// and what it is heard on.
	$: suggested = srcGear ? `${srcGear.name.split(' ')[0]} · ${earNames[0] ?? 'listening'}` : 'New chain';
	$: if (!named) name = suggested;
	// A phone or a music player goes with you.
	$: portable = rooms.find((r) => r.kind === 'portable');
	$: if (!roomsTouched && portable)
		roomIds = (srcGear && ['phone', 'dap'].includes(srcGear.equipment_type)) || (!srcGear && start === 'go') ? [portable.id] : [];
	$: reporters = devices.filter((d) => d.equipment_id != null && d.equipment_id === src);
	// A start from a box with no devices tied to it falls back to asking.
	$: if (find === 'device' && !reporters.length && devices.length) find = 'ask';

	$: canNext = step === 1 ? ears.length > 0 : step === 2 ? src != null : name.trim().length > 0;

	// Inline new gear, on whichever step asked for it.
	let newFor: GearGroup | null = null;
	let newName = '';
	let newType = '';
	async function makeGear() {
		if (!newFor || !newName.trim()) return;
		try {
			const made = await api.createEquipment({ name: newName.trim(), type: newType || NEW_GEAR_TYPES[newFor][0].v });
			equipment = (await api.getEquipment()).equipment;
			if (newFor === 'heard') ears = [...ears, made.id];
			else if (newFor === 'from') src = made.id;
			else mids = [...mids, made.id];
			newFor = null;
			newName = '';
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not add that gear');
		}
	}
	function openNew(g: GearGroup) {
		newFor = g;
		newName = '';
		newType = NEW_GEAR_TYPES[g][0].v;
	}

	let newRoom = '';
	async function addRoom() {
		if (!newRoom.trim()) return;
		try {
			const made = await api.createRoom(newRoom.trim(), null);
			rooms = (await api.getRooms()).rooms;
			roomIds = [...roomIds, made.id];
			roomsTouched = true;
			newRoom = '';
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not add that room');
		}
	}

	function step_(g: Equipment): ChainComponent {
		return { type: roleOfGear(g.equipment_type), name: g.name, detail: null, equipment_id: g.id };
	}
	async function create() {
		const s = srcGear;
		const first = gear(ears[0] ?? null);
		if (!s || !first) return;
		creating = true;
		error = '';
		try {
			const components: ChainComponent[] = [
				step_(s),
				...mids.map((id) => gear(id)).filter((g): g is Equipment => !!g).map(step_),
				{ ...step_(first), options: ears.length > 1 ? ears : undefined }
			];
			const made = await api.createChain({
				name: name.trim(),
				components,
				listening_context: ['phone', 'dap'].includes(s.equipment_type) ? 'active-mobile' : 'active',
				icon: CHAIN_ICON_FOR_SOURCE[s.equipment_type] ?? null
			});
			if (roomIds.length) await api.setChainRooms(made.id, roomIds);
			if (find === 'device') for (const d of reporters) await api.setDeviceChain(d.id, made.id);
			await goto(`/chains?chain=${made.id}&saved=${encodeURIComponent(`${name.trim()} is ready.`)}`);
		} catch (e) {
			error = e instanceof Error ? e.message.replace(/^API \d+: /, '') : 'Could not create the chain';
		}
		creating = false;
	}
	const next = () => (step < 3 ? (step += 1) : create());
</script>

<svelte:head><title>New chain — Tapedeck</title></svelte:head>

<a class="back" href="/chains">‹ Signal Chains</a>
<h1 class="h-display">New chain</h1>

<ol class="steps" aria-label="Steps">
	{#each ['Heard on', 'Plays from', 'Name it'] as label, i}
		<li class:on={step === i + 1} class:past={step > i + 1}><span class="n">{i + 1}</span>{label}</li>
	{/each}
</ol>

{#if loading}
	<div class="panel note">Loading…</div>
{:else}
	<div class="preview" aria-label="Your route so far">
		<span class="plabel">Your route so far</span>
		<span class="pv" class:empty={!srcGear}>{srcGear?.name ?? 'Plays from…'}</span>
		{#if midNames.length}<span class="ar">→</span><span class="pv">{midNames.join(' → ')}</span>{/if}
		<span class="ar">→</span>
		<span class="pv" class:empty={!ears.length} class:slot={ears.length > 1}>
			{ears.length === 0 ? 'Heard on…' : ears.length === 1 ? earNames[0] : `${earNames[0]} ▾ +${ears.length - 1}`}
		</span>
	</div>

	<section class="panel card">
		{#if step === 1}
			<h2 class="h-display">What do you hear it on?</h2>
			<p class="muted">The end of the route. If you swap between a few, pick them all: you get a “what’s on now” switch, not separate chains.</p>
			<div class="grid">
				{#each byGroup('heard') as g (g.id)}
					<button class="pick" class:on={ears.includes(g.id)} aria-pressed={ears.includes(g.id)} on:click={() => (ears = toggle(ears, g.id))}>
						<span class="box">{ears.includes(g.id) ? '✓' : ''}</span>
						<span class="ct"><b>{g.name}</b><small>{sub(g)}{ears[0] === g.id && ears.length > 1 ? ' · wired' : ''}</small></span>
					</button>
				{/each}
			</div>
			{#if ears.length > 1}<p class="muted">The first one you picked is what the chain is wired with; switch any time.</p>{/if}
		{:else if step === 2}
			<h2 class="h-display">What plays into it?</h2>
			<p class="muted">The device the music starts on. Anything in between is optional.</p>
			<div class="grid">
				{#each byGroup('from') as g (g.id)}
					<button class="pick" class:on={src === g.id} aria-pressed={src === g.id} on:click={() => (src = g.id)}>
						<span class="radio"></span>
						<span class="ct"><b>{g.name}</b><small>{sub(g)}</small></span>
					</button>
				{/each}
			</div>
			<div class="kicker">In between · optional</div>
			<div class="grid">
				{#each byGroup('between') as g (g.id)}
					<button class="pick" class:on={mids.includes(g.id)} aria-pressed={mids.includes(g.id)} on:click={() => (mids = toggle(mids, g.id))}>
						<span class="box">{mids.includes(g.id) ? mids.indexOf(g.id) + 1 : ''}</span>
						<span class="ct"><b>{g.name}</b><small>{sub(g)}</small></span>
					</button>
				{/each}
			</div>
			{#if mids.length > 1}<p class="muted">In the order you picked them, as the numbers show.</p>{/if}
		{:else}
			<h2 class="h-display">Name it</h2>
			<p class="muted">Call it after where you are or what you’re doing. Tapedeck suggested one from the gear.</p>
			<label class="field"><span>Name</span>
				<input class="input" bind:value={name} on:input={() => (named = true)} />
			</label>
			<div class="field"><span>Which room is it in?</span>
				<div class="chips">
					{#each rooms as r (r.id)}
						<button class="chip" class:on={roomIds.includes(r.id)} aria-pressed={roomIds.includes(r.id)}
							on:click={() => { roomIds = toggle(roomIds, r.id); roomsTouched = true; }}>
							<RoomIcon name={roomGlyph(r)} size={13} title="" />{r.name}
						</button>
					{/each}
					<span class="newroom">
						<input class="input" bind:value={newRoom} placeholder="+ New room" aria-label="New room name"
							on:keydown={(e) => e.key === 'Enter' && addRoom()} />
						{#if newRoom.trim()}<button class="btn btn-ghost" on:click={addRoom}>Add</button>{/if}
					</span>
				</div>
			</div>
			<div class="field"><span>How will plays find it?</span>
				<button class="pick wide" class:on={find === 'ask'} aria-pressed={find === 'ask'} on:click={() => (find = 'ask')}>
					<span class="radio"></span>
					<span class="ct"><b>Ask me when a new output turns up</b><small>The first play through it shows up as a question; one tap sends it here</small></span>
				</button>
				<button class="pick wide" class:on={find === 'device'} aria-pressed={find === 'device'} disabled={!reporters.length}
					on:click={() => (find = 'device')}>
					<span class="radio"></span>
					<span class="ct">
						<b>Everything from {srcGear?.name ?? 'that device'} with no other match</b>
						<small>{reporters.length
							? `Makes this the fallback for ${reporters.map((d) => d.name ?? d.machine_id).join(', ')}`
							: `Nothing reports as ${srcGear?.name ?? 'it'} yet; the inbox ties them together when it does`}</small>
					</span>
				</button>
			</div>
		{/if}

		{#if step < 3}
			{@const g = step === 1 ? 'heard' : null}
			{#if newFor}
				<div class="newgear">
					<input class="input" bind:value={newName} placeholder="Name" aria-label="New gear name" />
					<select class="input" bind:value={newType} aria-label="New gear type">
						{#each NEW_GEAR_TYPES[newFor] as t}<option value={t.v}>{t.label}</option>{/each}
					</select>
					<button class="btn btn-ghost" on:click={() => (newFor = null)}>Cancel</button>
					<button class="btn btn-secondary" disabled={!newName.trim()} on:click={makeGear}>Add</button>
				</div>
			{:else if g}
				<button class="padd" on:click={() => openNew('heard')}>+ New gear <small>add it here, inline</small></button>
			{:else}
				<div class="adds">
					<button class="padd" on:click={() => openNew('from')}>+ New source</button>
					<button class="padd" on:click={() => openNew('between')}>+ New box in between</button>
				</div>
			{/if}
		{/if}
		{#if error}<p class="err">{error}</p>{/if}
	</section>

	<div class="acts">
		<a class="btn btn-ghost" href="/chains">Cancel</a>
		<span class="grow"></span>
		{#if step > 1}<button class="btn btn-secondary" on:click={() => (step -= 1)}>Back</button>{/if}
		<button class="btn btn-primary" disabled={!canNext || creating} on:click={next}>
			{step === 3 ? (creating ? 'Creating…' : 'Create chain') : 'Next'}
		</button>
	</div>
{/if}

<style>
	.back {
		font-size: 13.5px;
		color: var(--td-accent);
	}
	h1 {
		font-size: 32px;
		margin: 4px 0 10px;
	}
	.steps {
		list-style: none;
		display: flex;
		gap: 18px;
		padding: 0;
		margin: 0 0 14px;
		flex-wrap: wrap;
	}
	.steps li {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 13.5px;
		color: var(--td-muted-2);
		font-weight: 600;
	}
	.steps .n {
		width: 24px;
		height: 24px;
		border-radius: 999px;
		display: grid;
		place-items: center;
		background: var(--td-surface2);
		font-size: 12px;
	}
	.steps li.on {
		color: var(--td-text);
	}
	.steps li.on .n,
	.steps li.past .n {
		background: var(--td-accent);
		color: var(--td-on-accent);
	}
	.preview {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
		margin-bottom: 12px;
		font-size: 13.5px;
	}
	.plabel {
		font-size: 11px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		font-weight: 700;
		color: var(--td-muted-2);
		margin-right: 4px;
	}
	.pv {
		padding: 4px 12px;
		border-radius: 999px;
		background: var(--td-surface2);
		font-weight: 600;
	}
	.pv.empty {
		color: var(--td-muted-2);
		font-weight: 500;
		border: 1px dashed var(--td-border-2);
		background: none;
	}
	.pv.slot {
		border: 1.5px dashed var(--td-accent);
		background: none;
	}
	.ar {
		color: var(--td-muted-3);
	}
	.note {
		border-radius: 20px;
		padding: 22px;
		color: var(--td-muted);
	}
	.card {
		border-radius: 22px;
		padding: 20px 22px;
		max-width: 900px;
	}
	.card h2 {
		font-size: 22px;
		margin: 0;
	}
	.muted {
		margin: 4px 0 12px;
		font-size: 13px;
		color: var(--td-muted);
		max-width: 70ch;
	}
	.kicker {
		font-size: 11px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		font-weight: 700;
		color: var(--td-muted-2);
		margin: 16px 0 8px;
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
		gap: 8px;
	}
	.pick {
		display: flex;
		align-items: center;
		gap: 10px;
		text-align: left;
		padding: 10px 12px;
		border-radius: 14px;
		border: 1px solid var(--td-border);
		background: var(--td-surface);
		font: inherit;
		color: inherit;
		cursor: pointer;
	}
	.pick.wide {
		width: 100%;
		max-width: 620px;
	}
	.pick.on {
		border-color: var(--td-accent);
	}
	.pick:disabled {
		opacity: 0.55;
		cursor: default;
	}
	.ct {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
	.ct b {
		font-weight: 600;
		font-size: 13.5px;
	}
	.ct small {
		font-size: 12px;
		color: var(--td-muted);
	}
	.box {
		flex: none;
		width: 20px;
		height: 20px;
		border-radius: 6px;
		border: 1.5px solid var(--td-muted-3);
		display: grid;
		place-items: center;
		font-size: 11.5px;
		color: var(--td-on-accent);
	}
	.on > .box {
		background: var(--td-accent);
		border-color: var(--td-accent);
	}
	.radio {
		flex: none;
		width: 18px;
		height: 18px;
		border-radius: 999px;
		border: 1.5px solid var(--td-muted-3);
	}
	.pick.on .radio {
		border: 5px solid var(--td-accent);
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 8px;
		margin-top: 14px;
		max-width: 620px;
	}
	.field > span {
		font-size: 12px;
		font-weight: 600;
		color: var(--td-muted);
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		align-items: center;
	}
	.chip {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 6px 12px;
		border-radius: 999px;
		border: 1px solid var(--td-border-2);
		background: var(--td-surface);
		font: inherit;
		font-size: 13px;
		color: inherit;
		cursor: pointer;
	}
	.chip.on {
		background: var(--td-accent);
		border-color: var(--td-accent);
		color: var(--td-on-accent);
	}
	.newroom {
		display: inline-flex;
		gap: 6px;
	}
	.newroom input {
		width: 150px;
	}
	.padd {
		margin-top: 12px;
		border: none;
		background: none;
		color: var(--td-accent);
		font: inherit;
		font-size: 13px;
		cursor: pointer;
		padding: 0;
	}
	.padd small {
		color: var(--td-muted-2);
	}
	.adds {
		display: flex;
		gap: 16px;
	}
	.newgear {
		display: flex;
		gap: 8px;
		margin-top: 12px;
		flex-wrap: wrap;
		max-width: 620px;
	}
	.newgear input {
		flex: 1 1 180px;
	}
	.err {
		color: var(--td-bad-fg);
		font-size: 13px;
		margin: 12px 0 0;
	}
	.acts {
		display: flex;
		gap: 8px;
		margin-top: 14px;
		max-width: 900px;
	}
	.grow {
		flex: 1;
	}
</style>
