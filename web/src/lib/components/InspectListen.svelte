<script lang="ts">
	import type { Scrobble } from '$lib/api';
	import { createEventDispatcher } from 'svelte';

	export let scrobble: Scrobble;

	const dispatch = createEventDispatcher<{ close: void }>();

	// Everything the row carries, including the fields the UI never renders
	// (source_id, delivered_sinks, session/device ids…). The list endpoint
	// already returns the whole stored row, so nothing here is a second guess
	// at what the database holds — it *is* what it holds.
	$: pretty = JSON.stringify(scrobble, null, 2);

	// MBIDs are the thing you actually want to click through on when a listen
	// looks mismatched.
	$: links = [
		['Recording', scrobble.mbid_recording, 'recording'],
		['Release', scrobble.mbid_release, 'release'],
		['Cover art release', scrobble.caa_release_mbid, 'release']
	].filter(([, id]) => !!id) as [string, string, string][];

	let copied = false;
	async function copy() {
		try {
			await navigator.clipboard.writeText(pretty);
			copied = true;
			setTimeout(() => (copied = false), 1800);
		} catch { /* clipboard blocked — the JSON is selectable anyway */ }
	}
</script>

<div class="dialog-backdrop" on:click={() => dispatch('close')} on:keydown={() => {}} role="presentation">
	<div class="dialog" style="width:min(680px,100%);" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
		<div class="dialog-title">Inspect listen</div>
		<div class="dialog-body">
			<p style="margin:0 0 12px;font-size:12.5px;color:var(--td-muted);">
				The stored row exactly as Tapedeck holds it — every column, including the ones no screen shows.
			</p>

			{#if links.length}
				<div style="display:flex;flex-wrap:wrap;gap:8px;margin-bottom:12px;">
					{#each links as [label, id, kind]}
						<a
							class="tag tag-outline"
							style="text-decoration:none;"
							href={`https://musicbrainz.org/${kind}/${id}`}
							target="_blank"
							rel="noopener"
						>{label} on MusicBrainz →</a>
					{/each}
				</div>
			{/if}

			<pre class="inspect-json">{pretty}</pre>
		</div>
		<div class="dialog-actions">
			<button class="btn btn-secondary" on:click={copy}>{copied ? '✓ Copied' : 'Copy JSON'}</button>
			<button class="btn btn-primary" on:click={() => dispatch('close')}>Close</button>
		</div>
	</div>
</div>

<style>
	.inspect-json {
		margin: 0;
		max-height: 46vh;
		overflow: auto;
		padding: 14px 16px;
		border-radius: 14px;
		background: var(--td-panel2);
		color: #a9c48a;
		font-size: 12.5px;
		line-height: 1.55;
		white-space: pre;
		tab-size: 2;
	}
</style>
