<script context="module" lang="ts">
	/** The room glyphs, in picker order. The UI owns this vocabulary, as it
	 *  does `ChainIcon`'s: the server stores the name and never checks it. */
	export const ROOM_ICONS = ['desk', 'sofa', 'bed', 'pot', 'tv', 'car', 'plant'] as const;
	export const ROOM_ICON_LABELS: Record<string, string> = {
		desk: 'Study / desk',
		sofa: 'Living room',
		bed: 'Bedroom',
		pot: 'Kitchen',
		tv: 'TV room',
		car: 'Car',
		plant: 'Garden',
		go: 'On the go',
		away: 'Elsewhere'
	};
</script>

<script lang="ts">
	// A room's glyph: inline SVG in `currentColor`, on the same 16-unit grid
	// and stroke as `ChainIcon`, so the two sit side by side without one
	// looking heavier. Unknown or missing names draw a plain door.
	export let name: string | null | undefined = null;
	export let size = 16;
	export let title: string | null = null;

	$: label = title ?? ROOM_ICON_LABELS[name ?? ''] ?? 'Room';
</script>

<span class="ri" title={label || undefined} aria-label={label || undefined} role={label ? 'img' : undefined}
	aria-hidden={label ? undefined : 'true'} style="width:{size}px;height:{size}px;">
	<svg viewBox="0 0 16 16" width={size} height={size} fill="none" stroke="currentColor"
		stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">
		{#if name === 'desk'}
			<path d="M1.5 6.5h13M3 6.5v7M13 6.5v7M9.5 6.5v4h3.5" /><rect x="5" y="2" width="5" height="3.2" rx=".6" /><path d="M7.5 5.2v1.3" />
		{:else if name === 'sofa'}
			<path d="M3 8V5.5A1.5 1.5 0 0 1 4.5 4h7A1.5 1.5 0 0 1 13 5.5V8" /><path d="M2 8.5A1.5 1.5 0 0 1 4.5 8v2h7V8a1.5 1.5 0 0 1 2.5.5V12H2z" /><path d="M3.5 12v1.5M12.5 12v1.5" />
		{:else if name === 'bed'}
			<path d="M1.5 4v9.5M1.5 10h13v3.5M14.5 10V8.5a2 2 0 0 0-2-2H7v3.5" /><circle cx="4.3" cy="8" r="1.4" />
		{:else if name === 'pot'}
			<path d="M3 7h10v4.5a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" /><path d="M1.5 7h13M6 7V5.5h4V7" /><path d="M6.5 3.5c0-.8.6-.8.6-1.6M9 3.5c0-.8.6-.8.6-1.6" />
		{:else if name === 'tv'}
			<rect x="1.5" y="3.5" width="13" height="8.5" rx="1.2" /><path d="M5.5 14.5h5M6 1.5l2 2 2-2" />
		{:else if name === 'car'}
			<path d="M2 10.5V8.3l1.6-3.3A1.5 1.5 0 0 1 5 4h6a1.5 1.5 0 0 1 1.4 1l1.6 3.3v2.2a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z" /><path d="M2.5 8.3h11" /><circle cx="4.8" cy="11.6" r="1.1" /><circle cx="11.2" cy="11.6" r="1.1" />
		{:else if name === 'plant'}
			<path d="M5 10.5h6l-.8 3.5H5.8z" /><path d="M8 10.5V6M8 6c0-2 1.5-3.5 4-3.5 0 2.3-1.6 3.5-4 3.5zM8 7.5C8 6 6.8 5 5 5c0 1.7 1.2 2.5 3 2.5z" />
		{:else if name === 'go'}
			<rect x="3" y="4.5" width="10" height="9" rx="2" /><path d="M6 4.5V3a1 1 0 0 1 1-1h2a1 1 0 0 1 1 1v1.5M3 8.5h10M7 8.5v1.5h2V8.5" />
		{:else if name === 'away'}
			<path d="M2 7.5 8 2.5l6 5" /><path d="M3.5 6.5v7h9v-7" /><path d="M6.5 13.5V10h3v3.5" /><path d="M11.5 2.5h2v2" />
		{:else}
			<rect x="4" y="1.5" width="8" height="13" rx="1" /><circle cx="10" cy="8.5" r=".6" fill="currentColor" stroke="none" />
		{/if}
	</svg>
</span>

<style>
	.ri {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		flex: none;
	}
</style>
