<script lang="ts">
	// A room's band behind its rows on the graph. Not interactive: it is the
	// background the boxes sit on, and clicks pass through it to the canvas.
	import RoomIcon from '../RoomIcon.svelte';

	export let data: { name: string; chainCount: number; icon: string | null; kind: string };
</script>

<div class="band" class:none={data.kind === 'none'}>
	<div class="bl">
		<b><RoomIcon name={data.icon} size={13} title="" />{data.name}</b>
		<span>{data.chainCount} {data.chainCount === 1 ? 'chain' : 'chains'}{data.kind === 'portable' ? ' · goes with you' : ''}</span>
	</div>
</div>

<style>
	.band {
		position: relative;
		width: 100%;
		height: 100%;
		box-sizing: border-box;
		border-radius: 20px;
		background: color-mix(in srgb, var(--td-surface2) 60%, transparent);
		border: 1px solid var(--td-border);
		transition: opacity 0.15s, background 0.15s;
	}
	.band.none {
		border-style: dashed;
		background: none;
	}
	.bl {
		position: absolute;
		right: 14px;
		top: 12px;
		width: 104px;
		text-align: right;
	}
	.bl b {
		display: flex;
		justify-content: flex-end;
		align-items: center;
		gap: 5px;
		font-size: 11.5px;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--td-muted);
	}
	.bl span {
		display: block;
		font-size: 11.5px;
		color: var(--td-muted-2);
		margin-top: 2px;
	}
	:global(.svelte-flow__node.on) .band {
		background: color-mix(in srgb, var(--td-accent) 9%, transparent);
		border-color: color-mix(in srgb, var(--td-accent) 40%, transparent);
	}
	:global(.svelte-flow__node.on) .bl b {
		color: var(--td-accent);
	}
	:global(.svelte-flow__node.off) .band {
		opacity: 0.3;
	}
</style>
