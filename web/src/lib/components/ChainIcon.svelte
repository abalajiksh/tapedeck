<script lang="ts" context="module">
	/**
	 * The fixed set a chain's icon can be. Exported so the picker and the
	 * renderer cannot drift — the picker offers exactly what this draws.
	 *
	 * Kept deliberately small. These stand for *how you were listening*, which
	 * is a handful of real situations, not a catalogue of equipment: the gear
	 * itself is already named in the chain's components.
	 */
	export const CHAIN_ICONS = [
		'headphones',
		'iem',
		'speakers',
		'turntable',
		'desk',
		'phone',
		'car',
		'radio'
	] as const;
	export type ChainIconName = (typeof CHAIN_ICONS)[number];

	export const CHAIN_ICON_LABELS: Record<string, string> = {
		headphones: 'Headphones',
		iem: 'In-ears',
		speakers: 'Speakers',
		turntable: 'Turntable',
		desk: 'Desk / computer',
		phone: 'Phone',
		car: 'Car',
		radio: 'Radio / hi-fi'
	};
</script>

<script lang="ts">
	/** Unknown or absent falls back to the generic mark — never an error, and
	 *  never anything the value itself can influence beyond picking a branch. */
	export let name: string | null | undefined = null;
	export let size = 15;
	export let title: string | null = null;

	$: label = title ?? CHAIN_ICON_LABELS[name ?? ''] ?? 'Signal chain';
</script>

<span class="ci" title={label} aria-label={label} role="img" style="width:{size}px;height:{size}px;">
	<svg viewBox="0 0 16 16" width={size} height={size} fill="none" stroke="currentColor"
		stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">
		{#if name === 'headphones'}
			<path d="M2.6 10.4V8a5.4 5.4 0 0 1 10.8 0v2.4" />
			<rect x="1.4" y="9.6" width="3" height="4.6" rx="1.5" />
			<rect x="11.6" y="9.6" width="3" height="4.6" rx="1.5" />
		{:else if name === 'iem'}
			<path d="M5.4 5.6a2.6 2.6 0 1 1 3.4 2.5c-.9.3-1.2 1-1.2 1.9v2.2" />
			<circle cx="6.2" cy="13.2" r="1.4" />
			<path d="M10.6 3.4a4.6 4.6 0 0 1 1.9 3.4" />
		{:else if name === 'speakers'}
			<rect x="2.2" y="1.4" width="11.6" height="13.2" rx="1.6" />
			<circle cx="8" cy="10.2" r="2.7" /><circle cx="8" cy="4.4" r="1.1" />
		{:else if name === 'turntable'}
			<rect x="1.4" y="2.6" width="13.2" height="10.8" rx="1.6" />
			<circle cx="7" cy="8" r="3.4" /><circle cx="7" cy="8" r="0.5" fill="currentColor" />
			<path d="M12.4 4.6v3.2l-2 2.2" />
		{:else if name === 'desk'}
			<rect x="1.4" y="2.6" width="13.2" height="8.4" rx="1.4" />
			<path d="M5.4 14h5.2" /><path d="M8 11v3" />
		{:else if name === 'phone'}
			<rect x="4" y="1.2" width="8" height="13.6" rx="1.8" />
			<line x1="6.8" y1="12.4" x2="9.2" y2="12.4" />
		{:else if name === 'car'}
			<path d="M2 10.6h12" />
			<path d="M3.4 10.6 4.6 6.4a1.4 1.4 0 0 1 1.35-1h4.1a1.4 1.4 0 0 1 1.35 1l1.2 4.2" />
			<circle cx="4.9" cy="12.2" r="1.3" /><circle cx="11.1" cy="12.2" r="1.3" />
		{:else if name === 'radio'}
			<rect x="1.4" y="5" width="13.2" height="9.2" rx="1.6" />
			<path d="M4.4 5 11 1.9" />
			<circle cx="10.8" cy="9.6" r="2.3" /><circle cx="4.6" cy="8.4" r="0.7" fill="currentColor" />
		{:else}
			<!-- Generic: a chain of two links, which is what this is. -->
			<path d="M6.6 9.4a2.6 2.6 0 0 0 3.9.3l1.9-1.9a2.6 2.6 0 0 0-3.7-3.7l-1.1 1" />
			<path d="M9.4 6.6a2.6 2.6 0 0 0-3.9-.3L3.6 8.2a2.6 2.6 0 0 0 3.7 3.7l1.1-1" />
		{/if}
	</svg>
</span>

<style>
	.ci {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		flex: none;
		color: var(--td-muted-3);
		vertical-align: -2px;
	}
</style>
