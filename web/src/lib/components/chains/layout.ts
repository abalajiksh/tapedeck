// The Signal Chains graph, laid out without a layout engine.
//
// A chain is a line (`components` is an array) and every step has a role, so
// the picture is deterministic: x comes from the role's column (Plays from →
// Converts → Amplifies → Heard on), y from one barycentre pass left to right
// that puts each box near the boxes feeding it. No dagre, no elk, no force
// layout: the graph looks the same every time it is opened, which is what makes
// it a map rather than a picture (design notes §1).
//
// **Every piece of gear is drawn once**, however many chains run through it.
// A step names its gear by `equipment_id` when it was picked from your gear,
// and by name otherwise, so two chains that typed "MR4" by hand share a box.

import type { ChainComponent, ChainVariant, Equipment, Room, SignalChain } from '$lib/api';

export const COLUMNS = ['Plays from', 'Converts', 'Amplifies', 'Heard on'] as const;

/** Which column a step's role sits in. Unknown roles read as a source: the
 *  start of a line is the least surprising place for something unclassified. */
const COLUMN_OF: Record<string, number> = {
	source: 0,
	transport: 0,
	network: 0,
	dac: 1,
	amp: 2,
	transducer: 3,
	bluetooth: 3
};
export const columnOf = (type: string): number => COLUMN_OF[type] ?? 0;

/** Gear that is part of another box (a laptop's own speakers, a phone's
 *  speaker). Drawn dashed: it is not something you could unplug. */
export const BUILT_IN = new Set(['builtin-speakers']);

/** A `ChainIcon` glyph for a box, from its gear type or, for a typed-in step,
 *  its role. */
export const ICON_FOR_GEAR: Record<string, string> = {
	headphones: 'headphones', iem: 'iem', earbuds: 'earbuds', speakers: 'speakers',
	subwoofer: 'subwoofer', 'builtin-speakers': 'speakers', dac: 'dac', 'dac-amp': 'dac',
	interface: 'dac', receiver: 'amp', amp: 'amp', preamp: 'amp', phono: 'amp',
	phone: 'phone', computer: 'laptop', dap: 'dap', turntable: 'turntable',
	cartridge: 'turntable', headshell: 'turntable', radio: 'radio', transport: 'cdplayer'
};
export const ICON_FOR_ROLE: Record<string, string> = {
	dac: 'dac', amp: 'amp', transducer: 'speakers', bluetooth: 'earbuds', transport: 'cdplayer'
};

export const NODE_W = 210;
export const NODE_H = 64;
const COL_GAP = 70;
const ROW_GAP = 18;
const SUB_GAP = 24;

/** Other gear that can be on at a slot this box is wired at. */
export interface SlotOption {
	name: string;
	/** Its hours through that chain. */
	hours: number;
	chainId: number;
	equipmentId: number;
	/** It is the one on now, instead of this box. */
	on: boolean;
}

export interface GearNodeData {
	[key: string]: unknown;
	key: string;
	name: string;
	/** Brand and model for your own gear, the role for a typed-in step. */
	sub: string;
	/** Hours as `equipment_seconds` derives them; null for a typed-in step,
	 *  which has no row to hold them. */
	hours: number | null;
	column: number;
	icon: string | null;
	equipmentId: number | null;
	builtIn: boolean;
	/** The chains this box is in, by id. */
	chains: number[];
	/** Gear that can be on instead of this box at a slot, across every chain. */
	options: SlotOption[];
}

export interface LaidNode {
	id: string;
	x: number;
	y: number;
	data: GearNodeData;
}

export interface LaidEdge {
	id: string;
	source: string;
	target: string;
	chains: number[];
}

/** One room's horizontal band behind its rows. `roomId` is null for the
 *  "No room yet" band, drawn only when some chains have a room and some not. */
export interface LaidBand {
	key: string;
	roomId: number | null;
	name: string;
	kind: string;
	icon: string | null;
	y: number;
	height: number;
	chainCount: number;
}

export interface Layout {
	nodes: LaidNode[];
	edges: LaidEdge[];
	bands: LaidBand[];
	/** Left edge of each column, for the headings above it. */
	columnX: number[];
	width: number;
	height: number;
}

export function stepKey(step: ChainComponent): string {
	if (step.equipment_id != null) return `g${step.equipment_id}`;
	return `t:${step.type}:${step.name.trim().toLowerCase()}`;
}

/** The keys of every box a chain lights up: its own steps and the gear its
 *  slots swap in. */
export function chainKeys(c: SignalChain): string[] {
	return (c.components ?? []).filter((s) => s.name.trim()).map(stepKey);
}

export function edgeKey(a: string, b: string): string {
	return `${a}→${b}`;
}

const TYPE_LABEL: Record<string, string> = {
	source: 'Source',
	transport: 'Transport',
	network: 'Network',
	dac: 'DAC',
	amp: 'Amplifier',
	transducer: 'Headphones / speakers',
	bluetooth: 'Bluetooth'
};

