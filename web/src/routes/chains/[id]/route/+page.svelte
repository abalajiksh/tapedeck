<script lang="ts">
	// The route editor: gear palette, a canvas you wire with patch cables, and
	// an inspector for the selected box (Ashwin's call, 2026-11-07).
	//
	// A chain is still one line of steps, so the wires are read back into that
	// line on save, and forks, loops and loose boxes are refused with the box
	// named. Plugging a cable into a socket that already has one replaces it,
	// the way a patch bay does. Dropping gear on a box makes it an option there
	// (a slot); dropping it on the canvas adds a box.
	//
	// Nothing is saved until "Save route", which says whether plays already
	// logged keep the route they had (the default) or are re-read through the
	// new one. Output changes made here (an output sent to this chain, an
	// output that puts gear on by itself) are applied after the route saves.
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { SvelteFlow, Background, Controls, type Node, type Edge, type Connection } from '@xyflow/svelte';
	import '@xyflow/svelte/dist/style.css';
	import {
		api,
		type ChainComponent,
		type ChainVariant,
		type DeviceBinding,
		type Binding,
		type Equipment,
		type SignalChain,
		type WhatsOn
	} from '$lib/api';
	import { formatHours } from '$lib/utils';
	import RouteNode, { type RouteNodeData } from '$lib/components/chains/nodes/RouteNode.svelte';
	import FlowBridge from '$lib/components/chains/FlowBridge.svelte';
	import { BUILT_IN, ICON_FOR_GEAR, ICON_FOR_ROLE } from '$lib/components/chains/layout';
	import {
		COMP_TYPES,
		GROUP_TITLE,
		NEW_GEAR_TYPES,
		compTypeLabel,
		groupOf,
		roleOfGear,
		type GearGroup
	} from '$lib/components/chains/gear';
	import { pickVariant } from '$lib/components/chains/picks';

	$: chainId = Number($page.params.id);

	let chain: SignalChain | null = null;
	let equipment: Equipment[] = [];
	let variants: ChainVariant[] = [];
	let bindings: Binding[] = [];
	let deviceBindings: DeviceBinding[] = [];
	let chains: SignalChain[] = [];
	let whatsOn: WhatsOn | null = null;
	let loading = true;
	let error = '';

	const nodeTypes = { box: RouteNode };
	let nodes: Node<RouteNodeData>[] = [];
	let edges: Edge[] = [];
	let toFlow: (x: number, y: number) => { x: number; y: number };
	let fit: () => void;
	let seq = 0;

	// ── Loading ──
	onMount(async () => {
		try {
			const [c, g, v, b, cl] = await Promise.all([
				api.getChain(chainId),
				api.getEquipment(),
				api.getVariants(),
				api.getBindings(),
				api.getChains()
			]);
			chain = c;
			equipment = g.equipment;
			variants = v.variants;
			bindings = b.bindings;
			deviceBindings = b.device_bindings ?? [];
			chains = cl.chains;
			whatsOn = await api.getWhatsOn(chainId).catch(() => null);
			start();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not load the chain';
		}
		loading = false;
	});

	const gearOf = (id: number | null) => (id == null ? undefined : equipment.find((e) => e.id === id));
	const nameOf = (id: number) => gearOf(id)?.name ?? 'Gone from your gear';

	function boxData(step: Partial<ChainComponent> & { name: string; type: string }, onNow: number | null = null): RouteNodeData {
		const g = gearOf(step.equipment_id ?? null);
		const options = step.equipment_id != null ? (step.options?.length ? [...step.options] : [step.equipment_id]) : [];
		return {
			equipmentId: step.equipment_id ?? null,
			name: step.name,
			role: step.type,
			sub: g ? [g.brand, g.model].filter(Boolean).join(' ') : '',
			icon: (g ? ICON_FOR_GEAR[g.equipment_type] : ICON_FOR_ROLE[step.type]) ?? null,
			builtIn: g ? BUILT_IN.has(g.equipment_type) : false,
			detail: step.detail ?? null,
			since: step.since ?? null,
			options,
			optionNames: options.map(nameOf),
			onNow,
			slotName: step.slot_name ?? null,
			dropName: null,
			problem: null
		};
	}

	/** The chain as it is, one box per step, wired left to right. */
	function start() {
		if (!chain) return;
		const steps = chain.components ?? [];
		nodes = steps.map((s, i) => ({
			id: `n${++seq}`,
			type: 'box',
			position: { x: i * 280, y: 80 },
			data: boxData(s, whatsOn?.slots.find((sl) => sl.position === i)?.on ?? null)
		}));
		edges = nodes.slice(1).map((n, i) => wire(nodes[i].id, n.id));
		history = [];
		pending = [];
		fit?.();
	}
	const wire = (source: string, target: string): Edge => ({ id: `e${source}-${target}`, source, target });

	// ── Undo ──
	type Snapshot = { nodes: Node<RouteNodeData>[]; edges: Edge[]; pending: Pending[] };
	let history: Snapshot[] = [];
	function remember() {
		history = [
			...history,
			{
				nodes: nodes.map((n) => ({ ...n, data: { ...n.data, options: [...n.data.options] } })),
				edges: edges.map((e) => ({ ...e })),
				pending: pending.map((p) => ({ ...p }))
			}
		].slice(-60);
	}
	function undo() {
		const last = history[history.length - 1];
		if (!last) return;
		history = history.slice(0, -1);
		nodes = last.nodes;
		edges = last.edges;
		pending = last.pending;
		problem = '';
	}
	function onKey(e: KeyboardEvent) {
		const t = e.target as HTMLElement | null;
		if (t && (t.tagName === 'INPUT' || t.tagName === 'SELECT' || t.tagName === 'TEXTAREA')) return;
		if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'z') {
			e.preventDefault();
			undo();
		}
	}

	// ── Wiring ──
	// A cable into an occupied socket replaces the cable that was there.
	const isValidConnection = (c: Connection | Edge) => c.source !== c.target;
	function onbeforeconnect(c: Connection): Edge {
		remember();
		return wire(c.source, c.target);
	}
	function onconnect(c: Connection) {
		edges = edges.filter(
			(e) => !(e.source === c.source && e.target !== c.target) && !(e.target === c.target && e.source !== c.source)
		);
		clearProblems();
	}
	function onbeforedelete() {
		remember();
		return Promise.resolve(true);
	}
	function ondelete({ nodes: gone }: { nodes: Node[]; edges: Edge[] }) {
		if (gone.some((n) => n.id === selectedId)) selectedId = null;
		clearProblems();
	}

	// ── Selection ──
	let selectedId: string | null = null;
	$: selected = nodes.find((n) => n.id === selectedId) ?? null;
	function onselectionchange({ nodes: sel }: { nodes: Node[]; edges: Edge[] }) {
		selectedId = sel.length === 1 ? sel[0].id : sel.length ? selectedId : null;
	}
	function patchSelected(fn: (d: RouteNodeData) => RouteNodeData) {
		if (!selectedId) return;
		remember();
		nodes = nodes.map((n) => (n.id === selectedId ? { ...n, data: fn({ ...n.data }) } : n));
	}
	function removeSelected() {
		if (!selectedId) return;
		remember();
		const id = selectedId;
		nodes = nodes.filter((n) => n.id !== id);
		edges = edges.filter((e) => e.source !== id && e.target !== id);
		selectedId = null;
	}
	/** Put a new box between the selected box and the one it feeds. */
	function addBetween() {
		if (!selected) return;
		const out = edges.find((e) => e.source === selected!.id);
		remember();
		const id = `n${++seq}`;
		const at = selected.position;
		nodes = [
			...nodes,
			{ id, type: 'box', position: { x: at.x + 140, y: at.y + 170 }, data: boxData({ name: 'New step', type: 'amp' }) }
		];
		edges = [...edges.filter((e) => e !== out), wire(selected.id, id), ...(out ? [wire(id, out.target)] : [])];
		selectedId = id;
	}

	// ── The palette ──
	$: inRoute = new Set(nodes.flatMap((n) => n.data.options.length ? n.data.options : n.data.equipmentId != null ? [n.data.equipmentId] : []));
	$: palette = (['heard', 'between', 'from'] as GearGroup[]).map((g) => ({
		group: g,
		gear: equipment
			.filter((e) => groupOf(e.equipment_type) === g)
			.sort((a, b) => Number(inRoute.has(b.id)) - Number(inRoute.has(a.id)) || a.name.localeCompare(b.name))
	}));
	let dragging: Equipment | null = null;

	function placeGear(g: Equipment, at: { x: number; y: number }) {
		remember();
		const id = `n${++seq}`;
		nodes = [
			...nodes,
			{ id, type: 'box', position: at, data: boxData({ name: g.name, type: roleOfGear(g.equipment_type), equipment_id: g.id }) }
		];
		selectedId = id;
	}
	/** Clicking a palette entry: a new box below everything, to be wired. */
	function addGear(g: Equipment) {
		const maxY = nodes.reduce((m, n) => Math.max(m, n.position.y), 0);
		const minX = nodes.reduce((m, n) => Math.min(m, n.position.x), 0);
		placeGear(g, { x: minX + 140, y: maxY + 190 });
	}
	function addOption(nodeId: string, g: Equipment) {
		const n = nodes.find((x) => x.id === nodeId);
		if (!n || n.data.equipmentId == null || n.data.options.includes(g.id)) return;
		remember();
		nodes = nodes.map((x) =>
			x.id === nodeId
				? { ...x, data: { ...x.data, options: [...x.data.options, g.id], optionNames: [...x.data.optionNames, g.name] } }
				: x
		);
		selectedId = nodeId;
	}
	// The box under the pointer: the drag event's own target, which is what
	// the browser hit-tested.
	const nodeAt = (e: DragEvent) =>
		(e.target as HTMLElement | null)?.closest?.('.svelte-flow__node')?.getAttribute('data-id') ?? null;
	let hoverId: string | null = null;
	function onDragOver(e: DragEvent) {
		if (!dragging) return;
		e.preventDefault();
		const id = nodeAt(e);
		const target = id ? nodes.find((n) => n.id === id) : null;
		const next = target && target.data.equipmentId != null && !target.data.options.includes(dragging.id) ? id : null;
		if (next !== hoverId) {
			hoverId = next;
			const name = dragging.name;
			nodes = nodes.map((n) => ({ ...n, data: { ...n.data, dropName: n.id === next ? name : null } }));
		}
	}
	function onDrop(e: DragEvent) {
		if (!dragging) return;
		e.preventDefault();
		const g = dragging;
		const id = hoverId;
		dragging = null;
		hoverId = null;
		nodes = nodes.map((n) => (n.data.dropName ? { ...n, data: { ...n.data, dropName: null } } : n));
		if (id) addOption(id, g);
		else placeGear(g, toFlow(e.clientX - 110, e.clientY - 30));
	}
	function onDragEnd() {
		dragging = null;
		if (hoverId) {
			hoverId = null;
			nodes = nodes.map((n) => (n.data.dropName ? { ...n, data: { ...n.data, dropName: null } } : n));
		}
	}

	// New gear, made here and placed straight away.
	let newGroup: GearGroup | null = null;
	let newName = '';
	let newType = '';
	async function makeGear() {
		if (!newGroup || !newName.trim()) return;
		try {
			const made = await api.createEquipment({ name: newName.trim(), type: newType || NEW_GEAR_TYPES[newGroup][0].v });
			equipment = (await api.getEquipment()).equipment;
			const g = equipment.find((e) => e.id === made.id);
			if (g) addGear(g);
			newGroup = null;
			newName = '';
			newType = '';
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not add that gear');
		}
	}

	// ── Outputs and the rules that pick gear by themselves ──
	type Pending =
		| { kind: 'send'; identifier: string; equipmentId: number | null; on: boolean }
		| { kind: 'rule'; identifier: string; equipmentId: number | null; gear: number | null };
	let pending: Pending[] = [];
	function queue(p: Pending) {
		remember();
		pending = [...pending.filter((x) => !(x.kind === p.kind && x.identifier === p.identifier && x.equipmentId === p.equipmentId)), p];
	}
	const variantGear = (vid: number | null, options: number[]) => {
		const v = variants.find((x) => x.id === vid);
		return v?.overrides.find((o) => o.equipment_id != null && options.includes(o.equipment_id))?.equipment_id ?? null;
	};
	/** The outputs this chain hears from: those of its source box, then any by
	 *  name already pointing here. Each with where it goes once the queue is
	 *  applied, and the gear it puts on by itself at a slot with `options`. */
	function outputsFor(sourceGear: number | null, options: number[]) {
		const rows = [
			...deviceBindings.filter((b) => b.equipment_id === sourceGear).map((b) => ({ b, box: b.equipment_id as number | null })),
			...bindings.filter((b) => b.chain_id === chainId).map((b) => ({ b, box: null as number | null }))
		];
		return rows.map(({ b, box }) => {
			const send = pending.find((p) => p.kind === 'send' && p.identifier === b.identifier && p.equipmentId === box);
			const rule = pending.find((p) => p.kind === 'rule' && p.identifier === b.identifier && p.equipmentId === box);
			const here = send ? (send as { on: boolean }).on : b.chain_id === chainId;
			const picks = rule ? (rule as { gear: number | null }).gear : b.chain_id === chainId ? variantGear(b.chain_variant_id, options) : null;
			const elsewhere = !here && b.chain_id != null ? chains.find((c) => c.id === b.chain_id)?.name ?? 'another chain' : null;
			return { identifier: b.identifier, box, here, picks, elsewhere, iface: b.interface };
		});
	}
	$: sourceNode = nodes.find((n) => !edges.some((e) => e.target === n.id) && n.data.role === 'source') ?? null;
	$: selectedOutputs = selected ? outputsFor(sourceNode?.data.equipmentId ?? null, selected.data.options) : [];

	// ── Reading the wires back into a line ──
	let problem = '';
	function clearProblems() {
		problem = '';
		if (nodes.some((n) => n.data.problem)) nodes = nodes.map((n) => (n.data.problem ? { ...n, data: { ...n.data, problem: null } } : n));
	}
	function flag(id: string, msg: string) {
		nodes = nodes.map((n) => ({ ...n, data: { ...n.data, problem: n.id === id ? msg : null } }));
		problem = msg;
		return null;
	}
	/** The boxes in signal order, or null with the trouble named. */
	function readLine(): Node<RouteNodeData>[] | null {
		problem = '';
		if (!nodes.length) {
			problem = 'Add at least one box.';
			return null;
		}
		const outs = new Map<string, string[]>();
		const ins = new Map<string, string[]>();
		for (const e of edges) {
			outs.set(e.source, [...(outs.get(e.source) ?? []), e.target]);
			ins.set(e.target, [...(ins.get(e.target) ?? []), e.source]);
		}
		const name = (id: string) => nodes.find((n) => n.id === id)?.data.name ?? 'A box';
		for (const n of nodes) {
			if ((outs.get(n.id)?.length ?? 0) > 1) return flag(n.id, `${n.data.name} feeds two boxes. A chain is one line.`);
			if ((ins.get(n.id)?.length ?? 0) > 1) return flag(n.id, `Two boxes feed ${n.data.name}. A chain is one line.`);
			if (nodes.length > 1 && !outs.has(n.id) && !ins.has(n.id)) return flag(n.id, `${n.data.name} isn’t wired to anything.`);
		}
		const starts = nodes.filter((n) => !ins.has(n.id));
		if (!starts.length) return flag(nodes[0].id, 'The wires go round in a loop. A chain starts somewhere.');
		if (starts.length > 1) return flag(starts[1].id, `The route is in ${starts.length} pieces. Wire ${name(starts[1].id)} into the line.`);
		const order: Node<RouteNodeData>[] = [];
		const seen = new Set<string>();
		let at: string | undefined = starts[0].id;
		while (at && !seen.has(at)) {
			seen.add(at);
			order.push(nodes.find((n) => n.id === at)!);
			at = outs.get(at)?.[0];
		}
		if (order.length < nodes.length) {
			const loose = nodes.find((n) => !seen.has(n.id))!;
			return flag(loose.id, `${loose.data.name} is in a loop, not the line.`);
		}
		clearProblems();
		return order;
	}

	function toComponents(order: Node<RouteNodeData>[]): ChainComponent[] {
		return order.map((n) => ({
			type: n.data.role,
			name: n.data.name.trim() || 'Unnamed',
			detail: n.data.detail,
			equipment_id: n.data.equipmentId,
			since: n.data.since,
			options: n.data.equipmentId != null && n.data.options.length >= 2 ? n.data.options : [],
			slot_name: n.data.options.length >= 2 ? n.data.slotName : null
		}));
	}

	// What changed, in words, for the save bar.
	$: summary = (() => {
		if (!chain) return '';
		const before = chain.components ?? [];
		const beforeGear = new Set(before.map((c) => c.equipment_id).filter((x) => x != null));
		const nowGear = new Set(nodes.map((n) => n.data.equipmentId).filter((x) => x != null));
		const bits: string[] = [];
		for (const n of nodes) if (n.data.equipmentId != null && !beforeGear.has(n.data.equipmentId)) bits.push(`Added ${n.data.name}`);
		for (const c of before) if (c.equipment_id != null && !nowGear.has(c.equipment_id)) bits.push(`Removed ${c.name}`);
		for (const n of nodes) {
			const was = before.find((c) => c.equipment_id != null && c.equipment_id === n.data.equipmentId);
			const wasOpts = new Set(was?.options ?? []);
			for (let i = 1; i < n.data.options.length; i++)
				if (!wasOpts.has(n.data.options[i])) bits.push(`Added ${n.data.optionNames[i]} to ${n.data.slotName ?? n.data.name}`);
		}
		for (const p of pending) {
			if (p.kind === 'send') bits.push(p.on ? `${p.identifier} comes here` : `${p.identifier} no longer comes here`);
			else bits.push(p.gear != null ? `${p.identifier} now picks ${nameOf(p.gear)}` : `${p.identifier} picks nothing by itself`);
		}
		return bits.join(' · ');
	})();
	$: changes = history.length;

	// ── Saving ──
	let past: 'keep' | 'rewrite' = 'keep';
	let saving = false;
	async function save() {
		if (!chain) return;
		const order = readLine();
		if (!order) return;
		if (
			past === 'rewrite' &&
			!confirm(
				`Re-read every play ever logged through “${chain.name}” with the new route? ` +
					`${formatHours(chain.total_hours)} of listening will credit the new gear instead of what it ran through. ` +
					`Use this only to fix a mistake.`
			)
		)
			return;
		saving = true;
		try {
			const components = toComponents(order);
			const out = await api.saveRoute(chain.id, { components, past });
			if (pending.length) {
				const [fresh, vs] = await Promise.all([api.getChain(chain.id), api.getVariants()]);
				for (const p of pending) {
					if (p.kind === 'send') {
						await api.setBinding(p.identifier, p.on ? chain.id : null, null, p.equipmentId);
					} else {
						const vid = p.gear == null ? null : await pickVariant(chain.id, fresh.components ?? [], vs.variants, p.gear, nameOf(p.gear));
						await api.setBinding(p.identifier, chain.id, vid, p.equipmentId);
					}
				}
			}
			const note = out.unchanged
				? 'Saved.'
				: out.past === 'keep'
					? `Route saved. ${out.kept ? `${out.kept} ${out.kept === 1 ? 'play keeps' : 'plays keep'} the old route.` : 'Nothing in your history moved.'}`
					: 'Route saved, and every past play re-read through it.';
			await goto(`/chains?chain=${chain.id}&saved=${encodeURIComponent(note)}`);
		} catch (e) {
			problem = e instanceof Error ? e.message.replace(/^API \d+: /, '') : 'That did not save';
		}
		saving = false;
	}

	$: scopeLine =
		past === 'keep'
			? 'Plays already logged keep the route they had. Nothing in your history moves.'
			: `Every play ever logged here is re-read with the new route${chain ? `, ${formatHours(chain.total_hours)} of it` : ''}. Use this only to fix a mistake.`;
	$: hoursHere = (id: number) =>
		(gearOf(id)?.sources ?? []).filter((s) => s.chain_id === chainId).reduce((a, s) => a + s.hours, 0);
