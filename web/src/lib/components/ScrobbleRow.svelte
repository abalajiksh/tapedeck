<script lang="ts">
	import { timeAgo, qualityLabel, coverArtUrl, formatDuration } from '$lib/utils';
	import type { Scrobble } from '$lib/api';

	export let scrobble: Scrobble;
	export let showTimestamp = true;

	$: quality = qualityLabel(scrobble);
	$: cover = coverArtUrl(scrobble.caa_id, scrobble.caa_release_mbid, 250);
	$: badgeClass =
		quality.tier === 'dsd' ? 'badge-dsd' :
		quality.tier === 'lossless' ? 'badge-lossless' :
		quality.tier === 'bt' ? 'badge-bt' :
		'badge-lossy';
</script>

<div class="group flex items-center gap-3 px-3 py-2.5 rounded-lg hover:bg-rp-hl-low transition-colors">
	<!-- Cover art -->
	<div class="w-10 h-10 rounded bg-rp-overlay shrink-0 overflow-hidden">
		{#if cover}
			<img src={cover} alt="" class="w-full h-full object-cover" loading="lazy" />
		{:else}
			<div class="w-full h-full flex items-center justify-center text-rp-muted text-lg">♫</div>
		{/if}
	</div>

	<!-- Track info -->
	<div class="flex-1 min-w-0">
		<p class="text-sm text-rp-text truncate font-medium">{scrobble.title}</p>
		<p class="text-xs text-rp-subtle truncate">
			{scrobble.artist}
			{#if scrobble.album}
				<span class="text-rp-muted"> — {scrobble.album}</span>
			{/if}
		</p>
	</div>

	<!-- Quality badge -->
	{#if quality.text}
		<span class={badgeClass}>{quality.text}</span>
	{/if}

	<!-- Duration -->
	{#if scrobble.duration}
		<span class="text-xs text-rp-muted w-12 text-right shrink-0">
			{formatDuration(scrobble.duration)}
		</span>
	{/if}

	<!-- Timestamp -->
	{#if showTimestamp}
		<span class="text-xs text-rp-muted w-16 text-right shrink-0">
			{timeAgo(scrobble.timestamp)}
		</span>
	{/if}
</div>
