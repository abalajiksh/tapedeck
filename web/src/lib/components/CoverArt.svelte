<script lang="ts">
	import { coverArtUrl, coverStyle } from '$lib/utils';
	import { showArtwork } from '$lib/prefs';

	export let caaId: number | null | undefined = null;
	export let releaseMbid: string | null | undefined = null;
	/** Drives both the fallback gradient and its initial. */
	export let seed = '?';
	export let size = 40;
	export let radius = 9;

	// Cover Art Archive can 404 even when we hold an id (release replaced,
	// artwork removed). Falling back on error means a broken image never shows.
	let failed = false;
	$: url = $showArtwork && !failed ? coverArtUrl(caaId, releaseMbid, 250) : null;
	$: initial = (seed || '?').charAt(0).toUpperCase();
	// Re-try when the row is recycled onto a different track.
	$: if (caaId || releaseMbid) failed = false;
</script>

{#if url}
	<img
		src={url}
		alt=""
		loading="lazy"
		on:error={() => (failed = true)}
		style="width:{size}px;height:{size}px;flex:none;border-radius:{radius}px;object-fit:cover;background:var(--td-track);"
	/>
{:else}
	<div
		style="width:{size}px;height:{size}px;flex:none;border-radius:{radius}px;display:flex;align-items:center;justify-content:center;color:rgba(255,255,255,.92);font-family:'Caprasimo',serif;font-size:{Math.round(size * 0.42)}px;{coverStyle(seed)}"
	>{initial}</div>
{/if}
