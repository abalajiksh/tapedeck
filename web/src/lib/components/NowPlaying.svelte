<script lang="ts">
	import { qualityLabel, coverArtUrl } from '$lib/utils';
	import type { Scrobble } from '$lib/api';

	export let scrobble: Scrobble | null = null;

	$: quality = scrobble ? qualityLabel(scrobble) : null;
	$: cover = scrobble ? coverArtUrl(scrobble.caa_id, scrobble.caa_release_mbid, 500) : null;
	$: badgeClass = quality
		? quality.tier === 'dsd' ? 'badge-dsd'
		: quality.tier === 'lossless' ? 'badge-lossless'
		: quality.tier === 'bt' ? 'badge-bt'
		: 'badge-lossy'
		: '';
</script>

{#if scrobble}
	<div class="relative overflow-hidden rounded-xl bg-rp-surface border border-rp-hl-med">
		<!-- Background blur from cover art -->
		{#if cover}
			<div
				class="absolute inset-0 opacity-15 blur-3xl scale-150"
				style="background-image: url({cover}); background-size: cover; background-position: center;"
			></div>
		{/if}

		<div class="relative flex items-center gap-5 p-5">
			<!-- Cover -->
			<div class="w-20 h-20 rounded-lg bg-rp-overlay shrink-0 overflow-hidden shadow-lg">
				{#if cover}
					<img src={cover} alt="" class="w-full h-full object-cover" />
				{:else}
					<div class="w-full h-full flex items-center justify-center text-rp-muted text-3xl">♫</div>
				{/if}
			</div>

			<!-- Info -->
			<div class="flex-1 min-w-0">
				<div class="flex items-center gap-2 mb-1">
					<span class="inline-block w-2 h-2 rounded-full bg-rp-love animate-pulse"></span>
					<span class="text-[11px] uppercase tracking-wider text-rp-love font-medium">Now Playing</span>
				</div>
				<p class="text-lg font-medium text-rp-text truncate">{scrobble.title}</p>
				<p class="text-sm text-rp-subtle truncate">
					{scrobble.artist}
					{#if scrobble.album}
						<span class="text-rp-muted"> — {scrobble.album}</span>
					{/if}
				</p>
			</div>

			<!-- Quality + context -->
			<div class="flex flex-col items-end gap-1.5 shrink-0">
				{#if quality?.text}
					<span class={badgeClass}>{quality.text}</span>
				{/if}
				{#if scrobble.listening_context && scrobble.listening_context !== 'unknown'}
					<span class="text-[10px] text-rp-muted uppercase tracking-wider">
						{scrobble.listening_context}
					</span>
				{/if}
			</div>
		</div>
	</div>
{:else}
	<div class="rounded-xl bg-rp-surface border border-rp-hl-med p-5">
		<div class="flex items-center gap-2 text-rp-muted">
			<span class="inline-block w-2 h-2 rounded-full bg-rp-hl-high"></span>
			<span class="text-sm">Nothing playing</span>
		</div>
	</div>
{/if}
