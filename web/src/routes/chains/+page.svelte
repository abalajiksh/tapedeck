<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { api } from '$lib/api';
	import type { DeviceBinding, InboxQuestion, Room, SignalChain, Equipment, ChainComponent, ChainInput, EquipmentInput, Binding, Device, ChainVariant, FoldSuggestion, WhatsOn } from '$lib/api';
	import { formatHours, timeAgo, toDateInput, fromDateInput, ownedFor, formatDateTime } from '$lib/utils';
	import ChainIcon, { CHAIN_ICONS, CHAIN_ICON_LABELS } from '$lib/components/ChainIcon.svelte';
	import ChainGraph from '$lib/components/chains/ChainGraph.svelte';
	import ChainList from '$lib/components/chains/ChainList.svelte';
	import ChainDrawer, { type RuleChange } from '$lib/components/chains/ChainDrawer.svelte';
	import ShelfLine from '$lib/components/chains/ShelfLine.svelte';
	import RoomChips from '$lib/components/chains/RoomChips.svelte';
	import RoomSuggest from '$lib/components/chains/RoomSuggest.svelte';
	import InboxCard from '$lib/components/chains/InboxCard.svelte';
	import { gearInUse } from '$lib/components/chains/layout';
	import { lastUsedPhrase } from '$lib/components/chains/format';
	import { syncUrl } from '$lib/url';

	let chains: SignalChain[] = [];
	let variants: ChainVariant[] = [];
	let equipment: Equipment[] = [];
	let bindings: Binding[] = [];
	/** Outputs learned per box, from reporters tied to a piece of gear. */
	let deviceBindings: DeviceBinding[] = [];
	let devices: Device[] = [];
	let loading = true;
	let error = '';



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

	// ── Sharing a chain ──
	//
	// **One state, two places.** This calls the same endpoint Patch → Decks
	// does, so the two screens cannot drift into disagreeing about what a deck
	// publishes. The chain object is patched in place rather than re-fetched:
	// the whole list would otherwise reload under an open drawer.
	let sharingChain: number | null = null;

	async function toggleShared(chain: SignalChain, on: boolean) {
		sharingChain = chain.id;
		try {
			await api.setChainShared(chain.id, on);
			chains = chains.map((c) => (c.id === chain.id ? { ...c, shared: on } : c));
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not save');
			// Re-read rather than keeping the optimistic value: what is published
			// is not something to guess at.
			chains = (await api.getChains()).chains;
		} finally {
			sharingChain = null;
		}
	}

	// ── The chain drawer ──
	//
	// Held as an id rather than the object: `load()` replaces every row, and a
	// drawer holding a stale object would show the chain as it was before the
	// variant you just added.
	let openChainId: number | null = null;
	$: openChain = chains.find((c) => c.id === openChainId) ?? null;

	// What's on now at the open chain's slots: fetched when the drawer opens
	// and after every change made in it.
	let whatsOn: WhatsOn | null = null;
	let drawerNotice = '';
	let whatsOnFor: number | null = null;
	$: if (openChainId !== whatsOnFor) {
		whatsOnFor = openChainId;
		drawerNotice = '';
		void loadWhatsOn();
	}
	async function loadWhatsOn() {
		const id = openChainId;
		if (id == null) { whatsOn = null; return; }
		try {
			const w = await api.getWhatsOn(id);
			if (openChainId === id) whatsOn = w;
		} catch {
			if (openChainId === id) whatsOn = null;
		}
	}
	async function drawerChanged(text: string) {
		drawerNotice = text;
		await load();
		await loadWhatsOn();
	}
	// Sorted by hours: the chain you actually listen through is the one worth
	// putting at the top, and a page of chains ordered by creation date buries
	// it under whatever you set up most recently.
	$: chainRows = [...chains].sort((a, b) => b.total_hours - a.total_hours || a.name.localeCompare(b.name));
	$: loggingCount = chains.filter((c) => c.total_hours > 0).length;

	// ── Which chain the graph lights ──
	//
	// Separate from the drawer: choosing a chain lights its route, and the
	// drawer opens from the bar under the graph. In the URL (`?chain=`) so a
	// lit route survives a reload and Back undoes a pick, through `syncUrl`.
	let selectedChainId: number | null = null;
	let lastUsed: Record<string, number> = {};
	/** chain id → (step position → gear on now), for chains with slots. */
	let onNow: Record<string, Record<string, number>> = {};

	// ── Rooms ──
	// Where chains live. A filter on the list and the graph, in the URL
	// (`?room=`) so a room view can be bookmarked; it never re-lays the graph
	// out. Rooms do not touch attribution, so a failure to load them leaves the
	// page working without them rather than showing an error.
	// ── The inbox ──
	// Questions about plays that landed in no chain. The first one sits at the
	// top of the page; the rest are a click away. Like rooms, a failure to load
	// it leaves the page working rather than showing an error.
	let inbox: InboxQuestion[] = [];
	let inboxDone = '';
	async function loadInbox() {
		try {
			inbox = (await api.getChainInbox()).questions;
		} catch {
			inbox = [];
		}
	}
	async function inboxAnswered(text: string) {
		inboxDone = text;
		await Promise.all([load(), loadInbox()]);
		setTimeout(() => (inboxDone = ''), 6000);
	}

	let rooms: Room[] = [];
	let roomFilter: number | null = null;
	let movingRooms = false;
	$: if (roomFilter != null && rooms.length && !rooms.some((r) => r.id === roomFilter)) roomFilter = null;
	$: filterRoom = rooms.find((r) => r.id === roomFilter) ?? null;
	// Picking a room keeps the lit chain when it is in the room, and otherwise
	// lights the room's busiest, as the design does.
	$: if (filterRoom && selectedChainId != null && !filterRoom.chain_ids.includes(selectedChainId)) {
		const first = chainRows.find((c) => filterRoom?.chain_ids.includes(c.id));
		if (first) selectedChainId = first.id;
	}
	async function loadRooms() {
		try {
			rooms = (await api.getRooms()).rooms;
		} catch {
			rooms = [];
		}
	}
	function pickRoom(id: number | null) {
		roomFilter = id;
		const url = new URL(window.location.href);
		if (id == null) url.searchParams.delete('room');
		else url.searchParams.set('room', String(id));
		syncUrl(url.pathname + url.search);
	}
	async function moveChainRooms(chain: SignalChain, ids: number[]) {
		movingRooms = true;
		try {
			await api.setChainRooms(chain.id, ids);
			await loadRooms();
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not change the rooms');
		} finally {
			movingRooms = false;
		}
	}
	$: if (chains.length && !chains.some((c) => c.id === selectedChainId)) selectedChainId = chainRows[0]?.id ?? null;
	$: selectedChain = chains.find((c) => c.id === selectedChainId) ?? null;
	$: usedGear = gearInUse(chains, variants);
	$: stepGear = gearInUse(chains, []);
	$: selectedLine = selectedChain
		? [
				(selectedChain.components ?? []).filter((s) => s.name.trim()).map((s) => s.name).join(' → ') ||
					'No steps yet',
				`${formatHours(selectedChain.total_hours)} on this chain`,
				lastUsedPhrase(lastUsed[String(selectedChain.id)])
			].join(' · ')
		: '';
	function selectChain(id: number) {
		selectedChainId = id;
		const url = new URL(window.location.href);
		url.searchParams.set('chain', String(id));
		syncUrl(url.pathname + url.search);
	}
	$: totalChainHours = chains.reduce((a, c) => a + c.total_hours, 0);

	function onKey(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			// Innermost first, or Escape closes the drawer out from under the
			// overlay sitting on top of it.
			if (openChainId != null) openChainId = null;
		}
	}

	async function removeVariant(v: ChainVariant) {
		// The listens keep their chain and fall back to it as wired, which is
		// what a null variant means everywhere — so this is not the destructive
		// act deleting a chain would be, and needs no confirmation.
		try {
			await api.deleteVariant(v.id);
			await load();
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
	}

	// ── Merging a chain that swaps one box ──
	//
	// Two chains that differ at one box past the player (Commute Phone on the
	// HD 569, Summer Commute on the Hexa) are one chain with a slot. The inbox
	// asks once; this is the same thing from the drawer, any time. The other
	// chain's listening moves across filed with its gear, and it is deleted.
	let dupes: FoldSuggestion[] = [];

	let mOpen = false;
	let mInto: SignalChain | null = null; // the chain kept
	let mFrom: number | null = null; // the chain that becomes an option
	let mErr = '';
	let mBusy = false;
	let mDone: string | null = null;

	const PLAYERS = new Set(['source', 'transport']);
	/** Chains that can merge into this one as an option: one box apart at a
	 *  step past the player, and carrying no setups of their own. */
	$: mergeable = (into: SignalChain | null) =>
		into?.id == null
			? []
			: dupes
					.filter((d) => d.distance === 1 && d.overrides.length === 1 && (d.base_chain_id === into.id || d.fold_chain_id === into.id))
					.map((d) => {
						const other = chains.find((c) => c.id === (d.base_chain_id === into.id ? d.fold_chain_id : d.base_chain_id));
						const pos = d.overrides[0].position;
						return { other, pos, mine: into.components?.[pos], theirs: other?.components?.[pos] };
					})
					.filter(
						(m) =>
							m.other &&
							m.mine?.equipment_id != null &&
							m.theirs?.equipment_id != null &&
							!PLAYERS.has(m.mine.type) &&
							(m.other.components?.length ?? 0) === (into.components?.length ?? 0) &&
							!variants.some((v) => v.chain_id === m.other?.id)
					);

	function openMerge(into: SignalChain, from?: number) {
		mInto = into;
		mErr = '';
		mDone = null;
		mFrom = from ?? mergeable(into)[0]?.other?.id ?? null;
		mOpen = true;
	}

	async function doMerge() {
		if (mFrom == null || mInto?.id == null) { mErr = 'Pick a chain to merge in.'; return; }
		mBusy = true;
		mErr = '';
		try {
			const m = mergeable(mInto).find((x) => x.other?.id === mFrom);
			const r = await api.mergeChain(mInto.id, mFrom);
			// Said rather than assumed: a merge that moved no listens is a real
			// outcome (an unused duplicate).
			mDone =
				`${m?.theirs?.name ?? 'Its gear'} is an option on “${mInto.name}” now. ` +
				(r.listens
					? `${r.listens} listen${r.listens > 1 ? 's' : ''} moved with it` +
					  (r.devices || r.tokens || r.bindings
							? `, and ${r.devices + r.tokens + r.bindings} rule${r.devices + r.tokens + r.bindings > 1 ? 's' : ''} for where plays go`
							: '') +
					  '.'
					: 'It had no listening of its own.');
			await load();
		} catch (e) {
			mErr = String(e).replace(/^API \d+: /, '');
		}
		mBusy = false;
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
		// The chain icons have had a subwoofer since v0.50.0 and the gear list
		// never did, so a sub had to be filed as speakers — which then reads as
		// a second pair in every slot that offers transducers.
		{ v: 'subwoofer', label: 'Subwoofer' },
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
		// A radio with its own speaker — a DAB+ kitchen radio, a clock radio.
		// It is the source *and* the speaker, so it may fill both slots of one
		// chain (see ALL_IN_ONE).
		{ v: 'radio', label: 'Radio / tuner (own speaker)' },
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
		// A sub is a second transducer beside the mains rather than a stage the
		// signal passes through — it lands in the same slot as the speakers.
		subwoofer: 'transducer',
		'builtin-speakers': 'transducer', earbuds: 'bluetooth',
		dac: 'dac', 'dac-amp': 'dac', interface: 'dac', receiver: 'dac',
		amp: 'amp', preamp: 'amp',
		source: 'source', turntable: 'source', phone: 'source', radio: 'source',
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
	// Gear that is its own speaker. It is placed as the source and may be
	// picked again for the speaker slot of the same chain — a kitchen radio is
	// both ends of its own chain. Gear hours count a box once per chain however
	// many slots it fills (`gear_hours` folds by equipment id), so this does
	// not double its hours.
	const ALL_IN_ONE = new Set(['radio']);
	function gearFitsSlot(gearType: string, compType: string): boolean {
		const known = componentTypeForGear[gearType];
		if (ALL_IN_ONE.has(gearType) && compType === 'transducer') return true;
		return known === undefined || known === compType;
	}
	/** Taken by another slot, unless it is all-in-one gear going into the speaker slot. */
	function blocked(e: Equipment, taken: Set<number | null | undefined>, slot: string): boolean {
		return taken.has(e.id) && !(ALL_IN_ONE.has(e.equipment_type) && slot === 'transducer');
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
	/** Other names this box reports as, comma-separated in the dialog. */
	let gAliases = '';
	let savingGear = false;
	let confirmGear: Equipment | null = null;
	// Set when the gear dialog was opened from inside a chain component row, so
	// the new gear drops straight into the slot that asked for it.
	let gearForComp: number | null = null;

	async function load() {
		try {
			const [c, g, b, d, v, dup] = await Promise.all([
				api.getChains(), api.getEquipment(), api.getBindings(), api.getDevices(), api.getVariants(),
				api.getChainDuplicates()
			]);
			chains = c.chains;
			lastUsed = c.last_used ?? {};
			onNow = c.on_now ?? {};
			variants = v.variants;
			dupes = dup.duplicates;
			equipment = g.equipment;
			bindings = b.bindings;
			deviceBindings = b.device_bindings ?? [];
			devices = d.devices;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
		}
		loading = false;
	}

	// Outputs and machines are set from the chain drawer ("How plays find
	// this chain"), the route editor's source box and the inbox, since the
	// Outputs and Machines tabs went (phase 4). `bindFor` is the output a new
	// chain was started from; nothing sets it today, the save still honours it.
	let bindFor: Binding | null = null;

	function closeChainEditor() { chainOpen = false; bindFor = null; }
	// ── How plays find a chain, changed from its drawer ──
	async function changeRule(r: RuleChange) {
		try {
			if (r.kind === 'device') await api.setDeviceChain(r.id, r.chainId);
			else if (r.chainId == null && r.kind === 'binding') await api.deleteBinding(r.id);
			else if (r.chainId == null && r.kind === 'device_binding') await api.deleteDeviceBinding(r.id);
			else await api.setBinding(r.identifier, r.chainId, null, r.kind === 'device_binding' ? r.equipmentId : null);
			drawerNotice = r.chainId == null ? `${r.label} no longer comes here.` : `${r.label} goes to ${chains.find((c) => c.id === r.chainId)?.name ?? 'that chain'} now.`;
			await load();
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not change that');
		}
	}

	// ── chain editor ──
	function newChain() {
		bindFor = null;
		chainId = null; cName = ''; cDesc = ''; cContext = 'active'; cIcon = null;
		cComps = [blankComp('source')];
		chainOpen = true;
	}
	function editChain(c: SignalChain) {
		bindFor = null;
		chainId = c.id; cName = c.name; cDesc = c.description ?? ''; cContext = c.listening_context;
		cIcon = c.icon ?? null;
		cComps = c.components?.length ? c.components.map((x) => ({ ...x })) : [blankComp('source')];
		chainOpen = true;
	}
	/** Most second chains are the first with one box swapped. */
	function duplicateChain(c: SignalChain) {
		bindFor = null;
		chainId = null;
		cName = `${c.name} (copy)`; cDesc = c.description ?? ''; cContext = c.listening_context;
		cIcon = c.icon ?? null;
		cComps = c.components?.length ? c.components.map((x) => ({ ...x })) : [blankComp('source')];
		chainOpen = true;
	}
	const blankComp = (type: string): ChainComponent => ({ type, name: '', detail: null, equipment_id: null, since: null });

	// ── when a box joined the path ──
	//
	// Chain hours are derived from the listens, so a box added to a chain that
	// already has history is otherwise credited with all of it — a subwoofer
	// bought on Tuesday reporting a year of use. The server dates gear a chain
	// did not name before, and this is where that date is corrected: it is
	// stamped on the day you got round to saving, which is rarely the day the
	// thing arrived.
	//
	// Quiet by default. Every row carrying a third control is the clutter the
	// drawer was built to remove, so it is one line of muted text that becomes
	// a date field when you press it.
	let sinceOpen: Record<number, boolean> = {};
	const dateValue = (secs?: number | null) =>
		secs ? new Date(secs * 1000).toISOString().slice(0, 10) : '';
	const sinceLabel = (secs?: number | null) =>
		secs
			? `in the path since ${new Date(secs * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' })}`
			: 'in the path from the start';
	function setSince(i: number, value: string) {
		// Midday rather than midnight: the date is a day the user names, and a
		// listen that afternoon should be on the near side of it whatever
		// timezone they are in.
		const t = value ? Math.floor(new Date(`${value}T12:00:00`).getTime() / 1000) : null;
		cComps[i] = { ...cComps[i], since: t };
		cComps = cComps;
	}
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
				(e) => !blocked(e, taken, slot) && (_all[i] || gearFitsSlot(e.equipment_type, slot))
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
				(e) => !blocked(e, taken, slot) && !gearFitsSlot(e.equipment_type, slot)
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
			if (chainId == null) {
				const made = await api.createChain(data);
				// Opened from an output: the chain exists so that output can use it.
				if (bindFor) await api.setBinding(bindFor.identifier, made.id, null);
			} else await api.updateChain(chainId, data);
			chainOpen = false;
			bindFor = null;
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
		gearId = null; gName = ''; gType = 'headphones'; gBrand = ''; gModel = ''; gNotes = ''; gAliases = '';
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
		gAliases = (e.aliases ?? []).join(', ');
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
				: await api.updateEquipment(gearId, {
						...data,
						status: gStatus,
						aliases: gAliases.split(',').map((a) => a.trim()).filter(Boolean)
					});
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

	// A `[[gear:…]]` shortcode in a liner note links to /chains?gear=<id>. There
	// is no gear detail page yet, so the link highlights the row it means rather
	// than dropping you at the top of a long page with nothing to look at.
	let highlightGear: number | null = null;

	// Which gear row is showing where its hours came from. One at a time — the
	// question is "is this number right", asked of one piece at a time.
	let openGearHours: number | null = null;

	// Which chain a play accepted out of somebody else's sitting is filed
	// under. Unset by default, and unset is the old behaviour exactly.
	let spoolChainId: number | null = null;

	async function saveSpoolChain(raw: string) {
		const id = raw ? Number(raw) : null;
		try {
			await api.setSpoolChain(id);
			spoolChainId = id;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not save that';
		}
	}

	onMount(async () => {
		const q = new URLSearchParams(window.location.search);
		const g = q.get('gear');
		highlightGear = g ? Number(g) : null;
		const picked = Number(q.get('chain'));
		if (picked) selectedChainId = picked;
		// Back from the route editor: open the chain and say what the save did.
		const saved = q.get('saved');
		if (picked && saved) {
			openChainId = picked;
			whatsOnFor = picked;
			void loadWhatsOn().then(() => (drawerNotice = saved));
			const url = new URL(window.location.href);
			url.searchParams.delete('saved');
			syncUrl(url.pathname + url.search);
		}
		const inRoom = Number(q.get('room'));
		if (inRoom) roomFilter = inRoom;
		void loadRooms();
		void loadInbox();
		await load();
		// Failing to read it leaves the picker on "No chain", which is the
		// default and therefore honest — never an error banner over a
		// preference nobody was setting.
		try {
			spoolChainId = (await api.getSpoolChain()).chain_id;
		} catch {
			/* the picker shows the default */
		}
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
	<a class="btn btn-primary" href="/chains/new">+ New chain</a>
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
			<div class="tile-sub">{equipment.length - [...usedGear].filter((id) => equipment.some((e) => e.id === id)).length} on the shelf</div>
		</div>
	</div>

	<!-- The nudges.
	     v0.62.0 allowed exactly **one** of these, on the reasoning that two
	     stacked banners are the clutter this page was being rescued from. That
	     was wrong in a way only real use showed: an unmapped device you never
	     intend to map is permanent, so it starved the queue behind it and the
	     duplicate-chain prompt — added in v0.67.0 — was never once visible on
	     the instance that had near-duplicate chains to fold.

	     A nudge nobody can reach is worse than a second thin line. So the two
	     that describe *data being lost or split right now* both show; the
	     output one, which is only ever a tidy-up, still waits for both to
	     clear. Two lines maximum. -->
	<!-- Rooms you sit in.
	     A play accepted out of a Shared Spool used to land with no chain at
	     all, so those listens earned no gear hours, carried no fidelity and
	     could never reach the Reel. Defensible — it was not your gear — and not
	     the whole truth, because you were in the room hearing real speakers.

	     Here rather than in Settings because the question is "which of my
	     chains", and this is the page holding the answer to it. Rendered only
	     once there is a chain to pick: a picker with nothing in it is a control
	     that reads as broken. -->
	{#if chainRows.length}
		<div class="spoolchain">
			<div class="sc-main">
				<div class="sc-title">Rooms you sit in</div>
				<div class="sc-sub">
					Where a play lands when you accept it out of somebody else's Shared Spool.
					Leave it unset and those listens carry no chain, which is what they have
					always done.
				</div>
			</div>
			<select
				class="input sc-pick"
				aria-label="Chain for plays accepted out of a Shared Spool"
				value={spoolChainId ?? ''}
				on:change={(e) => saveSpoolChain(e.currentTarget.value)}
			>
				<option value="">No chain</option>
				{#each chainRows as c (c.id)}
					<option value={String(c.id)}>{c.name}</option>
				{/each}
			</select>
		</div>
	{/if}

	<!-- The chains and how they are wired, side by side.
	     The list is how you get around (one button per chain, keyboard and all);
	     the graph is a picture of the same thing with every piece of gear drawn
	     once, the chosen chain's route lit. The drawer opens from the bar under
	     the graph, so scanning never pops a panel over what you were reading. -->
	{#if inboxDone}<div class="inbox-done">✓ {inboxDone}</div>{/if}
	{#if inbox.length}
		<div class="inbox-top">
			{#key inbox[0].key}
				<InboxCard q={inbox[0]} {chains} {equipment} position="1 of {inbox.length}"
					on:done={(e) => inboxAnswered(e.detail)} />
			{/key}
			{#if inbox.length > 1}<a class="btn btn-ghost inbox-all" href="/chains/inbox">See all {inbox.length} →</a>{/if}
		</div>
	{/if}
	{#if chainRows.length}
		<RoomSuggest chains={chainRows} {equipment} {rooms} on:done={loadRooms} />
		<RoomChips {rooms} chainCount={chains.length} roomId={roomFilter} on:pick={(e) => pickRoom(e.detail)} />
		<div class="wired">
			<div class="wlist">
				<ChainList chains={chainRows} {equipment} {onNow} {lastUsed} {rooms} roomId={roomFilter}
					selectedId={selectedChainId} on:select={(e) => selectChain(e.detail)} />
			</div>
			<section class="panel wgraph" aria-label="How it is wired">
				<div class="whead">
					<div>
						<h2 class="h-display" style="font-size:22px;">How it’s wired</h2>
						<p class="wsub">
							Pick a chain to light its route. Every piece of gear shows up once, with its
							hours; <span class="wswap">⇄</span> marks gear you swap at a step.
						</p>
					</div>
				</div>
				<ChainGraph {chains} {equipment} {onNow} {rooms} roomId={roomFilter}
					selectedId={selectedChainId} on:select={(e) => selectChain(e.detail)} />
				{#if selectedChain}
					<div class="wsel">
						<div style="flex:1 1 320px;min-width:0;">
							<div style="font-weight:700;">{selectedChain.name}</div>
							<div class="wsub" style="margin:0;">{selectedLine}</div>
						</div>
						<button class="btn btn-secondary" on:click={() => (openChainId = selectedChain?.id ?? null)}>
							Open {selectedChain.name}
						</button>
					</div>
				{/if}
			</section>
		</div>
	{:else}
		<div class="panel" style="border-radius:22px;padding:44px;text-align:center;color:var(--td-muted);">
			No signal chains yet — <a class="btn btn-ghost" href="/chains/new">create your first</a>
		</div>
	{/if}

	<ShelfLine {equipment} inUse={usedGear} onGraph={stepGear} on:edit={(e) => editGear(e.detail)} />

	<!-- The gear table. Outputs and machines used to be two tabs beside it;
	     they are set from a chain's drawer, the route editor and the inbox
	     now (phase 4). -->
	<div class="sechead">
		<h2 class="h-display" style="font-size:20px;margin:0;">Gear · {equipment.length}</h2>
		<button class="btn btn-secondary" on:click={newGear}>+ Add gear</button>
	</div>

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
									<td style="text-align:right;font-variant-numeric:tabular-nums;font-weight:600;color:{e.total_hours > 0 ? 'var(--td-good-fg)' : 'var(--td-muted-3)'};">
										<!-- Derived from listens through possibly several chains, so it is a
											     claim rather than a reading. Click it to see the addition. -->
										{#if e.sources?.length}
											<button class="hrs-btn" aria-expanded={openGearHours === e.id}
												aria-label="Where {e.name}'s hours came from"
												on:click={() => (openGearHours = openGearHours === e.id ? null : e.id)}>
												{formatHours(e.total_hours)}
												<span class="hrs-caret" class:on={openGearHours === e.id}>▾</span>
											</button>
										{:else}
											{formatHours(e.total_hours)}
										{/if}
									</td>
									<td style="text-align:right;white-space:nowrap;">
										<button class="icon-btn" title="Edit" on:click={() => editGear(e)} aria-label="Edit gear">
											<svg viewBox="0 0 24 24" width="15" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/></svg>
										</button>
										<button class="icon-btn danger" title="Delete" on:click={() => (confirmGear = e)} aria-label="Delete gear">
											<svg viewBox="0 0 24 24" width="15" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16"/><path d="M10 11v6M14 11v6"/><path d="M6 7l1 13h10l1-13"/><path d="M9 7V4h6v3"/></svg>
										</button>
									</td>
								</tr>
								{#if openGearHours === e.id && e.sources?.length}
									<tr class="hrs-row">
										<td colspan="7">
											<div class="hrs-head">Where these hours came from</div>
											{#each e.sources as src}
												<div class="hrs-line">
													<span class="hrs-what">{src.chain_name}{#if src.variant_name}<span class="hrs-var">· {src.variant_name}</span>{/if}</span>
													<span class="hrs-amt">{formatHours(src.hours)}</span>
												</div>
											{/each}
											<div class="hrs-line hrs-total">
												<span class="hrs-what">Total</span>
												<span class="hrs-amt">{formatHours(e.total_hours)}</span>
											</div>
										</td>
									</tr>
								{/if}
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

<!-- ── Chain detail drawer ──
     The design's "Place" layout on today's data; see `ChainDrawer`. Every
     action comes back here, where the dialogs and the data live. -->
{#if openChain}
	<ChainDrawer
		chain={openChain}
		{variants}
		{equipment}
		{devices}
		{bindings}
		{deviceBindings}
		{chains}
		{whatsOn}
		notice={drawerNotice}
		lastUsed={lastUsed[String(openChain.id)] ?? null}
		sharing={sharingChain === openChain.id}
		canMerge={mergeable(openChain).length > 0}
		{rooms}
		{movingRooms}
		on:rooms={(e) => openChain && moveChainRooms(openChain, e.detail)}
		on:close={() => (openChainId = null)}
		on:share={(e) => openChain && toggleShared(openChain, e.detail)}
		on:changed={(e) => drawerChanged(e.detail)}
		on:removeVariant={(e) => removeVariant(e.detail)}
		on:rule={(e) => changeRule(e.detail)}
		on:merge={() => openChain && openMerge(openChain)}
		on:delete={() => (confirmChain = openChain)}
		on:duplicate={() => openChain && duplicateChain(openChain)}
		on:edit={() => openChain && goto(`/chains/${openChain.id}/route`)}
		on:rename={() => openChain && editChain(openChain)}
	/>
{/if}

<!-- Merge dialog.
     A chain that swaps one box is one chain with a slot, but saying so after
     the fact has to *move the listening*, or the hours stay split across a row
     that no longer exists in the person's head. Not reversible, and the
     dialog says so rather than burying it. -->
{#if mOpen && mInto}
	{@const opts = mergeable(mInto)}
	<div class="dialog-backdrop" on:click={() => (mOpen = false)} on:keydown={() => {}} role="presentation">
		<div class="dialog" style="width:min(560px,100%);" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">Merge a chain into “{mInto.name}” as an option</div>
			<div class="dialog-body">
				{#if mDone}
					<p style="font-size:13.5px;margin:0;">{mDone}</p>
				{:else}
					<div style="display:flex;flex-direction:column;gap:13px;">
						<div class="field">
							<label for="mf">Which chain is this one with other gear on?</label>
							<select id="mf" class="input" bind:value={mFrom}>
								{#each opts as o (o.other?.id)}
									<option value={o.other?.id}>
										{o.other?.name} — {o.theirs?.name} instead of {o.mine?.name}{(o.other?.total_hours ?? 0) > 0 ? ` · ${formatHours(o.other?.total_hours ?? 0)}` : ''}
									</option>
								{/each}
							</select>
							<p style="font-size:12px;color:var(--td-muted-2);margin:5px 0 0;">
								Its gear becomes an option at that step, so you pick which is on. Its listening moves onto
								“{mInto.name}” filed with that gear, with its outputs, machines and rooms, and the chain itself
								is deleted.
							</p>
						</div>
						<p style="font-size:12.5px;color:var(--td-muted-2);margin:0;border-left:2px solid var(--td-muted-3);padding-left:10px;">
							Not reversible. The listens keep everything else; only which chain they went through changes.
						</p>
						{#if mErr}<p style="font-size:13px;color:var(--td-bad-fg);margin:0;">{mErr}</p>{/if}
					</div>
				{/if}
			</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary"
					on:click={() => { const keep = mDone ? mInto?.id ?? null : null; mOpen = false; if (keep != null) openChainId = keep; }}>
					{mDone ? 'Close' : 'Cancel'}
				</button>
				{#if !mDone}
					<button class="btn btn-primary" on:click={doMerge} disabled={mBusy || mFrom == null}>
						{mBusy ? 'Merging…' : 'Merge it in'}
					</button>
				{/if}
			</div>
		</div>
	</div>
{/if}

<!-- Chain editor dialog -->
{#if chainOpen}
	<div class="dialog-backdrop" on:click={closeChainEditor} on:keydown={() => {}} role="presentation">
		<div class="dialog" style="width:min(620px,100%);" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">{chainId == null ? 'New signal chain' : 'Rename and describe'}</div>
			{#if bindFor && chainId == null}
				<p style="font-size:12.5px;color:var(--td-muted-2);margin:-4px 0 10px;">
					For listens through “{bindFor.identifier}” — saving maps that output to it.
				</p>
			{/if}
			<div class="dialog-body" style="max-height:70vh;overflow-y:auto;">
				<div style="display:flex;flex-direction:column;gap:13px;">
					<div class="field"><label for="cn">Name</label><input id="cn" class="input" bind:value={cName} placeholder="Desktop Reference"></div>
					<div class="field"><label for="cd">Description</label><input id="cd" class="input" bind:value={cDesc} placeholder="The main desk rig"></div>
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
					<!-- A chain that exists changes its route in the route editor,
					     which asks what happens to plays already logged. -->
					{#if chainId == null}
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
								<!-- Empty means "for as long as the chain has existed", which
								     is what every step written before this field meant. -->
								<div class="cwhen">
									{#if sinceOpen[i]}
										<input
											type="date"
											class="input"
											style="width:auto;padding:4px 8px;font-size:12.5px;"
											value={dateValue(comp.since)}
											aria-label="In the path since"
											on:change={(e) => setSince(i, e.currentTarget.value)}
										>
										{#if comp.since}
											<button class="linkish" on:click={() => setSince(i, '')}>from the start instead</button>
										{/if}
									{:else}
										<button
											class="linkish"
											on:click={() => (sinceOpen = { ...sinceOpen, [i]: true })}
											title="Hours are counted from here. A box added to a chain that already has history should not be credited with it."
										>{sinceLabel(comp.since)}</button>
									{/if}
								</div>
							{/each}
							<button class="btn btn-ghost" style="align-self:flex-start;" on:click={addComp}>+ Add component</button>
						</div>
					</div>
					{/if}
				</div>
			</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={closeChainEditor}>Cancel</button>
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
								{gBought && ownedFor(fromDateInput(gBought))
									? `Owned for ${ownedFor(fromDateInput(gBought))}.`
									: 'Optional — leave blank if you are guessing.'}
							</p>
						</div>
					</div>
					<div class="field"><label for="go">Notes</label><input id="go" class="input" bind:value={gNotes}></div>
					{#if gearId != null}
						<!-- Remembered from the inbox, editable here: an app that reports
						     one of these names is suggested as this box, never tied to it
						     without asking. -->
						<div class="field">
							<label for="galias">Also reports as</label>
							<input id="galias" class="input" bind:value={gAliases} placeholder="A model number, say XQ-DQ54">
						</div>
					{/if}
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
	/* — where a gear total came from —
	   The figure is derived from listens across possibly several chains, so it
	   reads as a claim. This turns it into an addition you can check. */
	.hrs-btn {
		background: none; border: 0; padding: 0; cursor: pointer; font: inherit;
		color: inherit; display: inline-flex; align-items: center; gap: 5px;
	}
	.hrs-btn:hover { text-decoration: underline; }
	.hrs-caret {
		font-size: 9px; color: var(--td-muted-3); transition: transform .15s ease;
	}
	.hrs-caret.on { transform: rotate(180deg); }
	.hrs-row td {
		padding: 10px 14px 14px 14px;
		background: color-mix(in srgb, var(--td-text) 3%, transparent);
	}
	.hrs-head {
		font-size: 11px; letter-spacing: .08em; text-transform: uppercase;
		color: var(--td-muted-2); margin-bottom: 7px;
	}
	.hrs-line {
		display: flex; justify-content: space-between; gap: 16px;
		font-size: 13px; padding: 3px 0;
	}
	.hrs-what { color: var(--td-muted); min-width: 0; }
	/* The gap is a margin, not a leading space in the markup — Svelte trims
	   whitespace at an element boundary, so the space was silently dropped and
	   it rendered as "Commute Phone· Summer". */
	.hrs-var { color: var(--td-accent); margin-left: 5px; }
	.hrs-amt { font-variant-numeric: tabular-nums; white-space: nowrap; }
	.hrs-total {
		margin-top: 5px; padding-top: 7px; font-weight: 600;
		border-top: 1px solid var(--td-muted-3);
	}

	/* ── The secondary panel ── */
	.sechead { display: flex; align-items: center; justify-content: space-between; gap: 12px; flex-wrap: wrap; margin-bottom: 16px; }
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
	.empty-t { font-family: 'Caprasimo', serif; font-size: 19px; margin-bottom: 6px; }
	.empty p { margin: 0 auto 12px; max-width: 46ch; color: var(--td-muted); font-size: 13.5px; line-height: 1.55; }

	/* The gear tag. A select styled as a pill: same four choices as the design's
	   popover, reachable from a keyboard, and no positioning code. */
	.tagsel {
		appearance: none; -webkit-appearance: none;
		border: 1px solid var(--td-border); background: transparent;
		color: var(--td-muted-2); border-radius: 999px; padding: 3px 11px;
		font-size: 12px; font-family: inherit; cursor: pointer;
	}
	.tagsel.tagged { font-weight: 600; }
	/* Svelte scopes styles per component, so this is repeated here rather than
	   inherited from the four other pages that use it — matching them exactly
	   matters more than deduplicating, and a class defined nowhere renders as a
	   default browser button with nothing erroring. */
	.linkish {
		border: none;
		background: none;
		padding: 0;
		font: inherit;
		font-weight: 600;
		color: var(--td-accent);
		cursor: pointer;
	}
	.linkish:hover { color: var(--td-accent-hi); }
	.cwhen { display: flex; align-items: center; gap: 8px; margin: -2px 0 4px 176px; }
	@media (max-width: 560px) { .cwhen { margin-left: 0; } }

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

	.icon-btn {
		border: none; background: transparent; cursor: pointer; color: var(--td-muted-2);
		width: 30px; height: 30px; border-radius: 8px; display: inline-flex; align-items: center; justify-content: center;
	}
	.icon-btn:hover { background: var(--td-nav-hover); color: var(--td-text); }
	.icon-btn.danger:hover { color: #b2622d; }
	tr.highlight {
		background: color-mix(in srgb, var(--td-accent) 12%, transparent);
	}

	/* Rooms you sit in — the chain an accepted Shared Spool play lands on. */
	.spoolchain {
		display: flex;
		align-items: center;
		gap: 14px;
		flex-wrap: wrap;
		padding: 14px 16px;
		margin-bottom: 16px;
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
		border-radius: 14px;
	}
	.sc-main { flex: 1; min-width: 240px; }
	.sc-title { font-weight: 600; font-size: 14px; }
	.sc-sub { font-size: 12.5px; color: var(--td-muted); margin-top: 2px; line-height: 1.45; }
	.sc-pick { width: auto; min-width: 160px; }

	/* ── The chain list and the graph, side by side ──
	   The list keeps a reading width; the graph takes the rest and drops under
	   the list on a narrow screen rather than shrinking past legibility. */
	.wired { display: flex; flex-wrap: wrap; gap: 20px; align-items: flex-start; margin: 4px 0 16px; }
	.wlist { flex: 1 1 260px; max-width: 320px; }
	.wgraph { flex: 999 1 560px; min-width: 0; border-radius: 22px; padding: 18px 18px 14px; }
	.whead { display: flex; align-items: flex-start; justify-content: space-between; gap: 12px; padding: 0 4px 12px; }
	.wsub { font-size: 13px; color: var(--td-muted); margin: 3px 0 0; }
	.wswap { padding: 0 6px; border-radius: 999px; border: 1px dashed var(--td-muted-3); }
	.wsel {
		display: flex; align-items: center; gap: 12px; flex-wrap: wrap;
		padding: 14px 6px 2px; border-top: 1px solid var(--td-border); margin-top: 12px;
	}
	@media (max-width: 720px) {
		.wlist { max-width: none; }
	}

	/* ── The inbox at the top ── */
	.inbox-top { display: flex; flex-direction: column; gap: 6px; margin-bottom: 14px; }
	.inbox-all { align-self: flex-end; }
	.inbox-done {
		margin-bottom: 12px; padding: 10px 14px; border-radius: 14px;
		background: var(--td-good-bg); color: var(--td-good-fg); font-size: 13.5px;
	}
</style>
