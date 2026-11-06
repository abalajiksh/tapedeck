<script lang="ts">
	// Re-fits the graph whenever `key` changes. Svelte Flow fits once, on first
	// render, so a window resize or a chain added later would otherwise leave
	// the drawing cropped or floating in a corner. Lives inside <SvelteFlow>,
	// which is where `useSvelteFlow` finds its context.
	import { useSvelteFlow } from '@xyflow/svelte';

	export let key = '';

	const flow = useSvelteFlow();
	let last = '';
	$: if (key !== last) {
		last = key;
		// Two frames: one for the container's new size to land, one for Svelte
		// Flow's own resize observer to read it. Fitting sooner fits the old box.
		requestAnimationFrame(() =>
			requestAnimationFrame(() => void flow.fitView({ padding: 0.06, maxZoom: 1 }))
		);
	}
</script>
