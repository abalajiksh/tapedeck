<script lang="ts" context="module">
	/**
	 * The fixed set a chain's icon can be. Exported so the picker and the
	 * renderer cannot drift — the picker offers exactly what this draws.
	 *
	 * Ordered by family rather than alphabetically, because the picker renders
	 * this array directly and "everything I could put on my desk" next to
	 * "everywhere I might be listening" is how someone actually looks for one.
	 *
	 * This started as eight, on the reasoning that an icon stands for *how you
	 * were listening* and that is a handful of real situations. That held right
	 * up until a shelf of gear met it: a valve amp, a cassette deck and a DAC
	 * all landed on the same generic mark, which is the one thing the icon
	 * exists to avoid. The rule it keeps is the useful half of the original —
	 * an icon must be distinguishable at 15px, so near-duplicates of the same
	 * silhouette are still out.
	 */
	export const CHAIN_ICONS = [
		// Worn
		'headphones',
		'iem',
		'earbuds',
		// Boxes in a room
		'speakers',
		'monitors',
		'soundbar',
		'subwoofer',
		'smartspeaker',
		// Sources you handle
		'turntable',
		'tapedeck',
		'cdplayer',
		'reel',
		// The rack
		'amp',
		'tube',
		'dac',
		'mixer',
		'radio',
		'boombox',
		// Screens
		'desk',
		'laptop',
		'tv',
		'phone',
		'dap',
		// Places
		'car',
		'bike',
		'train',
		'plane',
		'outdoors',
		'night'
	] as const;
	export type ChainIconName = (typeof CHAIN_ICONS)[number];

	export const CHAIN_ICON_LABELS: Record<string, string> = {
		headphones: 'Headphones',
		iem: 'In-ears',
		earbuds: 'Wireless earbuds',
		speakers: 'Speakers',
		monitors: 'Studio monitors',
		soundbar: 'Soundbar',
		subwoofer: 'Subwoofer',
		smartspeaker: 'Smart speaker',
		turntable: 'Turntable',
		tapedeck: 'Cassette deck',
		cdplayer: 'CD / SACD player',
		reel: 'Reel to reel',
		amp: 'Amplifier',
		tube: 'Valve amp',
		dac: 'DAC',
		mixer: 'Mixer',
		radio: 'Radio / hi-fi',
		boombox: 'Boombox',
		desk: 'Desk / computer',
		laptop: 'Laptop',
		tv: 'TV',
		phone: 'Phone',
		dap: 'Portable player',
		car: 'Car',
		bike: 'Bike',
		train: 'Train',
		plane: 'Plane',
		outdoors: 'Outdoors',
		night: 'Late night'
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
		{:else if name === 'earbuds'}
			<!-- The case, open, with a bud in each well — the shape you actually
			     recognise, since a bud on its own reads as an in-ear. -->
			<rect x="1.6" y="6.4" width="12.8" height="7.4" rx="2.4" />
			<circle cx="5.4" cy="9.4" r="1.7" /><circle cx="10.6" cy="9.4" r="1.7" />
			<path d="M5.4 11.1v2.7" /><path d="M10.6 11.1v2.7" />
			<path d="M4 4.2h8" />
		{:else if name === 'speakers'}
			<rect x="2.2" y="1.4" width="11.6" height="13.2" rx="1.6" />
			<circle cx="8" cy="10.2" r="2.7" /><circle cx="8" cy="4.4" r="1.1" />
		{:else if name === 'monitors'}
			<!-- Nearfields: a squatter cabinet than a floorstander, on a stand,
			     which is what distinguishes the two at this size. -->
			<rect x="2.6" y="1.6" width="10.8" height="8.8" rx="1.4" />
			<circle cx="8" cy="7" r="2" /><circle cx="8" cy="3.8" r="0.8" />
			<path d="M8 10.4v3.2" /><path d="M5.2 14h5.6" />
		{:else if name === 'soundbar'}
			<rect x="1.2" y="6.2" width="13.6" height="4.4" rx="1.8" />
			{#each [4.2, 6.6, 9.4, 11.8] as cx}
				<circle cx={cx} cy="8.4" r="0.75" />
			{/each}
			<path d="M3.6 12.6h8.8" />
		{:else if name === 'subwoofer'}
			<!-- One big driver and a port, which is the whole visual signature. -->
			<rect x="2" y="2.4" width="12" height="11.2" rx="1.8" />
			<circle cx="8" cy="7.6" r="3.4" /><circle cx="8" cy="7.6" r="1" />
			<rect x="6.2" y="12" width="3.6" height="0.9" rx="0.45" />
		{:else if name === 'smartspeaker'}
			<path d="M4 13.6V6.4a4 4 0 0 1 8 0v7.2z" />
			<path d="M4.4 10.4h7.2" />
			<path d="M6.2 3.4a5.4 5.4 0 0 1 3.6 0" />
		{:else if name === 'turntable'}
			<rect x="1.4" y="2.6" width="13.2" height="10.8" rx="1.6" />
			<circle cx="7" cy="8" r="3.4" /><circle cx="7" cy="8" r="0.5" fill="currentColor" />
			<path d="M12.4 4.6v3.2l-2 2.2" />
		{:else if name === 'tapedeck'}
			<!-- Two hubs behind a window: a cassette, not a reel, because the
			     hubs are close together and the shell is a rounded slab. -->
			<rect x="1.4" y="3.4" width="13.2" height="9.2" rx="1.6" />
			<rect x="3.4" y="5.6" width="9.2" height="4.8" rx="1" />
			<circle cx="6.2" cy="8" r="1.15" /><circle cx="9.8" cy="8" r="1.15" />
		{:else if name === 'cdplayer'}
			<!-- A slot-loading transport with the disc half out. -->
			<rect x="1.4" y="4" width="13.2" height="8" rx="1.6" />
			<circle cx="6.6" cy="8" r="2.6" /><circle cx="6.6" cy="8" r="0.8" />
			<path d="M11 6.4h2.2" /><path d="M11 9.6h2.2" />
		{:else if name === 'reel'}
			<!-- Two large reels side by side with tape between them — the
			     proportions are what tell this apart from a cassette deck. -->
			<circle cx="4.6" cy="8" r="3.4" /><circle cx="11.4" cy="8" r="3.4" />
			<circle cx="4.6" cy="8" r="0.9" /><circle cx="11.4" cy="8" r="0.9" />
			<path d="M4.6 4.6v-.9" /><path d="M11.4 4.6v-.9" />
			<path d="M6.4 10.6h3.2" />
		{:else if name === 'amp'}
			<!-- A component with two big knobs and a row of vents: the
			     front panel of anything in a rack. -->
			<rect x="1.4" y="4.2" width="13.2" height="7.6" rx="1.4" />
			<circle cx="4.4" cy="8" r="1.5" /><circle cx="8" cy="8" r="1.5" />
			<path d="M11 6.6h2.4" /><path d="M11 8h2.4" /><path d="M11 9.4h2.4" />
		{:else if name === 'tube'}
			<!-- A valve: glass envelope, glowing plate, pins. -->
			<path d="M5 7.4a3 3 0 0 1 6 0v3.4a3 3 0 0 1-6 0z" />
			<path d="M5.6 5.4a3.4 3.4 0 0 1 4.8 0" />
			<path d="M8 8v2.2" />
			<path d="M6.6 13.4v1.2" /><path d="M8 13.4v1.2" /><path d="M9.4 13.4v1.2" />
			<path d="M5 10.8h6" />
		{:else if name === 'dac'}
			<!-- A stepped waveform becoming a smooth one: the conversion itself,
			     which is the only thing a DAC looks like. -->
			<rect x="1.2" y="3.6" width="13.6" height="8.8" rx="1.6" />
			<path d="M3.2 10V8h1.6V6.2h1.6" />
			<path d="M7.4 8.4c1.2-3.2 2.6-3.2 3.8 0s1 3.2 1.6 1.6" />
		{:else if name === 'mixer'}
			<!-- Faders. Nothing else reads as a desk at this size. -->
			<rect x="1.4" y="1.8" width="13.2" height="12.4" rx="1.6" />
			<path d="M4.6 4.4v7.2" /><path d="M8 4.4v7.2" /><path d="M11.4 4.4v7.2" />
			<circle cx="4.6" cy="9.4" r="1.05" /><circle cx="8" cy="6.4" r="1.05" />
			<circle cx="11.4" cy="8.4" r="1.05" />
		{:else if name === 'radio'}
			<rect x="1.4" y="5" width="13.2" height="9.2" rx="1.6" />
			<path d="M4.4 5 11 1.9" />
			<circle cx="10.8" cy="9.6" r="2.3" /><circle cx="4.6" cy="8.4" r="0.7" fill="currentColor" />
		{:else if name === 'boombox'}
			<!-- Two speakers, a handle, and a deck in the middle. -->
			<rect x="1.2" y="4.6" width="13.6" height="9" rx="1.6" />
			<path d="M5 4.6a3 3 0 0 1 6 0" />
			<circle cx="4.4" cy="9.2" r="2.1" /><circle cx="11.6" cy="9.2" r="2.1" />
			<rect x="7" y="7.8" width="2" height="2.8" rx="0.5" />
		{:else if name === 'desk'}
			<rect x="1.4" y="2.6" width="13.2" height="8.4" rx="1.4" />
			<path d="M5.4 14h5.2" /><path d="M8 11v3" />
		{:else if name === 'laptop'}
			<path d="M3 3.2h10a1 1 0 0 1 1 1v6H2v-6a1 1 0 0 1 1-1z" />
			<path d="M0.9 12.6h14.2" />
		{:else if name === 'tv'}
			<rect x="1.2" y="2.4" width="13.6" height="9" rx="1.4" />
			<path d="M5.4 14h5.2" /><path d="M6.6 4.6 10.4 6.9 6.6 9.2z" />
		{:else if name === 'phone'}
			<rect x="4" y="1.2" width="8" height="13.6" rx="1.8" />
			<line x1="6.8" y1="12.4" x2="9.2" y2="12.4" />
		{:else if name === 'dap'}
			<!-- A player, not a phone: a screen over a wheel, and a volume rocker
			     down the side. That silhouette is the whole difference. -->
			<rect x="3.4" y="1.2" width="9.2" height="13.6" rx="1.6" />
			<rect x="5" y="2.8" width="6" height="4.6" rx="0.6" />
			<circle cx="8" cy="11" r="2.2" /><circle cx="8" cy="11" r="0.55" />
			<path d="M2.4 5.4v2.2" />
		{:else if name === 'car'}
			<path d="M2 10.6h12" />
			<path d="M3.4 10.6 4.6 6.4a1.4 1.4 0 0 1 1.35-1h4.1a1.4 1.4 0 0 1 1.35 1l1.2 4.2" />
			<circle cx="4.9" cy="12.2" r="1.3" /><circle cx="11.1" cy="12.2" r="1.3" />
		{:else if name === 'bike'}
			<circle cx="3.8" cy="11.2" r="2.6" /><circle cx="12.2" cy="11.2" r="2.6" />
			<path d="M3.8 11.2 6.6 5.6h2.6l2.9 5.6" />
			<path d="M6.6 5.6h2.2" /><path d="M6.4 11.2h3.4" />
		{:else if name === 'train'}
			<rect x="3" y="1.8" width="10" height="9.4" rx="2" />
			<path d="M3.6 6.2h8.8" />
			<circle cx="5.8" cy="9" r="0.75" /><circle cx="10.2" cy="9" r="0.75" />
			<path d="M5.2 11.2 3.6 14.2" /><path d="M10.8 11.2l1.6 3" />
		{:else if name === 'plane'}
			<path d="M8 1.4c.9 0 1.4 1.1 1.4 2.4v2.5l4.6 2.8v1.6l-4.6-1.4v2.9l1.7 1.2v1.2L8 13.8l-3.1.8v-1.2l1.7-1.2V9.3L2 10.7V9.1l4.6-2.8V3.8c0-1.3.5-2.4 1.4-2.4z" />
		{:else if name === 'outdoors'}
			<!-- Walking with something on: a route, and a sun over it. -->
			<circle cx="11.6" cy="3.4" r="1.9" />
			<path d="M1.6 13.6c1.8 0 1.8-2.4 3.6-2.4s1.8 2.4 3.6 2.4 1.8-2.4 3.6-2.4 1.8 2.4 3.6 2.4" />
			<path d="M2.4 8.6 5.6 5l2.6 2.4L11 4.8" />
		{:else if name === 'night'}
			<path d="M13.2 9.6A5.6 5.6 0 0 1 6.4 2.8a5.8 5.8 0 1 0 6.8 6.8z" />
			<path d="M11.4 2.2v2" /><path d="M10.4 3.2h2" />
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