</script>

<svelte:window on:keydown={onKey} />
<svelte:head><title>Edit the route{chain ? ` · ${chain.name}` : ''} — Tapedeck</title></svelte:head>

{#if loading}
	<div class="panel note">Loading…</div>
{:else if error || !chain}
	<div class="panel note">{error || 'No such chain'} · <a href="/chains">Signal Chains</a></div>
{:else}
	<header class="bar">
		<div class="btitle">
			<a class="back" href="/chains?chain={chain.id}">‹ {chain.name}</a>
			<h1 class="h-display">Edit the route</h1>
			{#if changes}<span class="tag tag-accent">{changes} {changes === 1 ? 'change' : 'changes'}</span>{/if}
		</div>
		<div class="bacts">
			<button class="btn btn-ghost" disabled={!history.length} on:click={undo} title="⌘Z">Undo</button>
			<a class="btn btn-secondary" href="/chains?chain={chain.id}">Cancel</a>
			<button class="btn btn-primary" disabled={saving} on:click={save}>{saving ? 'Saving…' : 'Save route'}</button>
		</div>
	</header>

	<div class="editor">
		<aside class="palette" aria-label="Your gear">
			<p class="phint">Drag onto the canvas. Drop on a box to make it an option you can swap to.</p>
			{#each palette as p (p.group)}
				<div class="pgroup">
					<div class="kicker">{GROUP_TITLE[p.group]}</div>
					{#each p.gear as g (g.id)}
						<button class="pitem" class:in={inRoute.has(g.id)} draggable="true"
							on:dragstart={(e) => { dragging = g; e.dataTransfer?.setData('text/plain', g.name); }}
							on:dragend={onDragEnd}
							on:click={() => addGear(g)} title="Drag onto the canvas, or click to add a box">
							<span class="grip" aria-hidden="true">⋮⋮</span>
							<span class="pname">{g.name}</span>
							{#if inRoute.has(g.id)}<span class="ptag">in route</span>{:else if g.status === 'lent'}<span class="ptag">lent out</span>{/if}
						</button>
					{/each}
					{#if newGroup === p.group}
						<div class="newgear">
							<input class="input" bind:value={newName} placeholder="Name" aria-label="New gear name" />
							<select class="input" bind:value={newType} aria-label="New gear type">
								{#each NEW_GEAR_TYPES[p.group] as t}<option value={t.v}>{t.label}</option>{/each}
							</select>
							<div class="ngacts">
								<button class="btn btn-ghost" on:click={() => (newGroup = null)}>Cancel</button>
								<button class="btn btn-secondary" disabled={!newName.trim()} on:click={makeGear}>Add</button>
							</div>
						</div>
					{:else}
						<button class="padd" on:click={() => { newGroup = p.group; newType = NEW_GEAR_TYPES[p.group][0].v; }}>+ New gear</button>
					{/if}
				</div>
			{/each}
		</aside>

		<section class="canvas" aria-label="The route, as boxes and cables" on:dragover={onDragOver} on:drop={onDrop}>
			<SvelteFlow bind:nodes bind:edges {nodeTypes} fitView minZoom={0.3} maxZoom={1.5}
				{isValidConnection} {onbeforeconnect} {onconnect} {onbeforedelete} {ondelete} {onselectionchange}
				deleteKey={['Backspace', 'Delete']} proOptions={{ hideAttribution: true }}>
				<Background gap={22} />
				<Controls showLock={false} />
				<FlowBridge bind:toFlow bind:fit />
			</SvelteFlow>
			<p class="chint">Drag from a box’s right socket to the next box’s left to wire · Backspace removes the selected box or cable · ⌘Z undoes</p>
		</section>

		<aside class="inspector" aria-label="The selected box">
			{#if selected}
				{@const d = selected.data}
				<div class="kicker">Selected · {compTypeLabel(d.role)}</div>
				<h2 class="h-display iname">{d.options.length >= 2 ? (d.slotName ?? d.name) : d.name}</h2>
				{#if d.equipmentId == null}
					<label class="field"><span>Name</span>
						<input class="input" value={d.name} on:change={(e) => { const v = e.currentTarget.value; patchSelected((x) => ({ ...x, name: v })); }} />
					</label>
				{/if}
				<label class="field"><span>Role in the route</span>
					<select class="input" value={d.role} on:change={(e) => { const v = e.currentTarget.value; patchSelected((x) => ({ ...x, role: v })); }}>
						{#each COMP_TYPES as t}<option value={t.v}>{t.label}</option>{/each}
					</select>
				</label>

				{#if d.equipmentId != null}
					{#if d.options.length >= 2}
						<label class="field"><span>Slot name</span>
							<input class="input" value={d.slotName ?? ''} placeholder="On the go"
								on:change={(e) => { const v = e.currentTarget.value.trim() || null; patchSelected((x) => ({ ...x, slotName: v })); }} />
						</label>
					{/if}
					<div class="field"><span>{d.options.length >= 2 ? 'Options' : 'Gear you swap in here'}</span>
						{#each d.options as id, i (id)}
							<div class="orow">
								<span class="oname">{d.optionNames[i]}{#if i === 0}<small> · wired</small>{/if}{#if d.onNow === id}<small> · on now</small>{/if}</span>
								<span class="ohrs">{formatHours(hoursHere(id))} here</span>
								{#if i > 0}
									<button class="x" aria-label="Remove {d.optionNames[i]} as an option" title="Remove (its hours stay)"
										on:click={() => patchSelected((x) => ({ ...x, options: x.options.filter((o) => o !== id), optionNames: x.optionNames.filter((_, j) => j !== i) }))}>✕</button>
								{/if}
							</div>
						{/each}
						<p class="muted">Drop gear from the palette on this box to add it. Removing an option keeps its hours.</p>
					</div>

					{#if d.options.length >= 2}
						<div class="field"><span>Picks itself when</span>
							{#each selectedOutputs.filter((o) => o.here) as o (`${o.box}:${o.identifier}`)}
								<div class="rule">
									<span class="rname">{o.identifier}{o.box == null ? ' (any device)' : ''}</span>
									<select class="input rsel" value={o.picks ?? ''} aria-label="What {o.identifier} puts on"
										on:change={(e) => { const v = e.currentTarget.value; queue({ kind: 'rule', identifier: o.identifier, equipmentId: o.box, gear: v ? Number(v) : null }); }}>
										<option value="">whatever is on now</option>
										{#each d.options as id, i (id)}<option value={id}>switches to {d.optionNames[i]}</option>{/each}
									</select>
								</div>
							{:else}
								<p class="muted">No output comes here yet. Send one here from the box it plays from.</p>
							{/each}
						</div>
					{/if}

					{#if selected.id === sourceNode?.id}
						<div class="field"><span>Outputs of {d.name}</span>
							{#each outputsFor(d.equipmentId, []).filter((o) => o.box != null) as o (o.identifier)}
								<div class="rule">
									<span class="rname">{o.identifier}<small>{o.here ? ' · comes here' : o.elsewhere ? ` · goes to ${o.elsewhere}` : ' · goes nowhere yet'}</small></span>
									<button class="btn btn-ghost sm" on:click={() => queue({ kind: 'send', identifier: o.identifier, equipmentId: o.box, on: !o.here })}>
										{o.here ? 'Stop' : 'Send here'}
									</button>
								</div>
							{:else}
								<p class="muted">No outputs learned for it yet. The first play through one is asked about in the inbox.</p>
							{/each}
						</div>
					{/if}
				{/if}

				<div class="iacts">
					<button class="btn btn-secondary" on:click={addBetween}>Add a step after it</button>
					<button class="btn btn-ghost del" on:click={removeSelected}>Remove this box</button>
				</div>
			{:else}
				<div class="kicker">Nothing selected</div>
				<p class="muted">Pick a box to name its slot, add options, set what an output puts on by itself, or send its outputs here.</p>
			{/if}
		</aside>
	</div>

	<footer class="savebar">
		<div class="stext">
			{#if problem}<p class="prob">{problem}</p>{:else if summary}<p class="sum">{summary}</p>{:else}<p class="sum muted">No changes yet.</p>{/if}
			<p class="scope">{scopeLine}</p>
		</div>
		<div class="seg" role="group" aria-label="What happens to plays already logged">
			<button class="seg-btn" class:on={past === 'keep'} aria-pressed={past === 'keep'} on:click={() => (past = 'keep')}>From now on</button>
			<button class="seg-btn" class:on={past === 'rewrite'} aria-pressed={past === 'rewrite'} on:click={() => (past = 'rewrite')}>Rewrite past plays too</button>
		</div>
	</footer>
{/if}

<style>
	.note {
		border-radius: 20px;
		padding: 22px;
		color: var(--td-muted);
	}
	.bar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		flex-wrap: wrap;
		margin-bottom: 12px;
	}
	.btitle {
		display: flex;
		align-items: baseline;
		gap: 12px;
		flex-wrap: wrap;
	}
	.back {
		font-size: 13.5px;
		color: var(--td-accent);
	}
	h1 {
		font-size: 28px;
		margin: 0;
	}
	.bacts {
		display: flex;
		gap: 8px;
	}
	.editor {
		display: grid;
		grid-template-columns: 220px minmax(0, 1fr) 290px;
		grid-template-areas: 'pal canvas insp';
		gap: 12px;
		height: calc(100vh - 230px);
		min-height: 460px;
	}
	.palette {
		grid-area: pal;
	}
	.canvas {
		grid-area: canvas;
	}
	.inspector {
		grid-area: insp;
	}
	/* Narrower: the canvas gets the whole width, the palette and the
	   inspector share the row under it. */
	@media (max-width: 1400px) {
		.editor {
			grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
			grid-template-areas: 'canvas canvas' 'pal insp';
			grid-template-rows: 58vh auto;
			height: auto;
		}
		.palette,
		.inspector {
			max-height: 52vh;
		}
	}
	.palette,
	.inspector {
		overflow-y: auto;
		border-radius: 20px;
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		padding: 14px;
	}
	.phint,
	.muted {
		margin: 0;
		font-size: 12px;
		color: var(--td-muted);
		line-height: 1.45;
	}
	.kicker {
		font-size: 11px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		font-weight: 700;
		color: var(--td-muted-2);
		margin: 12px 0 6px;
	}
	.pgroup {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.pitem {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 7px 9px;
		border-radius: 12px;
		border: 1px solid var(--td-border);
		background: var(--td-surface2);
		font: inherit;
		font-size: 13px;
		color: inherit;
		cursor: grab;
		text-align: left;
	}
	.pitem.in {
		opacity: 0.7;
	}
	.grip {
		color: var(--td-muted-3);
		font-size: 11px;
	}
	.pname {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.ptag {
		font-size: 10.5px;
		color: var(--td-muted-2);
	}
	.padd {
		border: none;
		background: none;
		color: var(--td-accent);
		font: inherit;
		font-size: 12.5px;
		text-align: left;
		padding: 4px 2px;
		cursor: pointer;
	}
	.newgear {
		display: flex;
		flex-direction: column;
		gap: 6px;
		padding: 8px;
		border-radius: 12px;
		border: 1px dashed var(--td-border-2);
	}
	.ngacts {
		display: flex;
		justify-content: flex-end;
		gap: 6px;
	}
	.canvas {
		position: relative;
		border-radius: 20px;
		overflow: hidden;
		border: 1px solid var(--td-border);
		background: var(--td-panel);
	}
	.chint {
		position: absolute;
		left: 0;
		right: 0;
		top: 8px;
		margin: 0;
		text-align: center;
		font-size: 11.5px;
		color: var(--td-muted-2);
		pointer-events: none;
	}
	.iname {
		font-size: 22px;
		margin: 0 0 8px;
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 6px;
		margin-top: 12px;
	}
	.field > span {
		font-size: 12px;
		font-weight: 600;
		color: var(--td-muted);
	}
	.orow,
	.rule {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 13px;
	}
	.oname,
	.rname {
		flex: 1;
		min-width: 0;
	}
	.oname small,
	.rname small {
		color: var(--td-muted-2);
	}
	.ohrs {
		font-size: 12px;
		color: var(--td-muted-2);
		font-variant-numeric: tabular-nums;
	}
	.rsel {
		max-width: 150px;
		font-size: 12.5px;
	}
	.x {
		border: none;
		background: none;
		color: var(--td-muted-2);
		cursor: pointer;
	}
	.x:hover {
		color: var(--td-bad-fg);
	}
	.sm {
		padding: 4px 10px;
		font-size: 12.5px;
	}
	.iacts {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
		margin-top: 18px;
	}
	.del {
		color: var(--td-bad-fg);
	}
	.savebar {
		display: flex;
		align-items: center;
		gap: 14px;
		flex-wrap: wrap;
		margin-top: 12px;
		padding: 12px 16px;
		border-radius: 20px;
		background: var(--td-surface);
		border: 1px solid var(--td-border);
	}
	.stext {
		flex: 1 1 300px;
		min-width: 0;
	}
	.sum,
	.prob,
	.scope {
		margin: 0;
		font-size: 13px;
	}
	.prob {
		color: var(--td-bad-fg);
		font-weight: 600;
	}
	.scope {
		color: var(--td-muted);
		font-size: 12.5px;
		margin-top: 2px;
	}
	.seg {
		display: inline-flex;
		padding: 3px;
		border-radius: 999px;
		background: var(--td-surface2);
	}
	.seg-btn {
		border: none;
		background: none;
		font: inherit;
		font-size: 13px;
		font-weight: 600;
		padding: 7px 14px;
		border-radius: 999px;
		color: var(--td-muted);
		cursor: pointer;
	}
	.seg-btn.on {
		background: var(--td-accent);
		color: var(--td-on-accent);
	}
	@media (max-width: 700px) {
		.editor {
			grid-template-columns: 1fr;
			grid-template-areas: 'canvas' 'insp' 'pal';
		}
	}
</style>
