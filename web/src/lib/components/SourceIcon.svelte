<script lang="ts">
	/**
	 * Where a listen came from, as a glyph.
	 *
	 * These are **not** the services' logos. Reproducing Plex's or Jellyfin's
	 * trademark would be both legally murky and illegible at 15px, so the three
	 * library servers get a lettermark and everything genuinely different in
	 * kind gets a real glyph — a record looks like a record, a cassette like a
	 * cassette. That distinction is the one worth seeing at a glance: whether a
	 * listen came off a shelf, a server, or a phone.
	 *
	 * Everything draws in `currentColor` so it inherits the row's muted tone and
	 * works in both themes. No hardcoded hex, same rule as everywhere else.
	 */
	export let source: string | null = null;
	/** `submission_client` — names the app behind an `ingest:*` listen. */
	export let client: string | null = null;
	export let size = 15;

	// `source_name` is one of: Plex, jellyfin, emby, roon, navidrome, physical,
	// rockbox, import, spotify, radio, or `ingest:<client>`.
	$: key = (source ?? '').toLowerCase();
	$: kind = key.startsWith('ingest:')
		? 'push'
		: ['plex', 'jellyfin', 'emby', 'roon', 'navidrome', 'physical', 'rockbox', 'import', 'spotify', 'radio'].includes(key)
			? key
			: 'unknown';

	$: label =
		kind === 'push'
			? `Submitted by ${client || source?.slice(7) || 'a client'}`
			: kind === 'physical'
				? 'Played from the shelf'
				: kind === 'import'
					? 'Imported from a file'
					: kind === 'rockbox'
						? 'Rockbox player'
						: kind === 'spotify'
							? (client ?? 'Streamed on Spotify')
							: kind === 'radio'
								? (client ?? 'Heard on the radio')
							: kind === 'unknown'
								? (source ?? 'Unknown source')
								: key.charAt(0).toUpperCase() + key.slice(1);

	const LETTER: Record<string, string> = { plex: 'P', jellyfin: 'J', emby: 'E', navidrome: 'N', roon: 'R' };
</script>

<span class="si" title={label} aria-label={label} role="img" style="width:{size}px;height:{size}px;">
	{#if LETTER[kind]}
		<!-- Lettermark: unambiguous at this size, and not a trademark. -->
		<svg viewBox="0 0 16 16" width={size} height={size} fill="none" stroke="currentColor" stroke-width="1.3">
			<rect x="1.4" y="1.4" width="13.2" height="13.2" rx="3.6" />
			<text x="8" y="11.4" text-anchor="middle" font-size="8.5" font-weight="600"
				fill="currentColor" stroke="none" font-family="inherit">{LETTER[kind]}</text>
		</svg>
	{:else if kind === 'physical'}
		<!-- A record: the label and the groove. -->
		<svg viewBox="0 0 16 16" width={size} height={size} fill="none" stroke="currentColor" stroke-width="1.3">
			<circle cx="8" cy="8" r="6.6" />
			<circle cx="8" cy="8" r="2.4" />
			<circle cx="8" cy="8" r="0.5" fill="currentColor" />
		</svg>
	{:else if kind === 'rockbox'}
		<!-- A DAP: screen and a wheel. -->
		<svg viewBox="0 0 16 16" width={size} height={size} fill="none" stroke="currentColor" stroke-width="1.3">
			<rect x="3.2" y="1.2" width="9.6" height="13.6" rx="1.8" />
			<rect x="5" y="3" width="6" height="4" rx="0.6" />
			<circle cx="8" cy="11" r="2" />
		</svg>
	{:else if kind === 'radio'}
		<!-- A radio: the body, the speaker and the aerial. -->
		<svg viewBox="0 0 16 16" width={size} height={size} fill="none" stroke="currentColor" stroke-width="1.3">
			<rect x="1.6" y="5.4" width="12.8" height="8.6" rx="1.8" />
			<path d="M4.4 5.4 11 1.8" />
			<circle cx="10.2" cy="9.7" r="2" />
			<line x1="4.2" y1="8.6" x2="6.4" y2="8.6" />
			<line x1="4.2" y1="10.8" x2="6.4" y2="10.8" />
		</svg>
	{:else if kind === 'push'}
		<!-- A phone: something submitted a listen to us. -->
		<svg viewBox="0 0 16 16" width={size} height={size} fill="none" stroke="currentColor" stroke-width="1.3">
			<rect x="4" y="1.2" width="8" height="13.6" rx="1.8" />
			<line x1="6.8" y1="12.4" x2="9.2" y2="12.4" />
		</svg>
	{:else if kind === 'spotify'}
		<!--
			A cloud, not the Spotify mark: the same rule as the library servers
			above, and the distinction worth seeing here is that this listen
			happened on somebody else's server rather than on the shelf or the
			LAN. The exact service is in the `title`.
		-->
		<svg viewBox="0 0 16 16" width={size} height={size} fill="none" stroke="currentColor"
			stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">
			<path d="M4.4 12.4a2.9 2.9 0 0 1-.3-5.8 4 4 0 0 1 7.7-1 3.2 3.2 0 0 1 .5 6.8z" />
			<path d="M6.3 8.6a4.6 4.6 0 0 1 3.8.6" />
		</svg>
	{:else if kind === 'import'}
		<!-- Into a tray: history that arrived as a file. -->
		<svg viewBox="0 0 16 16" width={size} height={size} fill="none" stroke="currentColor"
			stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">
			<path d="M8 1.8v7.4" /><path d="M5 6.4 8 9.4l3-3" />
			<path d="M2.4 10.6v2.2a1.4 1.4 0 0 0 1.4 1.4h8.4a1.4 1.4 0 0 0 1.4-1.4v-2.2" />
		</svg>
	{:else}
		<!-- Unknown: a plain note rather than a guess. -->
		<svg viewBox="0 0 16 16" width={size} height={size} fill="none" stroke="currentColor" stroke-width="1.3">
			<path d="M6 12.4V3.6l7-1.4v8.8" /><circle cx="4.4" cy="12.4" r="1.7" /><circle cx="11.4" cy="11" r="1.7" />
		</svg>
	{/if}
</span>

<style>
	.si {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		flex: none;
		color: var(--td-muted-3);
		vertical-align: -2px;
	}
</style>
