<script lang="ts">
	// One piece of gear on the Signal Chains graph: drawn once, however many
	// chains run through it.
	//
	// Svelte 4 idiom on purpose, inside a Svelte Flow canvas that is written in
	// runes: Svelte 5 lets a legacy-mode component be a runes library's custom
	// node, and Svelte Flow hands its node props over like any parent would.
	import { Handle, Position } from '@xyflow/svelte';
	import ChainIcon from '$lib/components/ChainIcon.svelte';
	import { formatHours } from '$lib/utils';
	import type { GearNodeData } from '../layout';

	export let data: GearNodeData;

	$: options = data.options ?? [];
	$: optionTitle = options.length
		? `Swapped in by a setup: ${options.map((o) => `${o.name}${o.hours > 0 ? ` (${formatHours(o.hours)})` : ''}`).join(', ')}`
		: '';
</script>

<div class="gnode" class:builtin={data.builtIn}>
	<Handle type="target" position={Position.Left} isConnectable={false} />
	<span class="gicon" aria-hidden="true">
		{#if data.icon}<ChainIcon name={data.icon} size={16} title="" />{:else}<span class="gdot"></span>{/if}
	</span>
	<span class="gtext">
		<span class="gname">{data.name}</span>
		<span class="gsub">
			{data.sub}{#if options.length}<span class="swap" title={optionTitle}
					>⇄ {options.length} {options.length === 1 ? 'swap' : 'swaps'}</span
				>{/if}
		</span>
	</span>
	{#if data.hours != null && data.hours > 0}<span class="ghours">{formatHours(data.hours)}</span>{/if}
	<Handle type="source" position={Position.Right} isConnectable={false} />
</div>

<style>
	.gnode {
		width: 210px;
		height: 64px;
		box-sizing: border-box;
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 0 14px;
		border-radius: 18px;
		background: var(--td-surface);
		border: 1.5px solid var(--td-border);
		color: var(--td-text);
		transition: opacity 0.15s, border-color 0.15s;
	}
	.gnode.builtin {
		border-style: dashed;
	}
	.gicon {
		flex: none;
		width: 30px;
		height: 30px;
		border-radius: 999px;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--td-surface2);
		color: var(--td-muted);
	}
	.gdot {
		width: 7px;
		height: 7px;
		border-radius: 999px;
		background: var(--td-muted-3);
	}
	.gtext {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
	}
	.gname {
		font-size: 13.5px;
		font-weight: 600;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.gsub {
		font-size: 11.5px;
		color: var(--td-muted-2);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.swap {
		margin-left: 6px;
		padding: 0 6px;
		border-radius: 999px;
		border: 1px dashed var(--td-muted-3);
		color: var(--td-muted);
	}
	.ghours {
		flex: none;
		font-size: 11.5px;
		font-weight: 600;
		color: var(--td-muted);
	}
	/* Lit and dim come from the node's class, set by the graph on the wrapper. */
	:global(.svelte-flow__node.lit) .gnode {
		border-color: var(--td-accent);
	}
	:global(.svelte-flow__node.lit) .gicon {
		background: var(--td-accent);
		color: var(--td-on-accent);
	}
	:global(.svelte-flow__node.dim) .gnode {
		opacity: 0.45;
	}
	:global(.svelte-flow__node.away) .gnode {
		opacity: 0.16;
	}
</style>