/** A room's glyph: its own icon, or the built-in's. */
export function roomGlyph(r: Pick<Room, 'icon' | 'kind'>): string | null {
	return r.icon ?? (r.kind === 'portable' ? 'go' : r.kind === 'elsewhere' ? 'away' : null);
}

const BAND_PAD = 16;
const BAND_GAP = 22;

export function buildLayout(
	chains: SignalChain[],
	onNow: Record<string, Record<string, number>>,
	equipment: Equipment[],
	rooms: Room[] = []
): Layout {
	const gear = new Map(equipment.map((e) => [e.id, e]));
	const nodes = new Map<string, GearNodeData>();
	// How far into a run of same-column steps a box sits: a turntable then its
	// phono stage are both "Plays from"/"Amplifies" neighbours, but a turntable
	// then a cartridge share a column and need two sub-columns.
	const sub = new Map<string, number>();
	const edges = new Map<string, LaidEdge>();
	// Free-text steps have no hours row; sum the chains they sit in instead.
	const typedHours = new Map<string, number>();

	for (const c of chains) {
		const steps = (c.components ?? []).filter((s) => s.name.trim());
		let prevKey: string | null = null;
		let prevCol = -1;
		let run = 0;
		const seen = new Set<string>();
		for (const s of steps) {
			const key = stepKey(s);
			const col = columnOf(s.type);
			// A box can only move right along a line. A step whose role reads
			// "earlier" than the one before it (a DAC after the speakers) is
			// kept in the previous column rather than drawn as a backwards wire.
			const effCol = Math.max(col, prevCol);
			run = effCol === prevCol ? run + 1 : 0;
			const g = s.equipment_id != null ? gear.get(s.equipment_id) : undefined;
			let n = nodes.get(key);
			if (!n) {
				n = {
					key,
					name: g?.name ?? s.name,
					// A box its apps are tied to says who reports from it (the
					// design's reporter chips), which is the question the graph
					// answers about a phone or a laptop; otherwise brand and model.
					sub: g
						? g.reporters?.length
							? g.reporters.join(' · ')
							: [g.brand, g.model].filter(Boolean).join(' ') || (TYPE_LABEL[s.type] ?? s.type)
						: (TYPE_LABEL[s.type] ?? s.type),
					hours: g ? g.total_hours : null,
					column: effCol,
					icon: (g ? ICON_FOR_GEAR[g.equipment_type] : ICON_FOR_ROLE[s.type]) ?? null,
					equipmentId: g?.id ?? null,
					builtIn: g ? BUILT_IN.has(g.equipment_type) : false,
					chains: [],
					options: []
				};
				nodes.set(key, n);
			} else {
				n.column = Math.max(n.column, effCol);
			}
			if (!seen.has(key)) {
				seen.add(key);
				n.chains.push(c.id);
				if (!g) typedHours.set(key, (typedHours.get(key) ?? 0) + c.total_hours);
			}
			sub.set(key, Math.max(sub.get(key) ?? 0, run));
			if (prevKey && prevKey !== key) {
				const id = edgeKey(prevKey, key);
				const e = edges.get(id) ?? { id, source: prevKey, target: key, chains: [] };
				if (!e.chains.includes(c.id)) e.chains.push(c.id);
				edges.set(id, e);
			}
			prevKey = key;
			prevCol = effCol;
		}
	}
	for (const [key, h] of typedHours) {
		const n = nodes.get(key);
		if (n && n.hours == null) n.hours = h;
	}

	// Slot options on the box the step is wired with. Drawn on that box, not
	// as boxes of their own: a stylus you swap is a choice at one position,
	// and a second box would read as a second chain.
	for (const c of chains) {
		(c.components ?? []).forEach((step, i) => {
			if ((step.options?.length ?? 0) < 2 || !step.name.trim()) return;
			const n = nodes.get(stepKey(step));
			if (!n) return;
			const on = onNow[String(c.id)]?.[String(i)];
			for (const eq of step.options ?? []) {
				if (eq === step.equipment_id) continue;
				const g = gear.get(eq);
				const hours = (g?.sources ?? []).filter((s) => s.chain_id === c.id).reduce((a, s) => a + s.hours, 0);
				n.options.push({ name: g?.name ?? 'Gone from your gear', hours, chainId: c.id, equipmentId: eq, on: on === eq });
			}
		});
	}

	// ── x: columns, widened by however many sub-columns a run needs ──
	const maxSub = [0, 0, 0, 0];
	for (const [key, n] of nodes) maxSub[n.column] = Math.max(maxSub[n.column], sub.get(key) ?? 0);
	const columnX: number[] = [];
	let x = 0;
	for (let col = 0; col < COLUMNS.length; col++) {
		columnX.push(x);
		x += (maxSub[col] + 1) * NODE_W + maxSub[col] * SUB_GAP + COL_GAP;
	}
	const width = x - COL_GAP;
	const xOf = (n: GearNodeData) =>
		columnX[n.column] + (sub.get(n.key) ?? 0) * (NODE_W + SUB_GAP);

	// ── Which room's band each box sits in ──
	// A box can be in chains in several rooms (headphones on the desk and on
	// the commute). It is drawn once, in the room where it has the most hours,
	// and its wires reach into the others. A chain in two rooms counts in both.
	const roomsOf = new Map<number, number[]>();
	for (const r of rooms) for (const cid of r.chain_ids) roomsOf.set(cid, [...(roomsOf.get(cid) ?? []), r.id]);
	const banded = roomsOf.size > 0;
	const order = new Map(rooms.map((r, i) => [r.id, i]));
	const chainHours = new Map(chains.map((c) => [c.id, c.total_hours]));
	const bandOf = new Map<string, number | null>();
	for (const n of nodes.values()) {
		if (!banded) {
			bandOf.set(n.key, null);
			continue;
		}
		const weight = new Map<number, number>();
		for (const cid of n.chains)
			for (const rid of roomsOf.get(cid) ?? [])
				// A little per chain, so a room still wins on chains alone when
				// nothing has logged hours yet.
				weight.set(rid, (weight.get(rid) ?? 0) + (chainHours.get(cid) ?? 0) + 0.001);
		const best = [...weight.entries()].sort(
			(a, b) => b[1] - a[1] || (order.get(a[0]) ?? 0) - (order.get(b[0]) ?? 0)
		)[0];
		bandOf.set(n.key, best ? best[0] : null);
	}
	const bandKeys: (number | null)[] = banded
		? [
				...rooms.filter((r) => [...bandOf.values()].includes(r.id)).map((r) => r.id),
				...([...bandOf.values()].includes(null) ? [null] : [])
			]
		: [null];

	// ── y: one barycentre pass per band, rank by rank, left to right ──
	const incoming = new Map<string, string[]>();
	for (const e of edges.values()) {
		const list = incoming.get(e.target) ?? [];
		list.push(e.source);
		incoming.set(e.target, list);
	}
	const rankOf = (n: GearNodeData) => n.column * 10 + (sub.get(n.key) ?? 0);
	const y = new Map<string, number>();
	const step = NODE_H + ROW_GAP;
	// Ties break by hours, then by name, so equal inputs always draw the same.
	const byWeight = (a: GearNodeData, b: GearNodeData) =>
		(b.hours ?? 0) - (a.hours ?? 0) || a.name.localeCompare(b.name);
	const bands: LaidBand[] = [];
	let top = 0;
	for (const bk of bandKeys) {
		const inBand = [...nodes.values()].filter((n) => bandOf.get(n.key) === bk);
		if (!inBand.length) continue;
		const ranks = [...new Set(inBand.map(rankOf))].sort((a, b) => a - b);
		const start = top + (banded ? BAND_PAD : 0);
		let bottom = start;
		for (const r of ranks) {
			const here = inBand.filter((n) => rankOf(n) === r);
			// Only boxes in this band pull on each other; a wire from another
			// room's band must not drag a box out of its own.
			const want = (n: GearNodeData) => {
				const ys = (incoming.get(n.key) ?? [])
					.filter((k) => bandOf.get(k) === bk)
					.map((k) => y.get(k))
					.filter((v) => v != null) as number[];
				return ys.length ? ys.reduce((a, b) => a + b, 0) / ys.length : Number.POSITIVE_INFINITY;
			};
			here.sort((a, b) => want(a) - want(b) || byWeight(a, b));
			let next = start;
			for (const n of here) {
				const w = want(n);
				const at = Number.isFinite(w) ? Math.max(w, next) : next;
				y.set(n.key, at);
				next = at + step;
			}
			bottom = Math.max(bottom, next - ROW_GAP);
		}
		const end = bottom + (banded ? BAND_PAD : 0);
		if (banded) {
			const room = bk == null ? null : rooms.find((r) => r.id === bk);
			bands.push({
				key: `band:${bk ?? 'none'}`,
				roomId: bk,
				name: room?.name ?? 'No room yet',
				kind: room?.kind ?? 'none',
				icon: room ? roomGlyph(room) : null,
				y: top,
				height: end - top,
				chainCount: bk == null
					? chains.filter((c) => !roomsOf.has(c.id)).length
					: (room?.chain_ids.length ?? 0)
			});
		}
		top = end + (banded ? BAND_GAP : ROW_GAP);
	}
	const height = Math.max(0, top - (banded ? BAND_GAP : ROW_GAP));

	return {
		nodes: [...nodes.values()].map((d) => ({ id: d.key, x: xOf(d), y: y.get(d.key) ?? 0, data: d })),
		edges: [...edges.values()],
		bands,
		columnX,
		width,
		height
	};
}

/** Every equipment id a chain, a slot option or a setup names, for telling
 *  the shelf apart. */
export function gearInUse(chains: SignalChain[], variants: ChainVariant[]): Set<number> {
	const used = new Set<number>();
	for (const c of chains)
		for (const s of c.components ?? []) {
			if (s.equipment_id != null) used.add(s.equipment_id);
			for (const eq of s.options ?? []) used.add(eq);
		}
	for (const v of variants) for (const o of v.overrides ?? []) if (o.equipment_id != null) used.add(o.equipment_id);
	return used;
}
