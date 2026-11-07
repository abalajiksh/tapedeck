<script lang="ts">
	// "How it's wired": every chain at once, each piece of gear drawn once, the
	// selected chain's route lit.
	//
	// Browse mode only for now (phase 1). The list beside it is the way to
	// navigate; this is a picture of the same thing, so keyboard users never
	// have to work through a canvas to reach a chain. Highlighting is derived
	// state — lit and dim classes on the same nodes — and never re-lays out.
	import { createEventDispatcher } from 'svelte';
	import { SvelteFlow, Background, Controls, type Node, type Edge } from '@xyflow/svelte';
	import '@xyflow/svelte/dist/style.css';
	import type { Equipment, Room, SignalChain } from '$lib/api';
	import { formatHours } from '$lib/utils';
	import GearNode from './nodes/GearNode.svelte';
	import BandNode from './nodes/BandNode.svelte';
	import FitOnResize from './FitOnResize.svelte';
	import { buildLayout, chainKeys, edgeKey, COLUMNS, NODE_W, type GearNodeData } from './layout';

	export let chains: SignalChain[] = [];
	/** chain id → (step position → gear on now), for chains with slots. */
	export let onNow: Record<string, Record<string, number>> = {};
	export let equipment: Equipment[] = [];
	export let selectedId: number | null = null;
	export let rooms: Room[] = [];
	/** The room filter. Everything outside it dims and the view fits its band;
	 *  nothing moves. */
	export let roomId: number | null = null;

	const dispatch = createEventDispatcher<{ select: number }>();
	const nodeTypes = { gear: GearNode, band: BandNode };

	$: layout = buildLayout(chains, onNow, equipment, rooms);
	$: room = roomId == null ? null : (rooms.find((r) => r.id === roomId) ?? null);
	$: roomChains = new Set(room?.chain_ids ?? []);
	$: inRoom = (chainIds: number[]) => !room || chainIds.some((c) => roomChains.has(c));
	$: selected = chains.find((c) => c.id === selectedId) ?? null;
	$: litNodes = new Set(selected ? chainKeys(selected) : []);
	$: litEdges = new Set(
		selected
			? chainKeys(selected).flatMap((k, i, ks) => (i ? [edgeKey(ks[i - 1], k)] : []))
			: []
	);

	function label(d: GearNodeData): string {
		const n = d.chains.length;
		const hrs = d.hours ? `, ${formatHours(d.hours)}` : '';
		return `${d.name}, ${d.sub}${hrs}, in ${n} ${n === 1 ? 'chain' : 'chains'}`;
	}

	const HEAD_Y = -42;
	// Bands run the full width with their label in a gutter on the right.
	const BAND_X = -24;
	const BAND_LABEL = 132;
	$: banded = layout.bands.length > 0;
	$: bandWidth = layout.width - 2 * BAND_X + BAND_LABEL;
	$: drawnWidth = banded ? bandWidth : layout.width;

	// Column headings ride along as non-interactive nodes so they pan and zoom
	// with the graph instead of floating over it.
	$: heads = COLUMNS.map(
		(title, i): Node => ({
			id: `head:${i}`,
			type: 'default',
			position: { x: layout.columnX[i], y: HEAD_Y },
			data: { label: title },
			class: 'colhead',
			draggable: false,
			selectable: false,
			focusable: false,
			connectable: false,
			width: NODE_W
		})
	);

	let nodes: Node[] = [];
	let edges: Edge[] = [];
	$: nodes = [
		...layout.bands.map(
			(b): Node => ({
				id: b.key,
				type: 'band',
				position: { x: BAND_X, y: b.y },
				data: { name: b.name, chainCount: b.chainCount, icon: b.icon, kind: b.kind },
				class: room ? (b.roomId === room.id ? 'on' : 'off') : '',
				width: bandWidth,
				height: b.height,
				zIndex: -1,
				draggable: false,
				selectable: false,
				focusable: false,
				connectable: false
			})
		),
		...heads,
		...layout.nodes.map(
			(n): Node => ({
				id: n.id,
				type: 'gear',
				position: { x: n.x, y: n.y },
				data: n.data,
				class: !inRoom(n.data.chains)
					? 'away'
					: selected
						? litNodes.has(n.id)
							? 'lit'
							: 'dim'
						: '',
				draggable: false,
				connectable: false,
				ariaLabel: label(n.data)
			})
		)
	];
	$: edges = layout.edges.map(
		(e): Edge => ({
			id: e.id,
			source: e.source,
			target: e.target,
			class: !inRoom(e.chains) ? 'away' : selected ? (litEdges.has(e.id) ? 'lit' : 'dim') : '',
			animated: litEdges.has(e.id),
			selectable: false,
			focusable: false
		})
	);

	// A box in several chains selects the one already selected if it is among
	// them, and otherwise the one with the most hours through it.
	function pick(key: string) {
		const n = layout.nodes.find((x) => x.id === key);
		if (!n) return;
		if (selectedId != null && n.data.chains.includes(selectedId)) {
			dispatch('select', selectedId);
			return;
		}
		const best = chains
			.filter((c) => n.data.chains.includes(c.id))
			.sort((a, b) => b.total_hours - a.total_hours)[0];
		if (best) dispatch('select', best.id);
	}

	// Sized from the width it actually gets: the graph is fitted to the width,
	// so a height taken from the drawing alone leaves a band of empty canvas
	// whenever the panel is narrower than the drawing. Never zoomed past 1:1.
	let width = 0;
	const PAD = 48;
	$: scale = width ? Math.min(1, width / (drawnWidth + PAD)) : 1;

	// What the view fits when a room is picked: its band, plus any box of one of
	// its chains drawn in another room's band. Null fits everything.
	$: focus = (() => {
		if (!room) return null;
		const band = layout.bands.find((b) => b.roomId === room.id);
		const boxes = layout.nodes.filter((n) => inRoom(n.data.chains));
		if (!band && !boxes.length) return null;
		const ys = [...(band ? [band.y, band.y + band.height] : []), ...boxes.flatMap((n) => [n.y, n.y + 64])];
		const top = Math.min(...ys);
		return { x: BAND_X, y: top, width: banded ? bandWidth : layout.width, height: Math.max(...ys) - top };
	})();
	$: height = Math.round(Math.min(720, Math.max(240, (layout.height - HEAD_Y + PAD) * scale + PAD)));
