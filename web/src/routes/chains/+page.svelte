<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { api } from '$lib/api';
	import type { SignalChain, Equipment, ChainComponent, ChainInput, EquipmentInput, Binding, Device, ChainVariant, FoldSuggestion } from '$lib/api';
	import { formatHours, timeAgo, toDateInput, fromDateInput, ownedFor, formatDateTime } from '$lib/utils';
	import ChainIcon, { CHAIN_ICONS, CHAIN_ICON_LABELS } from '$lib/components/ChainIcon.svelte';

	let chains: SignalChain[] = [];
	let variants: ChainVariant[] = [];
	let equipment: Equipment[] = [];
	let bindings: Binding[] = [];
	let devices: Device[] = [];
	let loading = true;
	let error = '';

	$: unmapped = bindings.filter((b) => b.chain_id == null);
	$: mapped = bindings.filter((b) => b.chain_id != null);
	$: unmappedDevices = devices.filter((d) => d.default_chain_id == null);

	// ── Variants ──
	//
	// A chain with one step swapped. Two cartridges on one turntable, or the
	// same commute on different IEMs, are one chain and a choice — modelling
	// them as separate chains is what turns a modest gear collection into a page
	// of near-duplicates, and it is also what makes per-stylus hours
	// unanswerable, since "which chain" does not say what was fitted.
	$: variantsFor = (chainId: number | null | undefined) =>
		variants.filter((v) => v.chain_id === chainId);

	/** What a variant actually changes, for the chip's tooltip. */
	function overrideSummary(c: SignalChain, v: ChainVariant): string {
		if (!v.overrides.length) return 'Same as the chain';
		return v.overrides
			.map((o) => {
				const was = c.components?.[o.position]?.name;
				const now = o.name ?? equipment.find((e) => e.id === o.equipment_id)?.name ?? '—';
				return was ? `${was} → ${now}` : `+ ${now}`;
			})
			.join(', ');
	}

	// ── The role palette ──
	//
	// A categorical colour per component type, so a path can be read as a shape
	// rather than as a sentence — which is what lets a whole chain sit on one
	// row instead of a card. Hardcoded hex on purpose: these are the accent
	// family, which `app.css` declares theme-independent, and a categorical
	// scale has to stay stable across themes or the same DAC changes colour when
	// the lights go out.
	const ROLE_COLOUR: Record<string, string> = {
		source: '#c67139',
		transport: '#b2622d',
		dac: '#7a8a5e',
		amp: '#a97b3f',
		transducer: '#56633f',
		network: '#8c491a',
		bluetooth: '#9a8f7d'
	};
	const roleColour = (t: string) => ROLE_COLOUR[t] ?? '#9a8f7d';

	/** The role line above a step in the drawer.
	 *
	 *  The gear's own type is the more specific answer where the step was picked
	 *  from equipment — "IEM" and "Turntable" rather than the slot's broad
	 *  "Headphones / Speakers" and "Source". Falls back to the component type,
	 *  which is the only thing a free-text step has. */
	function roleLabel(step: ChainComponent): string {
		if (step.equipment_id != null) {
			const g = equipment.find((e) => e.id === step.equipment_id);
			if (g) return gearTypeLabel(g.equipment_type);
		}
		return compTypeLabel(step.type);
	}

	// ── The chain drawer ──
	//
	// Held as an id rather than the object: `load()` replaces every row, and a
	// drawer holding a stale object would show the chain as it was before the
	// variant you just added.
	let openChainId: number | null = null;
	$: openChain = chains.find((c) => c.id === openChainId) ?? null;
	// Sorted by hours: the chain you actually listen through is the one worth
	// putting at the top, and a page of chains ordered by creation date buries
	// it under whatever you set up most recently.
	$: chainRows = [...chains].sort((a, b) => b.total_hours - a.total_hours || a.name.localeCompare(b.name));
	$: loggingCount = chains.filter((c) => c.total_hours > 0).length;
	$: totalChainHours = chains.reduce((a, c) => a + c.total_hours, 0);

	function onKey(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			if (openChainId != null) openChainId = null;
		}
	}

	// The variant editor.
	let vOpen = false;
	let vChain: SignalChain | null = null;
	let vId: number | null = null;
	let vName = '';
	let vSlot = 0;
	let vGear: number | null = null;
	let vErr = '';
	let vSaving = false;

	function newVariant(c: SignalChain) {
		vChain = c;
		vId = null;
		vName = '';
		vErr = '';
		// The last step is the usual thing you swap — headphones, IEMs, a
		// cartridge at the other end. Not a rule, just the better default.
		vSlot = Math.max(0, (c.components?.length ?? 1) - 1);
		vGear = null;
		vOpen = true;
	}

	async function saveVariant() {
		if (!vChain?.id || !vName.trim()) { vErr = 'A variant needs a name.'; return; }
		if (vGear == null) { vErr = 'Pick the gear that goes in that slot.'; return; }
		vSaving = true;
		vErr = '';
		try {
			const overrides = [{
				position: vSlot,
				equipment_id: vGear,
				name: equipment.find((e) => e.id === vGear)?.name ?? null
			}];
			if (vId == null) await api.createVariant({ chain_id: vChain.id, name: vName.trim(), overrides });
			else await api.updateVariant(vId, { name: vName.trim(), overrides });
			vOpen = false;
			await load();
		} catch (e) {
			vErr = String(e).replace(/^API \d+: /, '');
		}
		vSaving = false;
	}

	async function removeVariant(v: ChainVariant) {
		// The listens keep their chain and fall back to its base components,
		// which is what a null variant means everywhere — so this is not the
		// destructive act deleting a chain would be, and needs no confirmation.
		try {
			await api.deleteVariant(v.id);
			await load();
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
	}

	// ── Folding a near-duplicate chain in ──
	//
	// Variants shipped forward-looking: build one, and listens tagged with it
	// from then on credit the gear fitted. But the case they were designed
	// against already existed as two chains — the same commute with different
	// IEMs — each holding real hours. Creating the variant did nothing to the
	// duplicate: it kept its own listens, its own total, and its own row on the
	// page whose whole complaint was near-duplicate rows.
	//
	// So the fold moves the listening across and deletes the chain, and the
	// hours that were split across two rows that could not be compared add up
	// on one.
	let dupes: FoldSuggestion[] = [];

	let fOpen = false;
	let fInto: SignalChain | null = null;      // the chain being kept
	let fFrom: number | null = null;           // the chain folding into it
	// Either a variant that exists, or a name to derive one from — never both,
	// which is why this is one control with a sentinel rather than two fields
	// that can disagree.
	let fVariant: number | 'new' = 'new';
	let fName = '';
	let fErr = '';
	let fBusy = false;
	let fDone: string | null = null;

	/** Chains that could fold into this one: everything else the user owns,
	 *  nearest wiring first, and never one carrying variants of its own. */
	$: foldable = (into: SignalChain | null) =>
		into?.id == null
			? []
			: chains
					.filter((c) => c.id !== into.id && !variants.some((v) => v.chain_id === c.id))
					.map((c) => ({
						chain: c,
						// `distance` comes from the server for the pairs it
						// considers near-duplicates; anything else is simply a
						// different rig and sorts last.
						dupe: dupes.find(
							(d) =>
								(d.base_chain_id === into.id && d.fold_chain_id === c.id) ||
								(d.fold_chain_id === into.id && d.base_chain_id === c.id)
						)
					}))
					.sort((a, b) => (a.dupe?.distance ?? 99) - (b.dupe?.distance ?? 99));

	function openFold(into: SignalChain, from?: number) {
		fInto = into;
		fErr = '';
		fDone = null;
		const options = foldable(into);
		fFrom = from ?? options[0]?.chain.id ?? null;
		pickFoldDefaults();
		fOpen = true;
	}

	/** Preselect the variant they already built for this pairing, if there is
	 *  one — that is the whole state this feature exists to resolve, and
	 *  offering to create a second variant wired identically would be the wrong
	 *  default in exactly the case that matters. */
	function pickFoldDefaults() {
		const d = dupes.find(
			(x) => x.base_chain_id === fInto?.id && x.fold_chain_id === fFrom
		);
		if (d?.matching_variant) {
			fVariant = d.matching_variant[0];
			fName = '';
		} else {
			fVariant = 'new';
			fName = chains.find((c) => c.id === fFrom)?.name ?? '';
		}
	}

	async function doFold() {
		if (fFrom == null || fInto?.id == null) { fErr = 'Pick a chain to fold in.'; return; }
		if (fVariant === 'new' && !fName.trim()) { fErr = 'Name the variant it becomes.'; return; }
		fBusy = true;
		fErr = '';
		try {
			const r = await api.foldChain(fFrom, {
				into_chain_id: fInto.id,
				...(fVariant === 'new' ? { variant_name: fName.trim() } : { variant_id: fVariant })
			});
			// Said rather than assumed: a fold that moved no listens is a real
			// outcome (an unused duplicate), and reporting "done" for both
			// hides the difference.
			fDone =
				`Folded into “${fInto.name}” as “${r.variant_name}”. ` +
				(r.listens
					? `${r.listens} listen${r.listens > 1 ? 's' : ''} moved` +
					  (r.sides ? `, ${r.sides} side${r.sides > 1 ? 's' : ''}` : '') +
					  (r.devices || r.tokens || r.bindings
							? `, and ${r.devices + r.tokens + r.bindings} attribution rule${
									r.devices + r.tokens + r.bindings > 1 ? 's' : ''
							  }`
							: '') +
					  '.'
					: 'It had no listening of its own.');
			await load();
		} catch (e) {
			fErr = String(e).replace(/^API \d+: /, '');
		}
		fBusy = false;
	}

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
		// A cartridge is not a turntable, and it is the one piece of gear people
		// own two of for the same deck — which is what chain variants exist for.
		{ v: 'cartridge', label: 'Cartridge / stylus' },
		{ v: 'headshell', label: 'Headshell' },
		{ v: 'phono', label: 'Phono stage' },
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

	// Why a piece of gear is not in a chain. Three answers cover it: it just
	// arrived, it is a spare, or someone else has it. Stored on `equipment.status`
	// and only ever shown for gear no chain names — on gear in daily use the
	// question does not arise.
	const gearStatuses = [
		{ v: 'new', label: 'New', colour: '#7a8a5e' },
		{ v: 'spare', label: 'Spare', colour: '#9a8f7d' },
		{ v: 'lent', label: 'Lent out', colour: '#c67139' }
	];
	const gearStatus = (v: string | null | undefined) => gearStatuses.find((s) => s.v === v);

	// Picking a piece of gear should fill in its chain position too — nobody
	// wants to tell the app twice that headphones go at the end. Guessed once on
	// selection and freely overridable, never re-applied afterwards.
	const componentTypeForGear: Record<string, string> = {
		headphones: 'transducer', iem: 'transducer', speakers: 'transducer',
		'builtin-speakers': 'transducer', earbuds: 'bluetooth',
		dac: 'dac', 'dac-amp': 'dac', interface: 'dac', receiver: 'dac',
		amp: 'amp', preamp: 'amp',
		source: 'source', turntable: 'source', phone: 'source',
		// A cartridge sits at the very start of the path, before anything
		// amplifies it; a phono stage is the first gain stage after it.
		cartridge: 'source', headshell: 'source', phono: 'amp',
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
	/** Carried through the dialog without a field of its own — the tag is set
	 *  from the gear table, but this dialog writes the whole record, so a value
	 *  it does not load is a value that editing the brand silently erases. That
	 *  is exactly what an edit used to do to `notes`. */
	let gStatus: string | null = null;
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
			const [c, g, b, d, v, dup] = await Promise.all([
				api.getChains(), api.getEquipment(), api.getBindings(), api.getDevices(), api.getVariants(),
				api.getChainDuplicates()
			]);
			chains = c.chains;
			variants = v.variants;
			dupes = dup.duplicates;
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
		try {
			await api.deleteChain(c.id);
			chains = chains.filter((x) => x.id !== c.id);
			if (openChainId === c.id) openChainId = null;
		} catch (e) { alert('Delete failed: ' + e); }
		confirmChain = null;
	}

	// ── equipment editor ──
	function newGear() {
		gearId = null; gName = ''; gType = 'headphones'; gBrand = ''; gModel = ''; gNotes = '';
		gBought = ''; gStatus = null;
		gearOpen = true;
	}
	function editGear(e: Equipment) {
		gearId = e.id; gName = e.name; gType = e.equipment_type; gBrand = e.brand ?? ''; gModel = e.model ?? '';
		// Read back rather than blanked. The save below writes every field, so a
		// field this dialog does not load is a field editing the brand silently
		// erases — which is exactly what an edit did to a note before this.
		gNotes = e.notes ?? '';
		gStatus = e.status ?? null;
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
			// `status` goes on the update only — the create endpoint has no field
			// for it and would drop it silently, which is worse than not sending
			// it. Carried through the dialog rather than edited here, so that
			// editing a brand doesn't clear the tag.
			const result = gearId == null
				? await api.createEquipment(data)
				: await api.updateEquipment(gearId, { ...data, status: gStatus });
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

	/** Tag a piece of gear from the table.
	 *
	 *  Sends the *whole* record, not just the status: `PATCH /equipment/:id` is
	 *  a whole-record replace, so a body carrying one field would clear the
	 *  brand, the model, the notes and the purchase date along with it. */
	async function setGearStatus(e: Equipment, status: string) {
		const next = status || null;
		try {
			await api.updateEquipment(e.id, {
				name: e.name, type: e.equipment_type,
				brand: e.brand ?? undefined, model: e.model ?? undefined,
				notes: e.notes ?? undefined,
				purchased_at: e.purchased_at,
				status: next
			});
			equipment = equipment.map((x) => (x.id === e.id ? { ...x, status: next } : x));
		} catch (err) { alert('Failed to tag gear: ' + err); }
	}

	async function deleteGear(e: Equipment) {
		try { await api.deleteEquipment(e.id); equipment = equipment.filter((x) => x.id !== e.id); } catch (err) { alert('Delete failed: ' + err); }
		confirmGear = null;
	}

	/** Chains that name this gear — shown in the table and before deleting it.
	 *
	 *  A variant override counts. A cartridge fitted only as a variant is
	 *  genuinely in use and logs hours through `equipment_seconds`, so leaving it
	 *  out of this listed it under "not in a chain yet" while it was visibly
	 *  accruing time. Reactive for the same reason as `availableGear`. */
	$: chainsUsing = ((_ch: SignalChain[], _vs: ChainVariant[]) => (e: Equipment) =>
		_ch.filter(
			(c) =>
				c.components?.some((x) => x.equipment_id === e.id) ||
				_vs.some((v) => v.chain_id === c.id && v.overrides.some((o) => o.equipment_id === e.id))
		))(chains, variants);

	$: inChainGear = equipment.filter((e) => chainsUsing(e).length > 0);
	$: spareGear = equipment.filter((e) => chainsUsing(e).length === 0);

	// The secondary panel. Three sections that used to be stacked full-height on
	// one page — the single biggest source of scrolling on this screen — behind
	// one control. Not remembered across visits: Equipment is the answer to
	// "what do I own", which is what people come here for.
	let secTab: 'equipment' | 'devices' | 'outputs' = 'equipment';

	// A `[[gear:…]]` shortcode in a liner note links to /chains?gear=<id>. There
	// is no gear detail page yet, so the link highlights the row it means rather
	// than dropping you at the top of a long page with nothing to look at.
	let highlightGear: number | null = null;

	onMount(async () => {
		const g = new URLSearchParams(window.location.search).get('gear');
		highlightGear = g ? Number(g) : null;
		await load();
		if (highlightGear != null) {
			// Equipment is the default tab, so the row is rendered by now.
			await tick();
			document
				.getElementById(`gear-${highlightGear}`)
				?.scrollIntoView({ block: 'center', behavior: 'smooth' });
		}
	});
</script>

<svelte:head><title>Signal Chains — Tapedeck</title></svelte:head>
<svelte:window on:keydown={onKey} />

<div style="display:flex;align-items:flex-end;justify-content:space-between;gap:16px;margin-bottom:22px;flex-wrap:wrap;">
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
	<!-- Summary strip: the three numbers this page is about, so the answer to
	     "how much of my listening is attributed" is above the fold rather than
	     something you total up by reading every row. -->
	<div class="tiles">
		<div class="tile">
			<div class="tile-big">{chains.length}</div>
			<div class="tile-label">signal chains</div>
			<div class="tile-sub">{loggingCount} logging listens</div>
		</div>
		<div class="tile">
			<div class="tile-big">{formatHours(totalChainHours)}</div>
			<div class="tile-label">logged across chains</div>
			<div class="tile-sub">all-time</div>
		</div>
		<div class="tile">
			<div class="tile-big">{equipment.length}</div>
			<div class="tile-label">pieces of gear</div>
			<div class="tile-sub">{spareGear.length} not in a chain</div>
		</div>
	</div>

	<!-- The nudge. Devices lead because every submitted listen goes through one,
	     where an output only exists if a client bothered to report it — so an
	     unassigned device is silently losing attribution right now, and an
	     unmapped output is a thing to tidy. One line either way; two stacked
	     banners is the clutter this page is being rescued from. -->
	{#if unmappedDevices.length}
		<button class="nudge" aria-label="Review devices with no default chain" on:click={() => (secTab = 'devices')}>
			<svg viewBox="0 0 24 24" width="17" fill="none" stroke="var(--td-accent)" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="flex:none;"><circle cx="12" cy="12" r="9"/><path d="M12 8h.01M11 12h1v4h1"/></svg>
			<span style="flex:1;font-size:13.5px;">
				{unmappedDevices.length} device{unmappedDevices.length > 1 ? 's have' : ' has'} no default chain —
				listens through {unmappedDevices.length > 1 ? 'them' : 'it'} won’t be attributed.
			</span>
			<span class="nudge-cta">Review →</span>
		</button>
	{:else if dupes.length}
		<!-- Two chains that are the same rig split their hours across two rows
		     that cannot be compared, which is wrong *now* rather than wrong for
		     future listens — but a device with no chain is losing attribution
		     outright, so it still leads. -->
		<button class="nudge" aria-label="Fold near-duplicate chains together"
			on:click={() => {
				const d = dupes[0];
				const into = chains.find((c) => c.id === d.base_chain_id);
				if (into) openFold(into, d.fold_chain_id);
			}}>
			<svg viewBox="0 0 24 24" width="17" fill="none" stroke="var(--td-accent)" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="flex:none;"><path d="M8 3v6l-4 3 4 3v6M16 3v6l4 3-4 3v6"/></svg>
			<span style="flex:1;font-size:13.5px;">
				{dupes.length === 1 ? '“' + dupes[0].fold_name + '” is' : dupes.length + ' chains are'}
				near-{dupes.length === 1 ? 'identical to “' + dupes[0].base_name + '”' : 'duplicates of another'} —
				fold {dupes.length === 1 ? 'it' : 'them'} in as {dupes.length === 1 ? 'a variant' : 'variants'} and the hours add up.
			</span>
			<span class="nudge-cta">Fold →</span>
		</button>
	{:else if unmapped.length}
		<button class="nudge" aria-label="Review unmapped outputs" on:click={() => (secTab = 'outputs')}>
			<svg viewBox="0 0 24 24" width="17" fill="none" stroke="var(--td-accent)" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="flex:none;"><circle cx="12" cy="12" r="9"/><path d="M12 8h.01M11 12h1v4h1"/></svg>
			<span style="flex:1;font-size:13.5px;">
				{unmapped.length} output{unmapped.length > 1 ? 's are' : ' is'} detected but unmapped —
				map {unmapped.length > 1 ? 'them' : 'it'} once and every future listen attributes itself.
			</span>
			<span class="nudge-cta">Review →</span>
		</button>
	{/if}

	<!-- Chain rows.
	     One line each, path included, instead of a card with three icon buttons
	     and a collapsible path. Eight chains used to be eight blocks; they are
	     now eight lines you can scan, and everything that was in the card lives
	     in the drawer a click away. -->
	<div class="rows">
		{#if chainRows.length}
			{#each chainRows as c (c.id)}
				<!-- Named explicitly: the row's text is a name, a path and a running
				     time in nested divs, which reads as a paragraph rather than as
				     what the control does. -->
				<button class="crow" class:on={openChainId === c.id}
					aria-label="Open {c.name}" aria-expanded={openChainId === c.id}
					on:click={() => (openChainId = c.id)}>
					<div class="cname">
						<div class="cname-t"><ChainIcon name={c.icon} size={16} title={c.name} />{c.name}</div>
						{#if c.description}<div class="cname-d">{c.description}</div>{/if}
					</div>
					<div class="cpath">
						{#if c.components?.length}
							{#each c.components as step, i}
								<span class="cstep">
									<span class="dot" style="background:{roleColour(step.type)};"></span>
									<span>{step.name}</span>
								</span>
								{#if i < c.components.length - 1}<span class="arrow">→</span>{/if}
							{/each}
						{:else}
							<span class="cnone">No steps yet</span>
						{/if}
						{#if variantsFor(c.id).length}
							<span class="setups" title="One chain, {variantsFor(c.id).length + 1} ways of wiring it">
								{variantsFor(c.id).length + 1} setups
							</span>
						{/if}
					</div>
					<div class="cend">
						<span class="chrs" class:zero={c.total_hours <= 0}>{formatHours(c.total_hours)}</span>
						<svg viewBox="0 0 24 24" width="17" fill="none" stroke="var(--td-muted-3)" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M9 6l6 6-6 6"/></svg>
					</div>
				</button>
			{/each}
		{:else}
			<div class="panel" style="border-radius:22px;padding:44px;text-align:center;color:var(--td-muted);">
				No signal chains yet — <button class="btn btn-ghost" on:click={newChain}>create your first</button>
			</div>
		{/if}
	</div>

	<!-- Equipment / Devices / Outputs behind one control. -->
	<div class="sechead">
		<div class="seg">
			<button class="seg-btn" class:on={secTab === 'equipment'} on:click={() => (secTab = 'equipment')}>Equipment · {equipment.length}</button>
			<button class="seg-btn" class:on={secTab === 'devices'} on:click={() => (secTab = 'devices')}>Devices · {devices.length}</button>
			<button class="seg-btn" class:on={secTab === 'outputs'} on:click={() => (secTab = 'outputs')}>Outputs · {bindings.length}</button>
		</div>
		{#if secTab === 'equipment'}
			<button class="btn btn-secondary" on:click={newGear}>+ Add gear</button>
		{/if}
	</div>

	{#if secTab === 'equipment'}
		{#if equipment.length}
			{#if inChainGear.length}
				<div class="panel sectable">
					<div class="secsub">In your chains</div>
					<div class="tablewrap">
					<table class="table">
						<thead><tr><th>Gear</th><th>Type</th><th>Brand</th><th>In chains</th><th>Owned</th><th style="text-align:right;">Hours</th><th></th></tr></thead>
						<tbody>
							{#each inChainGear as e (e.id)}
								<tr id={`gear-${e.id}`} class:highlight={highlightGear === e.id}>
									<td style="font-weight:600;">{e.name}</td>
									<td style="color:var(--td-muted);">{gearTypeLabel(e.equipment_type)}</td>
									<td style="color:var(--td-muted);">{e.brand ?? '—'}</td>
									<td style="color:var(--td-muted-2);font-size:12.5px;">{chainsUsing(e).map((c) => c.name).join(', ')}</td>
									<td style="color:var(--td-muted);font-size:12.5px;" title={e.purchased_at ? formatDateTime(e.purchased_at).split(',')[0] : ''}>
										{e.purchased_at ? ownedFor(e.purchased_at) : '—'}
									</td>
									<td style="text-align:right;font-variant-numeric:tabular-nums;font-weight:600;color:{e.total_hours > 0 ? 'var(--td-good-fg)' : 'var(--td-muted-3)'};">{formatHours(e.total_hours)}</td>
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
					</div>
				</div>
			{/if}
			{#if spareGear.length}
				<div class="panel sectable">
					<div class="secsub">
						Not in a chain yet
						<span style="font-size:12px;color:var(--td-muted-2);text-transform:none;letter-spacing:0;margin-left:8px;">new, spare, or lent — tag it so you remember why</span>
					</div>
					<div class="tablewrap">
					<table class="table">
						<thead><tr><th>Gear</th><th>Type</th><th>Brand</th><th>Status</th><th>Owned</th><th style="text-align:right;">Hours</th><th></th></tr></thead>
						<tbody>
							{#each spareGear as e (e.id)}
								<tr id={`gear-${e.id}`} class:highlight={highlightGear === e.id}>
									<td style="font-weight:600;">{e.name}</td>
									<td style="color:var(--td-muted);">{gearTypeLabel(e.equipment_type)}</td>
									<td style="color:var(--td-muted);">{e.brand ?? '—'}</td>
									<td>
										<!-- A select rather than the design's popover: it is the same
										     four choices, and it is reachable from a keyboard without
										     a layer of positioning and outside-click handling. -->
										<select
											class="tagsel"
											class:tagged={!!gearStatus(e.status)}
											style={gearStatus(e.status) ? `color:${gearStatus(e.status)?.colour};border-color:${gearStatus(e.status)?.colour};` : ''}
											value={e.status ?? ''}
											aria-label="Status of {e.name}"
											on:change={(ev) => setGearStatus(e, ev.currentTarget.value)}
										>
											<option value="">+ Tag</option>
											{#each gearStatuses as s}<option value={s.v}>{s.label}</option>{/each}
										</select>
									</td>
									<td style="color:var(--td-muted);font-size:12.5px;" title={e.purchased_at ? formatDateTime(e.purchased_at).split(',')[0] : ''}>
										{e.purchased_at ? ownedFor(e.purchased_at) : '—'}
									</td>
									<td style="text-align:right;font-variant-numeric:tabular-nums;font-weight:600;color:{e.total_hours > 0 ? 'var(--td-good-fg)' : 'var(--td-muted-3)'};">{formatHours(e.total_hours)}</td>
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
					</div>
				</div>
			{/if}
		{:else}
			<div class="panel empty">
				<div class="empty-t">No equipment yet</div>
				<p>Add the boxes you listen through and they become one-click steps in a chain.</p>
				<button class="btn btn-ghost" on:click={newGear}>Add your first</button>
			</div>
		{/if}
	{/if}

	{#if secTab === 'devices'}
		<p class="sechint">Machines that submit listens. A device's default chain is the last fallback — a token's own chain always wins.</p>
		{#if devices.length}
			<div class="panel" style="border-radius:20px;padding:8px;">
				{#each devices as d (d.id)}
					<div class="drow">
						<div class="dicon">
							<svg viewBox="0 0 24 24" width="19" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>
						</div>
						<div style="flex:1;min-width:160px;">
							<div style="font-weight:600;font-size:14.5px;">
								{d.name ?? d.machine_id}
								{#if d.platform}<span style="font-weight:400;font-size:12px;color:var(--td-muted-3);margin-left:5px;">{d.platform}</span>{/if}
							</div>
							<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:1px;">{d.total_listens} listens · last seen {timeAgo(d.last_seen)}</div>
						</div>
						<select class="input assign" class:needs={d.default_chain_id == null}
							value={d.default_chain_id ?? ''} aria-label="Default chain for {d.name ?? d.machine_id}"
							on:change={(e) => assignDevice(d, e.currentTarget.value)}>
							<option value="">— no default —</option>
							{#each chains as c}<option value={c.id}>{c.name}</option>{/each}
						</select>
					</div>
				{/each}
			</div>
		{:else}
			<div class="panel empty">
				<div class="empty-t">No devices yet</div>
				<p>They appear here once a client submits a listen carrying a machine id.</p>
			</div>
		{/if}
	{/if}

	{#if secTab === 'outputs'}
		<p class="sechint">Map each playback output to a chain once — every future listen through it attributes automatically.</p>
		{#if bindings.length}
			<div class="panel" style="border-radius:20px;padding:8px;">
				{#each [...unmapped, ...mapped] as b (b.id)}
					<div class="drow">
						<div class="dicon">
							<svg viewBox="0 0 24 24" width="19" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round"><path d="M11 5L6 9H2v6h4l5 4V5z"/><path d="M15.5 8.5a5 5 0 0 1 0 7M19 5a9 9 0 0 1 0 14"/></svg>
						</div>
						<div style="flex:1;min-width:160px;">
							<div style="font-weight:600;font-size:14.5px;display:flex;align-items:center;gap:8px;flex-wrap:wrap;">
								{b.identifier}
								{#if b.interface}<span class="tag tag-neutral">{b.interface}</span>{/if}
							</div>
							<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:1px;">{b.hits} plays · last seen {timeAgo(b.last_seen)}</div>
						</div>
						<select class="input assign" class:needs={b.chain_id == null}
							value={b.chain_id ?? ''} aria-label="Chain for {b.identifier}"
							on:change={(e) => assignBinding(b, e.currentTarget.value)}>
							<option value="">— unmapped —</option>
							{#each chains as c}<option value={c.id}>{c.name}</option>{/each}
						</select>
						<button class="icon-btn danger" title="Forget this output" on:click={() => removeBinding(b)} aria-label="Delete binding">✕</button>
					</div>
				{/each}
			</div>
		{:else}
			<div class="panel empty dashed">
				<div class="empty-icon">
					<svg viewBox="0 0 24 24" width="24" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round"><path d="M11 5L6 9H2v6h4l5 4V5z"/><path d="M15.5 8.5a5 5 0 0 1 0 7M19 5a9 9 0 0 1 0 14"/></svg>
				</div>
				<div class="empty-t">No outputs detected yet</div>
				<p>They appear here automatically once a client reports its output device — a USB DAC, Bluetooth endpoint, or headphone jack.</p>
			</div>
		{/if}
	{/if}
{/if}

<!-- ── Chain detail drawer ──
     Everything the card used to carry, at the size it deserves: the path drawn
     vertically with its roles, the variants with their own hours, and what
     actually attributes to this chain. -->
{#if openChain}
	<div class="drawer-back" on:click={() => (openChainId = null)} on:keydown={() => {}} role="presentation"></div>
	<aside class="drawer" aria-label="Chain detail">
		<div class="drawer-head">
			<div style="min-width:0;">
				<div class="h-display" style="font-size:23px;line-height:1.15;display:flex;align-items:center;gap:9px;">
					<ChainIcon name={openChain.icon} size={20} title={openChain.name} />{openChain.name}
				</div>
				{#if openChain.description}<div style="font-size:13px;color:var(--td-muted);margin-top:4px;">{openChain.description}</div>{/if}
			</div>
			<button class="icon-btn" on:click={() => (openChainId = null)} aria-label="Close">
				<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>
			</button>
		</div>

		<div class="drawer-body">
			<div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap;margin-bottom:20px;">
				<span class="hours-pill">
					<svg viewBox="0 0 24 24" width="14" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3.5 2"/></svg>
					{formatHours(openChain.total_hours)} logged on this chain
				</span>
				<span class={ctxClass(openChain.listening_context)}>{openChain.listening_context}</span>
			</div>

			<div class="drawer-label">Signal path</div>
			{#if openChain.components?.length}
				<div style="display:flex;flex-direction:column;">
					{#each openChain.components as step, i}
						<!-- Stretched rather than `flex-start`: the connector grows to
						     whatever height the step's text takes, so a step carrying a
						     `detail` line doesn't leave a visible break in the path. -->
						<div style="display:flex;gap:14px;align-items:stretch;">
							<div style="display:flex;flex-direction:column;align-items:center;flex:none;">
								<span class="pdot" style="background:{roleColour(step.type)};box-shadow:0 0 0 4px color-mix(in srgb, {roleColour(step.type)} 16%, transparent);"></span>
								{#if i < openChain.components.length - 1}<span class="pconn"></span>{/if}
							</div>
							<div style="padding-bottom:10px;min-width:0;">
								<div class="prole">{roleLabel(step)}</div>
								<div style="font-size:15.5px;font-weight:600;margin-top:1px;display:flex;align-items:center;gap:6px;">
									{step.name}
									{#if step.equipment_id != null}<span title="From your equipment" style="color:var(--td-accent);font-size:10px;">●</span>{/if}
								</div>
								{#if step.detail}<div style="font-size:12.5px;color:var(--td-muted-2);">{step.detail}</div>{/if}
							</div>
						</div>
					{/each}
				</div>
			{:else}
				<p style="font-size:13.5px;color:var(--td-muted);margin:0;">No steps yet — edit the chain to describe the path.</p>
			{/if}

			<!-- Variants: the same chain with one step swapped. Two cartridges on
			     one turntable are a choice, not two decks. -->
			<div class="drawer-label" style="margin-top:26px;">Also logged as</div>
			{#each variantsFor(openChain.id) as v (v.id)}
				<div class="vrow" title={overrideSummary(openChain, v)}>
					<span style="font-size:14px;min-width:0;">{v.name}</span>
					<span style="display:flex;align-items:center;gap:6px;flex:none;">
						<span style="font-size:12.5px;color:var(--td-muted-2);font-variant-numeric:tabular-nums;">{formatHours(v.total_hours)}</span>
						<button class="icon-btn danger" style="width:26px;height:26px;" title="Delete this variant"
							aria-label="Delete variant {v.name}" on:click={() => removeVariant(v)}>✕</button>
					</span>
				</div>
			{:else}
				<p style="font-size:12.5px;color:var(--td-muted-2);margin:0 0 8px;">
					One setup. Add a variant when you swap a single box — the other stylus, the other IEMs —
					and its hours land on the gear that was actually fitted.
				</p>
			{/each}
			<div style="display:flex;gap:8px;flex-wrap:wrap;">
				<button class="vadd" on:click={() => openChain && newVariant(openChain)}>+ variant</button>
				<!-- The other way to get a variant: an existing chain that is
				     really this one wired differently. Only offered when there
				     is something to fold. -->
				{#if openChain && foldable(openChain).length}
					<button class="vadd" on:click={() => openChain && openFold(openChain)}
						title="Move another chain's listening onto this one as a variant">
						⤵ fold a chain in
					</button>
				{/if}
			</div>

			{#if devices.some((d) => d.default_chain_id === openChain?.id) || bindings.some((b) => b.chain_id === openChain?.id)}
				<div class="drawer-label" style="margin-top:26px;">Attributing to this chain</div>
				{#each devices.filter((d) => d.default_chain_id === openChain?.id) as d (d.id)}
					<div class="arow">
						<svg viewBox="0 0 24 24" width="16" fill="none" stroke="var(--td-muted-2)" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round" style="flex:none;"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>
						<span style="font-size:14px;">{d.name ?? d.machine_id}</span>
						{#if d.platform}<span style="font-size:12px;color:var(--td-muted-3);">{d.platform}</span>{/if}
					</div>
				{/each}
				{#each bindings.filter((b) => b.chain_id === openChain?.id) as b (b.id)}
					<div class="arow">
						<svg viewBox="0 0 24 24" width="16" fill="none" stroke="var(--td-muted-2)" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round" style="flex:none;"><path d="M11 5L6 9H2v6h4l5 4V5z"/><path d="M15.5 8.5a5 5 0 0 1 0 7M19 5a9 9 0 0 1 0 14"/></svg>
						<span style="font-size:14px;">{b.identifier}</span>
						{#if b.interface}<span style="font-size:12px;color:var(--td-muted-3);">{b.interface}</span>{/if}
					</div>
				{/each}
			{/if}
		</div>

		<div class="drawer-foot">
			<button class="btn btn-ghost" style="color:#b2622d;" on:click={() => (confirmChain = openChain)}>Delete</button>
			<div style="display:flex;gap:8px;">
				<button class="btn btn-secondary" on:click={() => openChain && duplicateChain(openChain)}>Duplicate</button>
				<button class="btn btn-primary" on:click={() => openChain && editChain(openChain)}>Edit chain</button>
			</div>
		</div>
	</aside>
{/if}

<!-- Variant editor: pick a slot, pick what goes in it. Deliberately one slot
     at a time — the whole reason variants exist is that swapping *one* thing
     should not mean rebuilding a chain, and a full editor here would just be
     the chain editor again. -->
{#if vOpen && vChain}
	<div class="dialog-backdrop" on:click={() => (vOpen = false)} on:keydown={() => {}} role="presentation">
		<div class="dialog" style="width:min(520px,100%);" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">{vId == null ? 'New variant' : 'Edit variant'} of {vChain.name}</div>
			<div class="dialog-body">
				<div style="display:flex;flex-direction:column;gap:13px;">
					<div class="field">
						<label for="vn">Name</label>
						<input id="vn" class="input" bind:value={vName} placeholder="Microlinear" />
						<p style="font-size:12px;color:var(--td-muted-2);margin:5px 0 0;">
							What is different about it — the stylus fitted, the headphones on.
						</p>
					</div>
					<div class="field">
						<label for="vs">Swap which step</label>
						<select id="vs" class="input" bind:value={vSlot}>
							{#each vChain.components ?? [] as step, i}
								<option value={i}>{compTypeLabel(step.type)} · {step.name}</option>
							{/each}
							<option value={(vChain.components ?? []).length}>+ add a step at the end</option>
						</select>
					</div>
					<div class="field">
						<label for="vg">For</label>
						<select id="vg" class="input" bind:value={vGear}>
							<option value={null}>— pick from your gear —</option>
							{#each equipment as e}
								<option value={e.id}>{e.name}{e.brand ? ` · ${e.brand}` : ''}</option>
							{/each}
						</select>
						<p style="font-size:12px;color:var(--td-muted-2);margin:5px 0 0;">
							Listens tagged with this variant credit their hours here instead of
							whatever the chain names at that step — which is what makes
							per-stylus wear answerable.
						</p>
					</div>
					{#if vErr}<p style="font-size:13px;color:#b2622d;margin:0;">{vErr}</p>{/if}
				</div>
			</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (vOpen = false)}>Cancel</button>
				<button class="btn btn-primary" on:click={saveVariant} disabled={vSaving}>
					{vSaving ? 'Saving…' : 'Save variant'}
				</button>
			</div>
		</div>
	</div>
{/if}

<!-- Fold dialog.
     Two chains that are the same rig with one box swapped are one chain and a
     choice — but saying so after the fact has to *move the listening*, or the
     hours stay split across a row that no longer exists in the user's head.
     Not reversible, and the dialog says so rather than burying it. -->
{#if fOpen && fInto}
	<div class="dialog-backdrop" on:click={() => (fOpen = false)} on:keydown={() => {}} role="presentation">
		<div class="dialog" style="width:min(560px,100%);" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">Fold a chain into “{fInto.name}”</div>
			<div class="dialog-body">
				{#if fDone}
					<p style="font-size:13.5px;margin:0;">{fDone}</p>
				{:else}
					<div style="display:flex;flex-direction:column;gap:13px;">
						<div class="field">
							<label for="ff">Which chain is really this one?</label>
							<select id="ff" class="input" bind:value={fFrom} on:change={pickFoldDefaults}>
								{#each foldable(fInto) as o}
									<option value={o.chain.id}>
										{o.chain.name}{o.dupe
											? o.dupe.distance === 0
												? ' — identical wiring'
												: ` — ${o.dupe.distance} step${o.dupe.distance > 1 ? 's' : ''} differ${o.dupe.distance > 1 ? '' : 's'}`
											: ''}{o.chain.total_hours > 0 ? ` · ${formatHours(o.chain.total_hours)}` : ''}
									</option>
								{/each}
							</select>
							<p style="font-size:12px;color:var(--td-muted-2);margin:5px 0 0;">
								Its listening moves onto “{fInto.name}” and the chain itself is deleted.
								A chain carrying variants of its own can’t fold — its overrides point at
								steps this would replace.
							</p>
						</div>
						<div class="field">
							<label for="fv">Logged as</label>
							<select id="fv" class="input" bind:value={fVariant}>
								<option value="new">— a new variant —</option>
								{#each variantsFor(fInto.id) as v}
									<option value={v.id}>{v.name}</option>
								{/each}
							</select>
							{#if fVariant === 'new'}
								<input class="input" style="margin-top:8px;" bind:value={fName} placeholder="Summer" />
								<p style="font-size:12px;color:var(--td-muted-2);margin:5px 0 0;">
									What it swaps is worked out from the two chains, so the moved hours
									credit the gear that chain actually named — not this one’s.
								</p>
							{:else}
								<p style="font-size:12px;color:var(--td-muted-2);margin:5px 0 0;">
									The moved listens credit whatever this variant swaps in. Pick the one
									that matches how that chain was wired.
								</p>
							{/if}
						</div>
						<p style="font-size:12.5px;color:var(--td-muted-2);margin:0;border-left:2px solid var(--td-muted-3);padding-left:10px;">
							Not reversible. The listens keep everything else — only which chain and
							variant they went through changes.
						</p>
						{#if fErr}<p style="font-size:13px;color:#b2622d;margin:0;">{fErr}</p>{/if}
					</div>
				{/if}
			</div>
			<div class="dialog-actions">
				<!-- The drawer is opened on the way out rather than alongside the
				     result: both are overlays, and raising the drawer while the
				     dialog still holds the "what moved" line buries the one
				     thing worth reading. -->
				<button class="btn btn-secondary"
					on:click={() => { const keep = fDone ? fInto?.id ?? null : null; fOpen = false; if (keep != null) openChainId = keep; }}>
					{fDone ? 'Close' : 'Cancel'}
				</button>
				{#if !fDone}
					<button class="btn btn-primary" on:click={doFold} disabled={fBusy || fFrom == null}>
						{fBusy ? 'Folding…' : 'Fold in'}
					</button>
				{/if}
			</div>
		</div>
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
	/* ── Summary strip ── */
	.tiles { display: flex; gap: 12px; flex-wrap: wrap; margin: 0 0 14px; }
	.tile {
		flex: 1 1 150px; background: var(--td-surface);
		border: 1px solid var(--td-border); border-radius: 16px; padding: 15px 18px;
	}
	.tile-big { font-family: 'Caprasimo', serif; font-size: 28px; line-height: 1; letter-spacing: -.01em; }
	.tile-label { font-size: 13px; color: var(--td-muted); margin-top: 7px; }
	.tile-sub { font-size: 11.5px; color: var(--td-muted-3); margin-top: 2px; }

	/* ── The nudge ── */
	.nudge {
		display: flex; align-items: center; gap: 11px; width: 100%; text-align: left;
		cursor: pointer; font-family: inherit; color: var(--td-text);
		background: var(--td-good-bg); border: 1px solid var(--td-border);
		border-radius: 14px; padding: 11px 16px; margin-bottom: 18px;
	}
	.nudge:hover { border-color: var(--td-accent); }
	.nudge-cta { font-size: 13px; font-weight: 600; color: var(--td-accent); white-space: nowrap; }

	/* ── Chain rows ──
	   One line per chain. This is the whole decluttering: a card per chain with
	   its own icon buttons and a collapsible path turned eight setups into a
	   page you scroll; eight lines is a page you scan. */
	.rows { display: flex; flex-direction: column; gap: 9px; margin-bottom: 34px; }
	.crow {
		display: flex; align-items: center; gap: 16px; width: 100%; text-align: left;
		cursor: pointer; background: var(--td-surface);
		border: 1px solid var(--td-border); border-radius: 16px; padding: 13px 18px;
		font-family: inherit; color: var(--td-text);
		transition: border-color .16s ease, background .16s ease;
	}
	.crow:hover { border-color: var(--td-accent); background: var(--td-surface2); }
	.crow.on { border-color: var(--td-accent); box-shadow: 0 0 0 2px color-mix(in srgb, var(--td-accent) 30%, transparent); }
	@media (prefers-reduced-motion: reduce) { .crow { transition: none; } }
	.cname { flex: 0 0 auto; width: 158px; min-width: 0; }
	.cname-t {
		font-weight: 600; font-size: 15px; display: flex; align-items: center; gap: 7px;
		white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
	}
	.cname-d { font-size: 12.5px; color: var(--td-muted-2); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.cpath { flex: 1; display: flex; align-items: center; gap: 8px; flex-wrap: wrap; min-width: 0; }
	.cstep { display: inline-flex; align-items: center; gap: 7px; font-size: 13px; white-space: nowrap; }
	.dot { width: 9px; height: 9px; border-radius: 999px; flex: none; }
	.arrow { color: var(--td-muted-3); font-size: 12px; }
	.cnone { font-size: 13px; color: var(--td-muted-3); font-style: italic; }
	.setups {
		display: inline-flex; align-items: center; font-size: 11px; color: var(--td-muted-2);
		background: var(--td-surface2); border: 1px solid var(--td-border);
		border-radius: 999px; padding: 2px 9px; white-space: nowrap;
	}
	.cend { flex: 0 0 auto; display: flex; align-items: center; gap: 14px; }
	.chrs { font-variant-numeric: tabular-nums; font-weight: 600; font-size: 14px; white-space: nowrap; color: var(--td-good-fg); }
	.chrs.zero { color: var(--td-muted-3); }
	@media (max-width: 720px) {
		.crow { flex-wrap: wrap; gap: 10px; }
		.cname { flex: 1 1 100%; width: auto; }
		.cpath { flex: 1 1 auto; }
		/* Once the path wraps to its own line the hours follow it and land on
		   the left, under the dots. Pushed back to the right so the column of
		   running times reads down the page the way it does on a wide screen. */
		.cend { margin-left: auto; }
	}

	/* ── The secondary panel ── */
	.sechead { display: flex; align-items: center; justify-content: space-between; gap: 12px; flex-wrap: wrap; margin-bottom: 16px; }
	.seg { display: flex; gap: 4px; background: var(--td-sidebar); border-radius: 999px; padding: 4px; flex-wrap: wrap; }
	.seg-btn {
		white-space: nowrap; text-align: center; border: none; cursor: pointer;
		font-family: inherit; font-size: 13.5px; padding: 9px 16px; border-radius: 999px;
		background: transparent; color: var(--td-nav-fg); font-weight: 500;
	}
	.seg-btn.on { background: var(--td-accent); color: #fbf5ea; font-weight: 600; }
	.sechint { margin: 0 0 14px; color: var(--td-muted); font-size: 13.5px; }
	.sectable { border-radius: 20px; padding: 4px 20px 12px; margin-bottom: 18px; }
	.secsub {
		font-size: 11px; letter-spacing: .08em; text-transform: uppercase;
		color: var(--td-muted-3); padding: 15px 2px 4px;
	}
	/* Seven columns do not fit a phone, and the layout's `<main>` is
	   `overflow-x: hidden` — so without a scroller of its own the table is not
	   narrow, it is *cut off*: Owned, Hours and the edit and delete buttons
	   simply cannot be reached. Nothing errors, and it looks like a table that
	   happens to end early. */
	.tablewrap { overflow-x: auto; }

	.empty { border-radius: 20px; padding: 40px 28px; text-align: center; }
	.empty.dashed { border-style: dashed; border-color: var(--td-muted-3); }
	.empty-icon {
		display: inline-flex; align-items: center; justify-content: center;
		width: 52px; height: 52px; border-radius: 16px;
		background: var(--td-surface2); color: var(--td-muted-2); margin-bottom: 14px;
	}
	.empty-t { font-family: 'Caprasimo', serif; font-size: 19px; margin-bottom: 6px; }
	.empty p { margin: 0 auto 12px; max-width: 46ch; color: var(--td-muted); font-size: 13.5px; line-height: 1.55; }

	.drow { display: flex; align-items: center; gap: 14px; padding: 12px 14px; border-radius: 14px; flex-wrap: wrap; }
	.drow:hover { background: var(--td-surface2); }
	.dicon {
		width: 38px; height: 38px; flex: none; border-radius: 11px;
		background: var(--td-surface2); border: 1px solid var(--td-border);
		display: flex; align-items: center; justify-content: center; color: var(--td-muted);
	}
	.assign { width: auto; min-width: 170px; font-size: 13px; padding: 8px 12px; }
	/* An unassigned row is the one thing on this tab worth looking at. */
	.assign.needs { border-color: var(--td-accent); color: var(--td-accent); font-weight: 600; }

	/* The gear tag. A select styled as a pill: same four choices as the design's
	   popover, reachable from a keyboard, and no positioning code. */
	.tagsel {
		appearance: none; -webkit-appearance: none;
		border: 1px solid var(--td-border); background: transparent;
		color: var(--td-muted-2); border-radius: 999px; padding: 3px 11px;
		font-size: 12px; font-family: inherit; cursor: pointer;
	}
	.tagsel.tagged { font-weight: 600; }

	/* ── Chain drawer ── */
	.drawer-back {
		position: fixed; inset: 0; z-index: 80;
		background: rgba(27, 23, 18, .42); backdrop-filter: blur(2px);
		animation: td-fade-in .2s ease;
	}
	.drawer {
		position: fixed; top: 0; right: 0; z-index: 81;
		width: min(440px, 100%); height: 100vh;
		background: var(--td-surface); border-left: 1px solid var(--td-border);
		box-shadow: -14px 0 44px rgba(46, 43, 37, .22);
		display: flex; flex-direction: column;
		animation: td-slide-in .28s cubic-bezier(.4, 0, .2, 1);
	}
	@media (prefers-reduced-motion: reduce) {
		.drawer, .drawer-back { animation: none; }
	}
	.drawer-head {
		display: flex; align-items: flex-start; justify-content: space-between; gap: 12px;
		padding: 22px 22px 16px; border-bottom: 1px solid var(--td-border);
	}
	.drawer-body { flex: 1; overflow-y: auto; padding: 20px 22px; }
	.drawer-foot {
		display: flex; align-items: center; justify-content: space-between; gap: 10px;
		padding: 14px 22px; border-top: 1px solid var(--td-border); flex-wrap: wrap;
	}
	.drawer-label {
		font-size: 11px; letter-spacing: .08em; text-transform: uppercase;
		color: var(--td-muted-3); margin-bottom: 11px;
	}
	.hours-pill {
		display: inline-flex; align-items: center; gap: 8px; font-size: 13px;
		color: var(--td-good-fg); background: var(--td-good-bg);
		border-radius: 999px; padding: 5px 13px;
	}
	.pdot { width: 14px; height: 14px; border-radius: 999px; flex: none; margin-top: 3px; }
	/* Not `--td-border`, which the design used: at 8–12% alpha a 2px rule is
	   invisible, and the vertical path is the main thing in this drawer. This is
	   a line you are meant to see, so it is the hairline the shelf's dividers
	   are, not the almost-nothing a panel edge is. */
	.pconn {
		width: 2px; flex: 1; min-height: 22px; margin: 3px 0;
		background: color-mix(in srgb, var(--td-muted-3) 55%, transparent);
	}
	.prole { font-size: 10.5px; letter-spacing: .09em; text-transform: uppercase; color: var(--td-muted-3); }
	.vrow {
		display: flex; align-items: center; justify-content: space-between; gap: 10px;
		background: var(--td-surface2); border: 1px solid var(--td-border);
		border-radius: 12px; padding: 7px 8px 7px 14px; margin-bottom: 7px;
	}
	.arow { display: flex; align-items: center; gap: 10px; padding: 7px 2px; }

	/* ── Shared bits ── */
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

	.vadd {
		background: none; border: 1px dashed var(--td-border); border-radius: 999px;
		padding: 4px 12px; font-size: 12.5px; color: var(--td-muted); cursor: pointer;
		font-family: inherit;
	}
	.vadd:hover { border-color: var(--td-accent); color: var(--td-text); }

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
