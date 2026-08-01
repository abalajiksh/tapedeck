<script lang="ts">
	import type { Scrobble } from '$lib/api';
	import { qualityLabel, tierTagClass, formatDuration, timeAgo } from '$lib/utils';
	import CoverArt from './CoverArt.svelte';
	import { createEventDispatcher } from 'svelte';
	import { lovedKeys, loveKey, toggleLove } from '$lib/loves';

	export let scrobble: Scrobble;
	export let showMenu = false;
	export let selectable = false;
	export let selected = false;
	/** The heart is the point of the row for most people, but selection mode is
	 *  about bulk edits — a stray love there is a misclick, so it's hidden. */
	export let showLove = true;

	const dispatch = createEventDispatcher<{ menu: Scrobble; toggle: Scrobble }>();

	$: q = qualityLabel(scrobble);
	$: seed = scrobble.artist || scrobble.title || '?';
	$: time = new Date(scrobble.timestamp * 1000).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });

	// The bare heart means the *song*, matching Last.fm and ListenBrainz.
	// Album and artist stay in the row menu, where the distinction is explicit.
	$: trackKey = loveKey(scrobble, 'track');
	$: loved = trackKey ? $lovedKeys.has(trackKey) : false;
</script>

<div class="row" style="display:flex;align-items:center;gap:14px;padding:13px 18px;border-bottom:1px solid var(--td-border);{selected ? 'background:color-mix(in srgb, var(--td-accent) 8%, transparent);' : ''}">
	{#if selectable}
		<input type="checkbox" checked={selected} on:change={() => dispatch('toggle', scrobble)}
			aria-label="Select listen" style="width:18px;height:18px;flex:none;accent-color:var(--td-accent);cursor:pointer;">
	{/if}
	<CoverArt url={scrobble.artwork_url} caaId={scrobble.caa_id} releaseMbid={scrobble.caa_release_mbid} {seed} />
	<div style="flex:1;min-width:0;">
		<div style="font-size:14.5px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">{scrobble.title}</div>
		<div style="font-size:12.5px;color:var(--td-muted);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">{scrobble.artist}{scrobble.album ? ` — ${scrobble.album}` : ''}</div>
	</div>
	{#if showLove && !selectable}
		<button
			class="love"
			class:on={loved}
			on:click|stopPropagation={() => toggleLove(scrobble, 'track')}
			title={loved ? 'Loved — click to remove' : 'Love this track'}
			aria-label={loved ? 'Remove love' : 'Love this track'}
			aria-pressed={loved}
		>
			<svg viewBox="0 0 24 24" width="17" fill={loved ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1-1.1a5.5 5.5 0 0 0-7.8 7.8l1.1 1L12 21l7.7-7.6 1.1-1a5.5 5.5 0 0 0 0-7.8z"/>
			</svg>
		</button>
	{/if}
	{#if q.text}<span class={tierTagClass(q.tier)}>{q.text}</span>{/if}
	<span style="font-size:12px;color:var(--td-muted-3);width:54px;text-align:right;flex:none;">{formatDuration(scrobble.duration)}</span>
	<span style="font-size:12px;color:var(--td-muted-3);width:52px;text-align:right;flex:none;">{showMenu ? time : timeAgo(scrobble.timestamp)}</span>
	{#if showMenu && !selectable}
		<button on:click={() => dispatch('menu', scrobble)} aria-label="Listen actions" style="border:none;background:transparent;cursor:pointer;color:var(--td-muted-3);padding:5px;border-radius:8px;flex:none;display:flex;">
			<svg viewBox="0 0 24 24" width="18" fill="currentColor"><circle cx="12" cy="5" r="1.7"/><circle cx="12" cy="12" r="1.7"/><circle cx="12" cy="19" r="1.7"/></svg>
		</button>
	{/if}
</div>

<style>
	/* Present but quiet until you go looking for it; a filled heart stays
	   visible, because that one is information rather than an affordance. */
	.love {
		border: none;
		background: transparent;
		cursor: pointer;
		color: var(--td-muted-3);
		padding: 4px;
		border-radius: 8px;
		flex: none;
		display: flex;
		opacity: 0;
		transition: opacity .15s, color .15s;
	}
	.row:hover .love,
	.love:focus-visible,
	.love.on {
		opacity: 1;
	}
	.love:hover { color: #b2622d; }
	.love.on { color: #b2622d; }
	/* Touch has no hover, so hiding it would put the heart out of reach. */
	@media (hover: none) {
		.love { opacity: 1; }
	}
</style>
