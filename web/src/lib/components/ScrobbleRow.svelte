<script lang="ts">
	import type { Scrobble } from '$lib/api';
	import { qualityLabel, tierTagClass, coverStyle, formatDuration, timeAgo } from '$lib/utils';
	import { createEventDispatcher } from 'svelte';

	export let scrobble: Scrobble;
	export let showMenu = false;
	export let selectable = false;
	export let selected = false;

	const dispatch = createEventDispatcher<{ menu: Scrobble; toggle: Scrobble }>();

	$: q = qualityLabel(scrobble);
	$: seed = scrobble.artist || scrobble.title || '?';
	$: initial = (scrobble.artist || scrobble.title || '?').charAt(0).toUpperCase();
	$: time = new Date(scrobble.timestamp * 1000).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
</script>

<div style="display:flex;align-items:center;gap:14px;padding:13px 18px;border-bottom:1px solid var(--td-border);{selected ? 'background:color-mix(in srgb, var(--td-accent) 8%, transparent);' : ''}">
	{#if selectable}
		<input type="checkbox" checked={selected} on:change={() => dispatch('toggle', scrobble)}
			aria-label="Select listen" style="width:18px;height:18px;flex:none;accent-color:var(--td-accent);cursor:pointer;">
	{/if}
	<div style="width:40px;height:40px;flex:none;border-radius:9px;display:flex;align-items:center;justify-content:center;color:rgba(255,255,255,.92);font-family:'Caprasimo',serif;font-size:17px;{coverStyle(seed)}">{initial}</div>
	<div style="flex:1;min-width:0;">
		<div style="font-size:14.5px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">{scrobble.title}</div>
		<div style="font-size:12.5px;color:var(--td-muted);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">{scrobble.artist}{scrobble.album ? ` — ${scrobble.album}` : ''}</div>
	</div>
	{#if q.text}<span class={tierTagClass(q.tier)}>{q.text}</span>{/if}
	<span style="font-size:12px;color:var(--td-muted-3);width:54px;text-align:right;flex:none;">{formatDuration(scrobble.duration)}</span>
	<span style="font-size:12px;color:var(--td-muted-3);width:52px;text-align:right;flex:none;">{showMenu ? time : timeAgo(scrobble.timestamp)}</span>
	{#if showMenu && !selectable}
		<button on:click={() => dispatch('menu', scrobble)} aria-label="Listen actions" style="border:none;background:transparent;cursor:pointer;color:var(--td-muted-3);padding:5px;border-radius:8px;flex:none;display:flex;">
			<svg viewBox="0 0 24 24" width="18" fill="currentColor"><circle cx="12" cy="5" r="1.7"/><circle cx="12" cy="12" r="1.7"/><circle cx="12" cy="19" r="1.7"/></svg>
		</button>
	{/if}
</div>
