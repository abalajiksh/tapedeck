<script lang="ts">
	// Hands the route editor what only a component inside <SvelteFlow> can
	// reach: turning a screen point into canvas coordinates (for a drop from
	// the palette) and fitting the view. Lives inside <SvelteFlow>, where
	// `useSvelteFlow` finds its context.
	import { useSvelteFlow } from '@xyflow/svelte';

	const flow = useSvelteFlow();
	export const toFlow = (x: number, y: number) => flow.screenToFlowPosition({ x, y });
	export const fit = (): void => {
		requestAnimationFrame(() => requestAnimationFrame(() => void flow.fitView({ padding: 0.2, maxZoom: 1 })));
	};
</script>
