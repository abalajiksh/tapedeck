<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { api } from '$lib/api';
	import type { SignalChain, Equipment, ChainComponent, ChainInput, EquipmentInput, Binding, Device } from '$lib/api';
	import { formatHours, timeAgo, toDateInput, fromDateInput, ownedFor, formatDateTime } from '$lib/utils';
	import ChainIcon, { CHAIN_ICONS, CHAIN_ICON_LABELS } from '$lib/components/ChainIcon.svelte';

	let chains: SignalChain[] = [];
	let equipment: Equipment[] = [];
	let bindings: Binding[] = [];
	let devices: Device[] = [];
	let loading = true;
	let error = '';

	$: unmapped = bindings.filter((b) => b.chain_id == null);
	$: mapped = bindings.filter((b) => b.chain_id != null);
	$: chainName = (id: number | null) => chains.find((c) => c.id === id)?.name ?? '—';

	const contexts = ['active', 'active-mobile', 'passive', 'background', 'unknown'];
	// Values must stay in sync with `ComponentType` in src/models.rs — serde
	// rejects anything else with a 422, so an extra option here is a broken save.
	// The *labels* are ours to choose, and "transducer" is jargon: the thing at
	// the end of the chain is headphones or speakers, and that's what it says.
	const compTypes = [
		{ v: 'source', label: 'Source' },
		{ v: 'transport', label: 'Transport' },
		{ v: 'dac', label: 'DAC' },
		{ v: 'amp', label: 'Amplifier' },
		{ v: 'transducer', label: 'Headphones / Speakers' },
		{ v: 'network', label: 'Network' },
		{ v: 'bluetooth', label: 'Bluetooth' }
	];
	const compTypeLabel = (v: string) => compTypes.find((t) => t.v === v)?.label ?? v;

	// Free text on the backend (`equipment_type` is a plain string), so this list
	// is a UI convenience. Unknown values from older rows are preserved below.
	const gearTypes = [
		{ v: 'headphones', label: 'Headphones' },
		{ v: 'iem', label: 'IEM' },
		{ v: 'speakers', label: 'Speakers' },
		{ v: 'builtin-speakers', label: 'Built-in speakers (phone / laptop)' },
		{ v: 'earbuds', label: 'Wireless earbuds' },
		{ v: 'dac', label: 'DAC' },
		{ v: 'amp', label: 'Amplifier' },
		{ v: 'dac-amp', label: 'DAC / Amp combo' },
		{ v: 'preamp', label: 'Preamp' },
		{ v: 'source', label: 'Source / Streamer' },
		{ v: 'phone', label: 'Phone' },
		{ v: 'computer', label: 'Computer' },
		{ v: 'dap', label: 'DAP / Music player' },
		{ v: 'transport', label: 'Transport' },
		{ v: 'turntable', label: 'Turntable' },
		{ v: 'interface', label: 'Audio interface' },
		{ v: 'receiver', label: 'Receiver / Amp-DAC' },
		{ v: 'cable', label: 'Cable' },
		{ v: 'other', label: 'Other' }
	];
	// Keep whatever an existing row already has, even if it predates this list.
	$: gearTypeOptions = gearTypes.some((t) => t.v === gType)
		? gearTypes
		: [...gearTypes, { v: gType, label: gType }];
	// "iem" and "dac-amp" don't survive text-transform:capitalize intact.
	const gearTypeLabel = (v: string) => gearTypes.find((t) => t.v === v)?.label ?? v;

	// Picking a piece of gear should fill in its chain position too — nobody
	// wants to tell the app twice that headphones go at the end. Guessed once on
	// selection and freely overridable, never re-applied afterwards.
	const componentTypeForGear: Record<string, string> = {
		headphones: 'transducer', iem: 'transducer', speakers: 'transducer',
		'builtin-speakers': 'transducer', earbuds: 'bluetooth',
		dac: 'dac', 'dac-amp': 'dac', interface: 'dac', receiver: 'dac',
		amp: 'amp', preamp: 'amp',
		source: 'source', turntable: 'source', phone: 'source',
		computer: 'source', dap: 'source', other: 'source',
		transport: 'transport', cable: 'transport'
	};

	// The reverse: which gear belongs in a given slot. A DAC slot listing every
	// pair of headphones you own is the thing that made this dropdown useless.
	// Anything unmapped stays visible everywhere rather than becoming
	// unreachable, and "Show all gear" is always one click away.
	const gearTypesForComponent: Record<string, string[]> = {};
	for (const [gear, comp] of Object.entries(componentTypeForGear)) {
		(gearTypesForComponent[comp] ??= []).push(gear);
	}
	function gearFitsSlot(gearType: string, compType: string): boolean {
		const known = componentTypeForGear[gearType];
		return known === undefined || known === compType;
	}

	// chain editor
	let chainOpen = false;
	let chainId: number | null = null; // null = create
	let cName = '', cDesc = '', cContext = 'active';
	let cIcon: string | null = null;
	let cComps: ChainComponent[] = [];
	let savingChain = false;
	let confirmChain: SignalChain | null = null;

	// equipment editor
	let gearOpen = false;
	let gearId: number | null = null;
	let gName = '', gType = 'headphones', gBrand = '', gModel = '', gNotes = '';
	/** When it was bought, as `YYYY-MM-DD`. Distinct from `first_used`, which is
	 *  when Tapedeck first saw it — most gear is owned well before it is
	 *  catalogued, and burn-in hours are counted from the purchase. */
	let gBought = '';
	let savingGear = false;
	let confirmGear: Equipment | null = null;
	// Set when the gear dialog was opened from inside a chain component row, so
	// the new gear drops straight into the slot that asked for it.
	let gearForComp: number | null = null;

	function ctxClass(ctx: string): string {
		const map: Record<string, string> = { active: 'tag-accent-2', 'active-mobile': 'tag-accent-2', passive: 'tag-accent' };
		return 'tag ' + (map[ctx] ?? 'tag-neutral');
	}

	async function load() {
		try {
			const [c, g, b, d] = await Promise.all([api.getChains(), api.getEquipment(), api.getBindings(), api.getDevices()]);
			chains = c.chains;
			equipment = g.equipment;
			bindings = b.bindings;
			devices = d.devices;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
		}
		loading = false;
	}

	async function assignBinding(b: Binding, value: string) {
		const cid = value ? Number(value) : null;
		try {
			await api.setBinding(b.identifier, cid);
			bindings = bindings.map((x) => (x.id === b.id ? { ...x, chain_id: cid } : x));
		} catch (e) { alert('Failed to set binding: ' + e); }
	}
	async function removeBinding(b: Binding) {
		try { await api.deleteBinding(b.id); bindings = bindings.filter((x) => x.id !== b.id); } catch (e) { alert('Delete failed: ' + e); }
	}

	async function assignDevice(d: Device, value: string) {
		const cid = value ? Number(value) : null;
		try {
			await api.setDeviceChain(d.id, cid);
			devices = devices.map((x) => (x.id === d.id ? { ...x, default_chain_id: cid } : x));
		} catch (e) { alert('Failed to set device chain: ' + e); }
	}

	// ── chain editor ──
	function newChain() {
		chainId = null; cName = ''; cDesc = ''; cContext = 'active'; cIcon = null;
		cComps = [blankComp('source')];
		chainOpen = true;
	}
	function editChain(c: SignalChain) {
		chainId = c.id; cName = c.name; cDesc = c.description ?? ''; cContext = c.listening_context;
		cIcon = c.icon ?? null;
		cComps = c.components?.length ? c.components.map((x) => ({ ...x })) : [blankComp('source')];
		chainOpen = true;
	}
	/** Most second chains are the first with one box swapped. */
	function duplicateChain(c: SignalChain) {
		chainId = null;
		cName = `${c.name} (copy)`; cDesc = c.description ?? ''; cContext = c.listening_context;
		cIcon = c.icon ?? null;
		cComps = c.components?.length ? c.components.map((x) => ({ ...x })) : [blankComp('source')];
		chainOpen = true;
	}
	const blankComp = (type: string): ChainComponent => ({ type, name: '', detail: null, equipment_id: null });
	function addComp() { cComps = [...cComps, blankComp('transducer')]; }
	function rmComp(i: number) { cComps = cComps.filter((_, j) => j !== i); }

	/** What the gear picker shows for a component: owned gear, or free text. */
	const compValue = (c: ChainComponent) => (c.equipment_id != null ? `eq:${c.equipment_id}` : 'custom');

	function pickGear(i: number, value: string) {
		if (value === 'new') { gearForComp = i; newGear(); return; }
		if (value === 'all') { showAllGear = { ...showAllGear, [i]: true }; return; }
		if (value === 'custom') {
			cComps[i] = { ...cComps[i], equipment_id: null };
			cComps = cComps;
			return;
		}
		const g = equipment.find((e) => e.id === Number(value.slice(3)));
		if (!g) return;
		cComps[i] = {
			...cComps[i],
			equipment_id: g.id,
			name: g.name,
			type: componentTypeForGear[g.equipment_type] ?? cComps[i].type
		};
		cComps = cComps;
	}

	// Gear already in the chain shouldn't be offered again in the other slots —
	// a chain with the same DAC twice is a typo, not a setup.
	//
	// Reactive rather than a plain function on purpose: Svelte tracks the
	// dependencies of `availableGear(i)` as the *identifier*, not the variables
	// the body closes over, so a plain function never re-renders the options
	// when `equipment` changes — gear added from inside this dialog would land
	// in a dropdown that has no option for it, and the select would blank out.
	//
	// Also filtered to gear that fits the slot's component type — picking "DAC"
	// and being shown every pair of headphones is what made this unusable.
	// `showAllGear` opts out per row, for gear typed unusually.
	let showAllGear: Record<number, boolean> = {};
	$: availableGear = ((_eq: Equipment[], _cc: ChainComponent[], _all: Record<number, boolean>) =>
		(i: number): Equipment[] => {
			const taken = new Set(
				_cc.filter((_, j) => j !== i).map((c) => c.equipment_id).filter((x) => x != null)
			);
			const slot = _cc[i]?.type ?? '';
			return _eq.filter(
				(e) => !taken.has(e.id) && (_all[i] || gearFitsSlot(e.equipment_type, slot))
			);
		})(equipment, cComps, showAllGear);

	/** How much gear the slot filter is hiding, so the escape hatch is honest. */
	$: hiddenGearCount = ((_eq: Equipment[], _cc: ChainComponent[], _all: Record<number, boolean>) =>
		(i: number): number => {
			if (_all[i]) return 0;
			const taken = new Set(
				_cc.filter((_, j) => j !== i).map((c) => c.equipment_id).filter((x) => x != null)
			);
			const slot = _cc[i]?.type ?? '';
			return _eq.filter(
				(e) => !taken.has(e.id) && !gearFitsSlot(e.equipment_type, slot)
			).length;
		})(equipment, cComps, showAllGear);

	async function saveChain() {
		if (!cName.trim()) return;
		savingChain = true;
		const data: ChainInput = {
			name: cName.trim(), description: cDesc.trim() || undefined,
			listening_context: cContext,
			icon: cIcon,
			components: cComps.filter((c) => c.name.trim()).map((c) => ({ ...c, detail: c.detail || null }))
		};
		try {
			if (chainId == null) await api.createChain(data);
			else await api.updateChain(chainId, data);
			chainOpen = false;
			await load();
		} catch (e) { alert('Failed to save chain: ' + e); }
		savingChain = false;
	}
	async function deleteChain(c: SignalChain) {
		try { await api.deleteChain(c.id); chains = chains.filter((x) => x.id !== c.id); } catch (e) { alert('Delete failed: ' + e); }
		confirmChain = null;
	}

	// ── equipment editor ──
	function newGear() {
		gearId = null; gName = ''; gType = 'headphones'; gBrand = ''; gModel = ''; gNotes = '';
		gBought = '';
		gearOpen = true;
	}
	function editGear(e: Equipment) {
		gearId = e.id; gName = e.name; gType = e.equipment_type; gBrand = e.brand ?? ''; gModel = e.model ?? '';
		// Read back rather than blanked. The save below writes every field, so a
		// field this dialog does not load is a field editing the brand silently
		// erases — which is exactly what an edit did to a note before this.
		gNotes = e.notes ?? '';
		gBought = toDateInput(e.purchased_at);
		gearOpen = true;
	}
	function closeGear() { gearOpen = false; gearForComp = null; }

	async function saveGear() {
		if (!gName.trim()) return;
		savingGear = true;
		const data: EquipmentInput = {
			name: gName.trim(), type: gType,
			brand: gBrand.trim() || undefined, model: gModel.trim() || undefined,
			notes: gNotes.trim() || undefined,
			purchased_at: fromDateInput(gBought)
		};
		try {
			const result = gearId == null ? await api.createEquipment(data) : await api.updateEquipment(gearId, data);
			// Refresh gear without reloading chains — the chain editor may be open
			// behind this dialog with unsaved edits in it.
			equipment = (await api.getEquipment()).equipment;
			if (gearForComp != null) {
				const slot = gearForComp;
				// Let the new <option> reach the DOM before selecting it; a select
				// can't hold a value whose option doesn't exist yet.
				await tick();
				pickGear(slot, `eq:${result.id}`);
			} else {
				chains = (await api.getChains()).chains;
			}
			closeGear();
		} catch (e) { alert('Failed to save equipment: ' + e); }
		savingGear = false;
	}
	async function deleteGear(e: Equipment) {
		try { await api.deleteEquipment(e.id); equipment = equipment.filter((x) => x.id !== e.id); } catch (err) { alert('Delete failed: ' + err); }
		confirmGear = null;
	}
	/** Chains that name this gear — shown in the table and before deleting it.
	 *  Reactive for the same reason as `availableGear`. */
	$: chainsUsing = ((_ch: SignalChain[]) => (e: Equipment) =>
		_ch.filter((c) => c.components?.some((x) => x.equipment_id === e.id)))(chains);

	// A `[[gear:…]]` shortcode in a liner note links to /chains?gear=<id>. There
	// is no gear detail page yet, so the link highlights the row it means rather
	// than dropping you at the top of a long page with nothing to look at.
	let highlightGear: number | null = null;

	onMount(async () => {
		const g = new URLSearchParams(window.location.search).get('gear');
		highlightGear = g ? Number(g) : null;
		await load();
		if (highlightGear != null) {
			await tick();
			document
				.getElementById(`gear-${highlightGear}`)
				?.scrollIntoView({ block: 'center', behavior: 'smooth' });
		}
	});
