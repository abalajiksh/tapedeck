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

import type { ChainComponent, ChainVariant, Equipment, SignalChain } from '$lib/api';

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
const BUILT_IN = new Set(['builtin-speakers']);

/** A `ChainIcon` glyph for a box, from its gear type or, for a typed-in step,
 *  its role. */
const ICON_FOR_GEAR: Record<string, string> = {
	headphones: 'headphones', iem: 'iem', earbuds: 'earbuds', speakers: 'speakers',
	subwoofer: 'subwoofer', 'builtin-speakers': 'speakers', dac: 'dac', 'dac-amp': 'dac',
	interface: 'dac', receiver: 'amp', amp: 'amp', preamp: 'amp', phono: 'amp',
	phone: 'phone', computer: 'laptop', dap: 'dap', turntable: 'turntable',
	cartridge: 'turntable', headshell: 'turntable', radio: 'radio', transport: 'cdplayer'
};
const ICON_FOR_ROLE: Record<string, string> = {
	dac: 'dac', amp: 'amp', transducer: 'speakers', bluetooth: 'earbuds', transport: 'cdplayer'
};

export const NODE_W = 210;
export const NODE_H = 64;
const COL_GAP = 70;
const ROW_GAP = 18;
const SUB_GAP = 24;

/** One way a step can be swapped, from a named setup that overrides it. */
export interface SlotOption {
	name: string;
	hours: number;
	chainId: number;
	variantId: number;
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
	/** Named setups that swap this box for another, across every chain. */
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

export interface Layout {
	nodes: LaidNode[];
	edges: LaidEdge[];
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
 *  setups swap in. */
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

export function buildLayout(
	chains: SignalChain[],
	variants: ChainVariant[],
	equipment: Equipment[]
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
					sub: g
						? [g.brand, g.model].filter(Boolean).join(' ') || (TYPE_LABEL[s.type] ?? s.type)
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

	// Setups as slot options on the box they swap out. Drawn on that box, not
	// as boxes of their own: a stylus you swap is a choice at one position, and
	// a second box would read as a second chain.
	for (const v of variants) {
		const c = chains.find((x) => x.id === v.chain_id);
		if (!c) continue;
		for (const o of v.overrides ?? []) {
			const step = c.components?.[o.position];
			if (!step || !step.name.trim()) continue;
			const n = nodes.get(stepKey(step));
			if (!n) continue;
			const name =
				o.name ?? (o.equipment_id != null ? gear.get(o.equipment_id)?.name : null) ?? '—';
			n.options.push({ name, hours: v.total_hours, chainId: c.id, variantId: v.id });
		}
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

	// ── y: one barycentre pass, rank by rank, left to right ──
	const incoming = new Map<string, string[]>();
	for (const e of edges.values()) {
		const list = incoming.get(e.target) ?? [];
		list.push(e.source);
		incoming.set(e.target, list);
	}
	const rankOf = (n: GearNodeData) => n.column * 10 + (sub.get(n.key) ?? 0);
	const ranks = [...new Set([...nodes.values()].map(rankOf))].sort((a, b) => a - b);
	const y = new Map<string, number>();
	const step = NODE_H + ROW_GAP;
	// Ties break by hours, then by name, so equal inputs always draw the same.
	const byWeight = (a: GearNodeData, b: GearNodeData) =>
		(b.hours ?? 0) - (a.hours ?? 0) || a.name.localeCompare(b.name);
	let height = 0;
	for (const r of ranks) {
		const here = [...nodes.values()].filter((n) => rankOf(n) === r);
		const want = (n: GearNodeData) => {
			const ys = (incoming.get(n.key) ?? []).map((k) => y.get(k)).filter((v) => v != null) as number[];
			return ys.length ? ys.reduce((a, b) => a + b, 0) / ys.length : Number.POSITIVE_INFINITY;
		};
		here.sort((a, b) => want(a) - want(b) || byWeight(a, b));
		let next = 0;
		for (const n of here) {
			const w = want(n);
			const at = Number.isFinite(w) ? Math.max(w, next) : next;
			y.set(n.key, at);
			next = at + step;
		}
		height = Math.max(height, next - ROW_GAP);
	}

	return {
		nodes: [...nodes.values()].map((d) => ({ id: d.key, x: xOf(d), y: y.get(d.key) ?? 0, data: d })),
		edges: [...edges.values()],
		columnX,
		width,
		height
	};
}

/** Every equipment id any chain or setup names, for telling the shelf apart. */
export function gearInUse(chains: SignalChain[], variants: ChainVariant[]): Set<number> {
	const used = new Set<number>();
	for (const c of chains) for (const s of c.components ?? []) if (s.equipment_id != null) used.add(s.equipment_id);
	for (const v of variants) for (const o of v.overrides ?? []) if (o.equipment_id != null) used.add(o.equipment_id);
	return used;
}
