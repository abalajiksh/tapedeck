<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { SignalChain, Equipment, ChainComponent, ChainInput, EquipmentInput, Binding } from '$lib/api';
	import { formatHours, timeAgo } from '$lib/utils';

	let chains: SignalChain[] = [];
	let equipment: Equipment[] = [];
	let bindings: Binding[] = [];
	let loading = true;
	let error = '';

	$: unmapped = bindings.filter((b) => b.chain_id == null);
	$: mapped = bindings.filter((b) => b.chain_id != null);
	$: chainName = (id: number | null) => chains.find((c) => c.id === id)?.name ?? '—';

	const contexts = ['active', 'active-mobile', 'passive', 'background', 'unknown'];
	// Must stay in sync with `ComponentType` in src/models.rs — serde rejects
	// anything else with a 422, so an extra option here is a broken save.
	const compTypes = ['source', 'transport', 'dac', 'amp', 'transducer', 'network', 'bluetooth'];

	// Free text on the backend (`equipment_type` is a plain string), so this list
	// is a UI convenience. Unknown values from older rows are preserved below.
	const gearTypes = [
		{ v: 'headphones', label: 'Headphones' },
		{ v: 'iem', label: 'IEM' },
		{ v: 'speakers', label: 'Speakers' },
		{ v: 'dac', label: 'DAC' },
		{ v: 'amp', label: 'Amplifier' },
		{ v: 'dac-amp', label: 'DAC / Amp combo' },
		{ v: 'preamp', label: 'Preamp' },
		{ v: 'source', label: 'Source / Streamer' },
		{ v: 'transport', label: 'Transport' },
		{ v: 'turntable', label: 'Turntable' },
		{ v: 'interface', label: 'Audio interface' },
		{ v: 'cable', label: 'Cable' },
		{ v: 'other', label: 'Other' }
	];
	// Keep whatever an existing row already has, even if it predates this list.
	$: gearTypeOptions = gearTypes.some((t) => t.v === gType)
		? gearTypes
		: [...gearTypes, { v: gType, label: gType }];
	// "iem" and "dac-amp" don't survive text-transform:capitalize intact.
	const gearTypeLabel = (v: string) => gearTypes.find((t) => t.v === v)?.label ?? v;

	// chain editor
	let chainOpen = false;
	let chainId: number | null = null; // null = create
	let cName = '', cDesc = '', cContext = 'active';
	let cComps: ChainComponent[] = [];
	let savingChain = false;
	let confirmChain: SignalChain | null = null;

	// equipment editor
	let gearOpen = false;
	let gearId: number | null = null;
	let gName = '', gType = 'headphones', gBrand = '', gModel = '', gNotes = '';
	let savingGear = false;
	let confirmGear: Equipment | null = null;

	function ctxClass(ctx: string): string {
		const map: Record<string, string> = { active: 'tag-accent-2', 'active-mobile': 'tag-accent-2', passive: 'tag-accent' };
		return 'tag ' + (map[ctx] ?? 'tag-neutral');
	}

	async function load() {
		try {
			const [c, g, b] = await Promise.all([api.getChains(), api.getEquipment(), api.getBindings()]);
			chains = c.chains;
			equipment = g.equipment;
			bindings = b.bindings;
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

	// ── chain editor ──
	function newChain() {
		chainId = null; cName = ''; cDesc = ''; cContext = 'active';
		cComps = [{ type: 'source', name: '', detail: '' }];
		chainOpen = true;
	}
	function editChain(c: SignalChain) {
		chainId = c.id; cName = c.name; cDesc = c.description ?? ''; cContext = c.listening_context;
		cComps = c.components?.length ? c.components.map((x) => ({ ...x })) : [{ type: 'source', name: '', detail: '' }];
		chainOpen = true;
	}
	function addComp() { cComps = [...cComps, { type: 'other', name: '', detail: '' }]; }
	function rmComp(i: number) { cComps = cComps.filter((_, j) => j !== i); }

	async function saveChain() {
		if (!cName.trim()) return;
		savingChain = true;
		const data: ChainInput = {
			name: cName.trim(), description: cDesc.trim() || undefined,
			listening_context: cContext,
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
		gearOpen = true;
	}
	function editGear(e: Equipment) {
		gearId = e.id; gName = e.name; gType = e.equipment_type; gBrand = e.brand ?? ''; gModel = e.model ?? ''; gNotes = '';
		gearOpen = true;
	}
	async function saveGear() {
		if (!gName.trim()) return;
		savingGear = true;
		const data: EquipmentInput = {
			name: gName.trim(), type: gType,
			brand: gBrand.trim() || undefined, model: gModel.trim() || undefined, notes: gNotes.trim() || undefined
		};
		try {
			if (gearId == null) await api.createEquipment(data);
			else await api.updateEquipment(gearId, data);
			gearOpen = false;
			await load();
		} catch (e) { alert('Failed to save equipment: ' + e); }
		savingGear = false;
	}
	async function deleteGear(e: Equipment) {
		try { await api.deleteEquipment(e.id); equipment = equipment.filter((x) => x.id !== e.id); } catch (err) { alert('Delete failed: ' + err); }
		confirmGear = null;
	}

	onMount(load);
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
							<div class="h-display" style="font-size:20px;line-height:1.15;">{c.name}</div>
							{#if c.description}<div style="font-size:13px;color:var(--td-muted);margin-top:4px;">{c.description}</div>{/if}
						</div>
						<div style="display:flex;align-items:center;gap:8px;flex:none;">
							<span class={ctxClass(c.listening_context)}>{c.listening_context}</span>
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
				No signal chains yet — <button class="btn btn-ghost" on:click={newChain}>create your first</button>
			</div>
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

	<div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:12px;">
		<h3 class="h-display" style="font-size:19px;">Equipment</h3>
		<button class="btn btn-secondary" on:click={newGear}>+ Add gear</button>
	</div>
	<div class="panel" style="border-radius:22px;overflow:hidden;padding:6px 20px 14px;">
		{#if equipment.length}
			<table class="table">
				<thead><tr><th>Gear</th><th>Type</th><th>Brand</th><th style="text-align:right;">Hours</th><th></th></tr></thead>
				<tbody>
					{#each equipment as e (e.id)}
						<tr>
							<td style="font-weight:600;">{e.name}</td>
							<td style="color:var(--td-muted);">{gearTypeLabel(e.equipment_type)}</td>
							<td style="color:var(--td-muted);">{e.brand ?? '—'}</td>
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
{/if}

<!-- Chain editor dialog -->
{#if chainOpen}
	<div class="dialog-backdrop" on:click={() => (chainOpen = false)} on:keydown={() => {}} role="presentation">
		<div class="dialog" style="width:min(560px,100%);" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">{chainId == null ? 'New signal chain' : 'Edit chain'}</div>
			<div class="dialog-body" style="max-height:70vh;overflow-y:auto;">
				<div style="display:flex;flex-direction:column;gap:13px;">
					<div class="field"><label for="cn">Name</label><input id="cn" class="input" bind:value={cName} placeholder="Desktop Reference"></div>
					<div class="field"><label for="cd">Description</label><input id="cd" class="input" bind:value={cDesc} placeholder="Primary desk setup"></div>
					<div class="field"><label for="cx">Listening context</label>
						<select id="cx" class="input" bind:value={cContext}>{#each contexts as ct}<option value={ct}>{ct}</option>{/each}</select>
					</div>
					<div class="field">
						<div style="font-size:12px;margin-bottom:5px;color:var(--td-muted);">Components (source → ears)</div>
						<div style="display:flex;flex-direction:column;gap:8px;">
							{#each cComps as comp, i}
								<div style="display:flex;gap:6px;align-items:center;">
									<select class="input" style="width:120px;flex:none;" bind:value={comp.type}>{#each compTypes as ct}<option value={ct}>{ct}</option>{/each}</select>
									<input class="input" style="flex:1;" bind:value={comp.name} placeholder="e.g. Schiit Mimir">
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
	<div class="dialog-backdrop" on:click={() => (gearOpen = false)} on:keydown={() => {}} role="presentation">
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
					<div class="field"><label for="go">Notes</label><input id="go" class="input" bind:value={gNotes}></div>
				</div>
			</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (gearOpen = false)}>Cancel</button>
				<button class="btn btn-primary" on:click={saveGear} disabled={savingGear || !gName.trim()}>{savingGear ? 'Saving…' : 'Save'}</button>
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
			<div class="dialog-body" style="color:var(--td-nav-fg);">This removes the equipment record and its logged hours.</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (confirmGear = null)}>Cancel</button>
				<button class="btn btn-primary" style="background:#b2622d;" on:click={() => confirmGear && deleteGear(confirmGear)}>Delete</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.spin { animation: td-spin 0.7s linear infinite; }
	.icon-btn {
		border: none; background: transparent; cursor: pointer; color: var(--td-muted-2);
		width: 30px; height: 30px; border-radius: 8px; display: inline-flex; align-items: center; justify-content: center;
	}
	.icon-btn:hover { background: var(--td-nav-hover); color: var(--td-text); }
	.icon-btn.danger:hover { color: #b2622d; }
</style>
