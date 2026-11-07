<script lang="ts" context="module">
	/** One box in the route editor. The editor owns these fields; the node
	 *  only draws them. */
	export interface RouteNodeData {
		[key: string]: unknown;
		/** The gear this box is, or null for a typed-in step. */
		equipmentId: number | null;
		name: string;
		/** The chain role (`ComponentType`). */
		role: string;
		/** Brand/model, or the role for a typed-in step. */
		sub: string;
		icon: string | null;
		builtIn: boolean;
		detail: string | null;
		since: number | null;
		/** Gear that can be on here, the wired gear first; one entry means not
		 *  a slot. */
		options: number[];
		optionNames: string[];
		/** The option on now, when the box is a slot. */
		onNow: number | null;
		slotName: string | null;
		/** Gear being dragged over this box: "drop to add" it. */
		dropName: string | null;
		/** What is wrong with this box's wiring, after a save was refused. */
		problem: string | null;
	}
</script>

<script lang="ts">
	// A box on the route editor's canvas, with a socket on each side: signal in
	// on the left, out on the right. Patch cables go from a right socket to a
	// left one. A slot lists its options, and the one on now.
	import { Handle, Position } from '@xyflow/svelte';
	import ChainIcon from '$lib/components/ChainIcon.svelte';
	import { compTypeLabel } from '../gear';

	export let data: RouteNodeData;
	export let selected = false;

	$: slot = data.options.length >= 2;
</script>

<div class="rnode" class:slot class:builtin={data.builtIn} class:sel={selected} class:bad={!!data.problem}
	class:drop={!!data.dropName}>
	<Handle type="target" position={Position.Left} class="sock" />
	<div class="top">
		<span class="ic" aria-hidden="true">
			{#if data.icon}<ChainIcon name={data.icon} size={16} title="" />{:else}<span class="dot"></span>{/if}
		</span>
		<span class="tx">
			<span class="name">{slot ? (data.slotName ?? data.name) : data.name}</span>
			<span class="sub">{slot ? 'Swappable · pick one at a time' : `${compTypeLabel(data.role)}${data.sub ? ` · ${data.sub}` : ''}`}</span>
		</span>
	</div>
	{#if slot}
		<ul class="opts">
			{#each data.options as id, i (id)}
				<li class:on={data.onNow === id}>
					<span class="r" aria-hidden="true"></span>{data.optionNames[i]}
					{#if data.onNow === id}<span class="now">on now</span>{/if}
				</li>
			{/each}
		</ul>
	{/if}
	{#if data.dropName}<div class="hint">+ Drop to add {data.dropName} as an option</div>{/if}
	{#if data.problem}<div class="prob">{data.problem}</div>{/if}
	<Handle type="source" position={Position.Right} class="sock" />
</div>

<style>
	.rnode {
		width: 220px;
		padding: 10px 12px;
		border-radius: 16px;
		background: var(--td-surface);
		border: 1.5px solid var(--td-border-2);
		color: var(--td-text);
		font-size: 13px;
		box-shadow: 0 6px 18px rgba(0, 0, 0, 0.18);
	}
	.rnode.builtin {
		border-style: dashed;
	}
	.rnode.slot {
		border-style: dashed;
		border-color: var(--td-accent);
	}
	.rnode.sel {
		outline: 2px solid var(--td-accent);
		outline-offset: 2px;
	}
	.rnode.bad {
		border-color: var(--td-bad-fg);
	}
	.rnode.drop {
		background: color-mix(in srgb, var(--td-accent) 12%, var(--td-surface));
	}
	.top {
		display: flex;
		align-items: center;
		gap: 9px;
	}
	.ic {
		flex: none;
		width: 30px;
		height: 30px;
		border-radius: 10px;
		display: grid;
		place-items: center;
		background: var(--td-surface2);
		color: var(--td-muted);
	}
	.dot {
		width: 8px;
		height: 8px;
		border-radius: 999px;
		background: var(--td-muted-3);
	}
	.tx {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
	.name {
		font-weight: 700;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.sub {
		font-size: 11.5px;
		color: var(--td-muted-2);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.opts {
		list-style: none;
		margin: 8px 0 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.opts li {
		display: flex;
		align-items: center;
		gap: 7px;
		font-size: 12.5px;
	}
	.r {
		width: 11px;
		height: 11px;
		border-radius: 999px;
		border: 1.5px solid var(--td-muted-3);
		flex: none;
	}
	.opts li.on .r {
		border: 3.5px solid var(--td-accent);
	}
	.now {
		margin-left: auto;
		font-size: 10.5px;
		color: var(--td-accent);
		font-weight: 600;
	}
	.hint {
		margin-top: 8px;
		font-size: 12px;
		color: var(--td-accent);
		font-weight: 600;
	}
	.prob {
		margin-top: 6px;
		font-size: 11.5px;
		color: var(--td-bad-fg);
	}
	.rnode :global(.sock) {
		width: 12px;
		height: 12px;
		background: var(--td-accent);
		border: 2px solid var(--td-surface);
	}
</style>