</script>

<div class="graph" bind:clientWidth={width} style="height:{height}px;">
	<!-- Not drawn until its width is known, so the first fit is to the real size. -->
	{#if layout.nodes.length && width}
		<SvelteFlow
			bind:nodes
			bind:edges
			{nodeTypes}
			fitView
			fitViewOptions={{ padding: 0.06, maxZoom: 1 }}
			minZoom={0.12}
			maxZoom={1.6}
			nodesDraggable={false}
			nodesConnectable={false}
			elementsSelectable={false}
			zoomOnScroll={false}
			panOnScroll={false}
			preventScrolling={false}
			zoomOnDoubleClick={false}
			proOptions={{ hideAttribution: true }}
			onnodeclick={({ node }) => pick(node.id)}
		>
			<FitOnResize key="{width}x{height}:{layout.nodes.length}:{roomId ?? 'all'}" bounds={focus} />
			<Background gap={22} size={1.4} />
			<Controls showLock={false} />
		</SvelteFlow>
	{:else if !layout.nodes.length}
		<p class="empty">No chains to draw yet. Make one and its route appears here.</p>
	{/if}
</div>

<style>
	.graph {
		width: 100%;
		border-radius: 18px;
		overflow: hidden;
		background: var(--td-bg);
	}
	.empty {
		margin: 0;
		padding: 40px;
		text-align: center;
		color: var(--td-muted);
		font-size: 13.5px;
	}
	/* Column headings: text, not boxes. */
	.graph :global(.svelte-flow__node.colhead) {
		background: none;
		border: none;
		box-shadow: none;
		padding: 0;
		text-align: left;
		font-size: 11px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		font-weight: 700;
		color: var(--td-muted-2);
		pointer-events: none;
	}
	.graph :global(.svelte-flow__node.colhead .svelte-flow__handle) {
		display: none;
	}
	.graph :global(.svelte-flow__node-gear) {
		cursor: pointer;
	}
	.graph :global(.svelte-flow__handle) {
		width: 7px;
		height: 7px;
		min-width: 0;
		min-height: 0;
		opacity: 0.7;
	}
	.graph :global(.svelte-flow__edge path) {
		stroke-width: 2;
	}
	.graph :global(.svelte-flow__edge.lit path) {
		stroke: var(--td-accent);
		stroke-width: 3;
	}
	.graph :global(.svelte-flow__edge.dim path) {
		opacity: 0.45;
	}
	.graph :global(.svelte-flow__edge.away path) {
		opacity: 0.1;
	}
	.graph :global(.svelte-flow__node-band) {
		pointer-events: none;
		cursor: default;
	}
</style>