</script>

<svelte:head><title>Signal Chains — Tapedeck</title></svelte:head>

<div style="display:flex;align-items:flex-end;justify-content:space-between;gap:16px;margin-bottom:24px;flex-wrap:wrap;">
	<div>
		<h1 class="h-display" style="font-size:38px;">Signal Chains</h1>
		<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;">The path your music takes, from source to ears.</p>
	</div>
	<button class="btn btn-primary" on:click={newChain}>+ New chain</button>
</div>

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
							<div class="h-display" style="font-size:20px;line-height:1.15;display:flex;align-items:center;gap:9px;">
								<ChainIcon name={c.icon} size={19} title={c.name} />{c.name}
							</div>
							{#if c.description}<div style="font-size:13px;color:var(--td-muted);margin-top:4px;">{c.description}</div>{/if}
						</div>
						<div style="display:flex;align-items:center;gap:8px;flex:none;">
							<span class={ctxClass(c.listening_context)}>{c.listening_context}</span>
							<button class="icon-btn" title="Duplicate" on:click={() => duplicateChain(c)} aria-label="Duplicate chain">
								<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/></svg>
							</button>
							<button class="icon-btn" title="Edit" on:click={() => editChain(c)} aria-label="Edit chain">
								<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/></svg>
							</button>
							<button class="icon-btn danger" title="Delete" on:click={() => (confirmChain = c)} aria-label="Delete chain">
								<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16"/><path d="M10 11v6M14 11v6"/><path d="M6 7l1 13h10l1-13"/><path d="M9 7V4h6v3"/></svg>
							</button>
						</div>
					</div>
					{#if c.components?.length}
						<div style="display:flex;align-items:center;gap:6px;flex-wrap:wrap;">
							{#each c.components as step, i}
								<span style="display:inline-flex;flex-direction:column;background:var(--td-surface2);border:1px solid var(--td-border);border-radius:12px;padding:7px 12px;">
									<span style="font-size:10px;letter-spacing:.08em;text-transform:uppercase;color:var(--td-muted-3);">{compTypeLabel(step.type)}</span>
									<span style="font-size:13.5px;font-weight:600;display:flex;align-items:center;gap:5px;">
										{step.name}
										{#if step.equipment_id != null}<span title="From your equipment" style="color:var(--td-accent);font-size:10px;">●</span>{/if}
									</span>
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
				No signal chains yet — <button class="btn btn-ghost" on:click={newChain}>create your first</button>
			</div>
		{/if}
	</div>

	<div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:12px;">
		<div>
			<h3 class="h-display" style="font-size:19px;">Equipment</h3>
			<p style="margin:4px 0 0;color:var(--td-muted);font-size:13.5px;">Your gear. Anything here can be dropped into a chain from a dropdown.</p>
		</div>
		<button class="btn btn-secondary" on:click={newGear}>+ Add gear</button>
	</div>
	<div class="panel" style="border-radius:22px;overflow:hidden;padding:6px 20px 14px;margin-bottom:34px;">
		{#if equipment.length}
			<table class="table">
				<thead><tr><th>Gear</th><th>Type</th><th>Brand</th><th>In chains</th><th>Owned</th><th style="text-align:right;">Hours</th><th></th></tr></thead>
				<tbody>
					{#each equipment as e (e.id)}
						<tr id={`gear-${e.id}`} class:highlight={highlightGear === e.id}>
							<td style="font-weight:600;">{e.name}</td>
							<td style="color:var(--td-muted);">{gearTypeLabel(e.equipment_type)}</td>
							<td style="color:var(--td-muted);">{e.brand ?? '—'}</td>
							<td style="color:var(--td-muted);font-size:12.5px;">{chainsUsing(e).map((c) => c.name).join(', ') || '—'}</td>
							<td style="color:var(--td-muted);font-size:12.5px;" title={e.purchased_at ? formatDateTime(e.purchased_at).split(',')[0] : ''}>
								{e.purchased_at ? ownedFor(e.purchased_at) : '—'}
							</td>
							<td style="text-align:right;color:var(--td-good-fg);font-weight:600;">{formatHours(e.total_hours)}</td>
							<td style="text-align:right;white-space:nowrap;">
								<button class="icon-btn" title="Edit" on:click={() => editGear(e)} aria-label="Edit gear">
									<svg viewBox="0 0 24 24" width="15" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/></svg>
								</button>
								<button class="icon-btn danger" title="Delete" on:click={() => (confirmGear = e)} aria-label="Delete gear">
									<svg viewBox="0 0 24 24" width="15" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16"/><path d="M10 11v6M14 11v6"/><path d="M6 7l1 13h10l1-13"/><path d="M9 7V4h6v3"/></svg>
								</button>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{:else}
			<p style="color:var(--td-muted);font-size:14px;padding:16px 0;margin:0;">No equipment yet — <button class="btn btn-ghost" on:click={newGear}>add your first</button></p>
		{/if}
	</div>

	<!-- Submitting devices → default chain -->
	<div style="margin-bottom:12px;">
		<h3 class="h-display" style="font-size:19px;">Devices</h3>
		<p style="margin:4px 0 0;color:var(--td-muted);font-size:13.5px;">Machines that have submitted listens. The last fallback — a token's chain wins over this.</p>
	</div>
	<div class="panel" style="border-radius:22px;padding:18px 20px;margin-bottom:34px;">
		{#if devices.length}
			{#each devices as d (d.id)}
				<div style="display:flex;align-items:center;gap:12px;padding:10px 0;border-bottom:1px solid var(--td-border);flex-wrap:wrap;">
					<div style="flex:1;min-width:160px;">
						<div style="font-weight:600;font-size:14px;display:flex;align-items:center;gap:8px;">
							{d.name ?? d.machine_id}
							{#if d.platform}<span class="tag tag-neutral">{d.platform}</span>{/if}
						</div>
						<div style="font-size:12px;color:var(--td-muted-2);margin-top:2px;">{d.total_listens} listens · last seen {timeAgo(d.last_seen)}</div>
					</div>
					<select class="input" style="width:auto;min-width:180px;" value={d.default_chain_id ?? ''} on:change={(e) => assignDevice(d, e.currentTarget.value)}>
						<option value="">— no default —</option>
						{#each chains as c}<option value={c.id}>{c.name}</option>{/each}
					</select>
				</div>
			{/each}
		{:else}
			<p style="color:var(--td-muted);font-size:14px;margin:0;">No devices yet. They appear once a client submits a listen with a machine id.</p>
		{/if}
	</div>

	<!-- Outputs → chain bindings (auto-learned) -->
	<div style="margin-bottom:12px;">
		<h3 class="h-display" style="font-size:19px;">Outputs</h3>
		<p style="margin:4px 0 0;color:var(--td-muted);font-size:13.5px;">Map each playback output to a chain once — every future listen through it attributes automatically.</p>
	</div>
	<div class="panel" style="border-radius:22px;padding:18px 20px;margin-bottom:34px;">
		{#if bindings.length}
			{#if unmapped.length}
				<div style="font-size:11px;letter-spacing:.1em;text-transform:uppercase;color:var(--td-accent);margin-bottom:8px;">Needs assignment</div>
			{/if}
			{#each [...unmapped, ...mapped] as b (b.id)}
				<div style="display:flex;align-items:center;gap:12px;padding:10px 0;border-bottom:1px solid var(--td-border);flex-wrap:wrap;">
					<div style="flex:1;min-width:160px;">
						<div style="font-weight:600;font-size:14px;display:flex;align-items:center;gap:8px;">
							{b.identifier}
							{#if b.interface}<span class="tag tag-neutral">{b.interface}</span>{/if}
							{#if b.chain_id == null}<span class="tag tag-accent">unmapped</span>{/if}
						</div>
						<div style="font-size:12px;color:var(--td-muted-2);margin-top:2px;">{b.hits} plays · last seen {timeAgo(b.last_seen)}</div>
					</div>
					<select class="input" style="width:auto;min-width:180px;" value={b.chain_id ?? ''} on:change={(e) => assignBinding(b, e.currentTarget.value)}>
						<option value="">— unmapped —</option>
						{#each chains as c}<option value={c.id}>{c.name}</option>{/each}
					</select>
					<button class="icon-btn danger" title="Forget this output" on:click={() => removeBinding(b)} aria-label="Delete binding">✕</button>
				</div>
			{/each}
		{:else}
			<p style="color:var(--td-muted);font-size:14px;margin:0;">No outputs detected yet. They appear here automatically once a client reports its output device (USB DAC, Bluetooth, headphone jack…).</p>
		{/if}
	</div>
{/if}

<!-- Chain editor dialog -->
{#if chainOpen}
	<div class="dialog-backdrop" on:click={() => (chainOpen = false)} on:keydown={() => {}} role="presentation">
		<div class="dialog" style="width:min(620px,100%);" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">{chainId == null ? 'New signal chain' : 'Edit chain'}</div>
			<div class="dialog-body" style="max-height:70vh;overflow-y:auto;">
				<div style="display:flex;flex-direction:column;gap:13px;">
					<div class="field"><label for="cn">Name</label><input id="cn" class="input" bind:value={cName} placeholder="Desktop Reference"></div>
					<div class="field"><label for="cd">Description</label><input id="cd" class="input" bind:value={cDesc} placeholder="Primary desk setup"></div>
					<div class="field"><label for="cx">Listening context</label>
						<select id="cx" class="input" bind:value={cContext}>{#each contexts as ct}<option value={ct}>{ct}</option>{/each}</select>
					</div>
					<div style="margin-bottom:14px;">
						<div style="display:flex;align-items:baseline;justify-content:space-between;gap:10px;margin-bottom:7px;">
							<span style="font-size:13px;color:var(--td-muted);">Icon</span>
							<!-- The set is large enough now that a wall of glyphs needs to
							     say what the hovered-or-chosen one is; a tooltip alone means
							     hunting. -->
							<span style="font-size:12px;color:var(--td-muted-2);">
								{cIcon ? CHAIN_ICON_LABELS[cIcon] ?? cIcon : 'No icon'}
							</span>
						</div>
						<div class="icon-grid">
							<button type="button" class="icon-pick" class:on={!cIcon} title="No icon"
								on:click={() => (cIcon = null)}><ChainIcon name={null} size={17} /></button>
							{#each CHAIN_ICONS as ic}
								<button type="button" class="icon-pick" class:on={cIcon === ic}
									title={CHAIN_ICON_LABELS[ic]} on:click={() => (cIcon = ic)}>
									<ChainIcon name={ic} size={17} />
								</button>
							{/each}
						</div>
					</div>
					<div class="field">
						<div style="font-size:12px;margin-bottom:5px;color:var(--td-muted);">Components (source → ears)</div>
						<div style="display:flex;flex-direction:column;gap:8px;">
							{#each cComps as comp, i}
								<div style="display:flex;gap:6px;align-items:center;flex-wrap:wrap;">
									<select class="input" style="width:170px;flex:none;" bind:value={comp.type}>
										{#each compTypes as ct}<option value={ct.v}>{ct.label}</option>{/each}
									</select>
									<select class="input" style="flex:1;min-width:150px;" value={compValue(comp)} on:change={(e) => pickGear(i, e.currentTarget.value)}>
										{#if availableGear(i).length}
											<optgroup label="Your gear">
												{#each availableGear(i) as g}<option value={`eq:${g.id}`}>{g.name}{g.brand ? ` — ${g.brand}` : ''}</option>{/each}
											</optgroup>
										{/if}
										<option value="custom">Something else…</option>
										<option value="new">＋ Add new gear…</option>
										{#if hiddenGearCount(i)}
											<option value="all">Show all gear ({hiddenGearCount(i)} hidden)…</option>
										{/if}
									</select>
									{#if comp.equipment_id == null}
										<input class="input" style="flex:1;min-width:150px;" bind:value={comp.name} placeholder="e.g. a friend's Bifrost">
									{/if}
									<button class="icon-btn danger" on:click={() => rmComp(i)} aria-label="Remove component">✕</button>
								</div>
							{/each}
							<button class="btn btn-ghost" style="align-self:flex-start;" on:click={addComp}>+ Add component</button>
						</div>
					</div>
				</div>
			</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (chainOpen = false)}>Cancel</button>
				<button class="btn btn-primary" on:click={saveChain} disabled={savingChain || !cName.trim()}>{savingChain ? 'Saving…' : 'Save chain'}</button>
			</div>
		</div>
	</div>
{/if}

<!-- Equipment editor dialog -->
{#if gearOpen}
	<div class="dialog-backdrop" on:click={closeGear} on:keydown={() => {}} role="presentation">
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">{gearId == null ? 'Add equipment' : 'Edit equipment'}</div>
			<div class="dialog-body">
				<div style="display:flex;flex-direction:column;gap:13px;">
					<div class="field"><label for="gn">Name</label><input id="gn" class="input" bind:value={gName} placeholder="HD 650"></div>
					<div class="field"><label for="gt">Type</label>
						<select id="gt" class="input" bind:value={gType}>
							{#each gearTypeOptions as t}<option value={t.v}>{t.label}</option>{/each}
						</select>
					</div>
					<div style="display:flex;gap:12px;flex-wrap:wrap;">
						<div class="field" style="flex:1;min-width:160px;"><label for="gb">Brand</label><input id="gb" class="input" bind:value={gBrand} placeholder="Sennheiser"></div>
						<div class="field" style="flex:1;min-width:160px;"><label for="gm">Model</label><input id="gm" class="input" bind:value={gModel}></div>
					</div>
					<div style="display:flex;gap:12px;flex-wrap:wrap;">
						<div class="field" style="flex:1;min-width:160px;">
							<label for="gp">Bought on</label>
							<input id="gp" class="input" type="date" bind:value={gBought}>
						</div>
						<div class="field" style="flex:1;min-width:160px;align-self:flex-end;">
							<!-- Not the same as "first seen". Gear is usually owned for a
							     while before anyone catalogues it, and burn-in hours are
							     counted from the purchase. -->
							<p style="margin:0 0 9px;font-size:12px;color:var(--td-muted-2);">
								{gBought ? `Owned ${ownedFor(fromDateInput(gBought))}.` : 'Optional — leave blank if you are guessing.'}
							</p>
						</div>
					</div>
					<div class="field"><label for="go">Notes</label><input id="go" class="input" bind:value={gNotes}></div>
				</div>
			</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={closeGear}>Cancel</button>
				<button class="btn btn-primary" on:click={saveGear} disabled={savingGear || !gName.trim()}>{savingGear ? 'Saving…' : gearForComp != null ? 'Save & use' : 'Save'}</button>
			</div>
		</div>
	</div>
{/if}

<!-- Confirm deletes -->
{#if confirmChain}
	<div class="dialog-backdrop" on:click={() => (confirmChain = null)} on:keydown={() => {}} role="presentation">
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">Delete “{confirmChain.name}”?</div>
			<div class="dialog-body" style="color:var(--td-nav-fg);">Listens already tagged with this chain keep their data; the chain definition is removed.</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (confirmChain = null)}>Cancel</button>
				<button class="btn btn-primary" style="background:#b2622d;" on:click={() => confirmChain && deleteChain(confirmChain)}>Delete</button>
			</div>
		</div>
	</div>
{/if}
{#if confirmGear}
	<div class="dialog-backdrop" on:click={() => (confirmGear = null)} on:keydown={() => {}} role="presentation">
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">Delete “{confirmGear.name}”?</div>
			<div class="dialog-body" style="color:var(--td-nav-fg);">
				This removes the equipment record and its logged hours.
				{#if chainsUsing(confirmGear).length}
					<div style="margin-top:8px;">Used by {chainsUsing(confirmGear).map((c) => c.name).join(', ')} — those chains keep the name as plain text.</div>
				{/if}
			</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (confirmGear = null)}>Cancel</button>
				<button class="btn btn-primary" style="background:#b2622d;" on:click={() => confirmGear && deleteGear(confirmGear)}>Delete</button>
			</div>
		</div>
	</div>
{/if}

<style>
	/* A grid of toggles rather than a dropdown: a glyph is quicker to recognise
	   than a word to read, which is the entire point of having them. A fixed
	   column count keeps the families in `CHAIN_ICONS` roughly in rows, and the
	   cap stops the set pushing the rest of the dialog off a short screen. */
	.icon-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(34px, 1fr));
		gap: 7px;
		max-height: 168px;
		overflow-y: auto;
		padding-right: 2px;
	}
	.icon-pick {
		display: flex; align-items: center; justify-content: center;
		width: 34px; height: 34px; border-radius: 10px; cursor: pointer;
		border: 1px solid var(--td-border); background: transparent;
		color: var(--td-muted-2); transition: border-color .15s, color .15s, background .15s;
	}
	.icon-pick:hover { color: var(--td-text); border-color: var(--td-muted-3); }
	.icon-pick.on { border-color: var(--td-accent); color: var(--td-accent); background: color-mix(in srgb, var(--td-accent) 10%, transparent); }

	.spin { animation: td-spin 0.7s linear infinite; }
	.icon-btn {
		border: none; background: transparent; cursor: pointer; color: var(--td-muted-2);
		width: 30px; height: 30px; border-radius: 8px; display: inline-flex; align-items: center; justify-content: center;
	}
	.icon-btn:hover { background: var(--td-nav-hover); color: var(--td-text); }
	.icon-btn.danger:hover { color: #b2622d; }
	tr.highlight {
		background: color-mix(in srgb, var(--td-accent) 12%, transparent);
	}
</style>
