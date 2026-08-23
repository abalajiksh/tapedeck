<script lang="ts">
	import type { Scrobble } from '$lib/api';
	import { qualityLabel, tierTagClass, mediumLabel, formatDuration, timeAgo } from '$lib/utils';
	import CoverArt from './CoverArt.svelte';
	import SourceIcon from './SourceIcon.svelte';
	import ChainIcon from './ChainIcon.svelte';
	import { createEventDispatcher } from 'svelte';
	import { lovedKeys, isLoved, toggleLove } from '$lib/loves';
	import { openAlbum, openArtist } from '$lib/entities';

	export let scrobble: Scrobble;
	export let showMenu = false;
	export let selectable = false;
	export let selected = false;
	/** The heart is the point of the row for most people, but selection mode is
	 *  about bulk edits — a stray love there is a misclick, so it's hidden. */
	export let showLove = true;
	/** Chains by id, so a row can show which setup it came out of. Optional —
	 *  a caller that doesn't have them simply shows no chain glyph. */
	export let chains: Record<number, { name: string; icon?: string | null }> = {};
	$: chainFor = scrobble.chain_id != null ? chains[scrobble.chain_id] : undefined;
	/** Off in selection mode, where a click is meant to select the row. */
	$: linkable = !selectable;

	const dispatch = createEventDispatcher<{ menu: Scrobble; toggle: Scrobble }>();

	$: q = qualityLabel(scrobble);
	// A side of vinyl has no codec, so the fidelity badge is empty for one and the
	// slot sat blank — a record and an unlabelled import looked the same. The two
	// never both apply: a listen either came off an object or out of a file.
	$: medium = mediumLabel(scrobble);
	$: seed = scrobble.artist || scrobble.title || '?';
	$: time = new Date(scrobble.timestamp * 1000).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });

	// The bare heart means the *song*, matching Last.fm and ListenBrainz.
	// Album and artist stay in the row menu, where the distinction is explicit.
	// Any form the row can be recognised by — the same song from another player
	// spells its artist differently, so an exact match alone left the heart empty.
	$: loved = isLoved($lovedKeys, scrobble, 'track');
</script>

<div class="row" style="border-bottom:1px solid var(--td-border);{selected ? 'background:color-mix(in srgb, var(--td-accent) 8%, transparent);' : ''}">
	{#if selectable}
		<input type="checkbox" checked={selected} on:change={() => dispatch('toggle', scrobble)}
			aria-label="Select listen" style="width:18px;height:18px;flex:none;accent-color:var(--td-accent);cursor:pointer;">
	{/if}
	<CoverArt url={scrobble.artwork_url} caaId={scrobble.caa_id} releaseMbid={scrobble.caa_release_mbid} {seed} />
	<div style="flex:1;min-width:0;">
		<div style="font-size:14.5px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">{scrobble.title}</div>
		<div class="subline">
			<SourceIcon source={scrobble.source_name} client={scrobble.submission_client} />
			{#if chainFor}<ChainIcon name={chainFor.icon} title={chainFor.name} />{/if}
			<span class="names">
				{#if linkable}
					<!-- Buttons rather than anchors: the row knows names, and an entity
					     id has to be resolved (and possibly created) before there is a
					     URL to go to. -->
					<button class="xref" on:click|stopPropagation={() => openArtist(scrobble.artist)}>{scrobble.artist}</button>
					{#if scrobble.album}
						<span aria-hidden="true"> — </span>
						<button class="xref" on:click|stopPropagation={() => scrobble.album && openAlbum(scrobble.artist, scrobble.album)}>{scrobble.album}</button>
					{/if}
				{:else}
					{scrobble.artist}{scrobble.album ? ` — ${scrobble.album}` : ''}
				{/if}
			</span>
			<!-- The same badge as the one in the row, shown only once: narrow
			     viewports move it down here so the title keeps the whole first
			     line. Rendering it twice and letting CSS choose is what avoids a
			     resize listener for something that is purely a layout question. -->
			{#if medium}<span class="tag tag-medium qtag-inline">{medium}</span>
			{:else if q.text}<span class="{tierTagClass(q.tier)} qtag-inline">{q.text}</span>{/if}
			<!--
				Who it was heard with. On the subline beside the source and chain
				icons, never on the title line — that line was only just rescued
				from being crushed on mobile.
			-->
			{#if scrobble.company}
				<span
					class="withwho"
					title={scrobble.joint_session_id
						? `Heard in a Shared Spool with ${scrobble.company}`
						: `Heard with ${scrobble.company}`}
				>
					<svg viewBox="0 0 24 24" width="11" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" aria-hidden="true">
						<circle cx="8" cy="12" r="4.4" /><circle cx="16" cy="12" r="4.4" />
					</svg>
					{scrobble.company}
				</span>
			{/if}
		</div>
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
	{#if medium}<span class="tag tag-medium qtag-row">{medium}</span>
	{:else if q.text}<span class="{tierTagClass(q.tier)} qtag-row">{q.text}</span>{/if}
	<span class="dur">{formatDuration(scrobble.duration)}</span>
	<span class="when">{showMenu ? time : timeAgo(scrobble.timestamp)}</span>
	{#if showMenu && !selectable}
		<button on:click={() => dispatch('menu', scrobble)} aria-label="Listen actions" style="border:none;background:transparent;cursor:pointer;color:var(--td-muted-3);padding:5px;border-radius:8px;flex:none;display:flex;">
			<svg viewBox="0 0 24 24" width="18" fill="currentColor"><circle cx="12" cy="5" r="1.7"/><circle cx="12" cy="12" r="1.7"/><circle cx="12" cy="19" r="1.7"/></svg>
		</button>
	{/if}
</div>

<style>
	/* Deliberately quiet: it is context, not a rating. The same accent the
	   spool glyph uses elsewhere, at the subline's own size. */
	.withwho {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		flex: none;
		max-width: 22ch;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
		color: var(--td-accent);
		font-size: 11.5px;
	}
	.withwho svg { flex: none; }

	.row {
		display: flex;
		align-items: center;
		gap: 14px;
		padding: 13px 18px;
	}
	.subline {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 12.5px;
		color: var(--td-muted);
		min-width: 0;
	}
	.names {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		min-width: 0;
	}
	.dur, .when {
		font-size: 12px;
		color: var(--td-muted-3);
		text-align: right;
		flex: none;
	}
	.dur { width: 54px; }
	.when { width: 52px; }

	/* One badge is shown at a time — the row's on wide viewports, the subline's
	   on narrow ones. */
	.qtag-inline { display: none; }
	.qtag-row { flex: none; }

	/* The row has seven fixed-width columns, and `.tag` is `white-space: nowrap`,
	   so the badge never gave any width back. Everything else is `flex:none`,
	   which left the title — the only `flex:1` item — absorbing the entire
	   shortfall: on a 360px viewport the fixed columns already overflow the row,
	   so the title was squeezed to a character or two. Below this width the
	   title takes the whole first line and the badge drops to the subline. */
	@media (max-width: 640px) {
		.row { gap: 10px; padding: 12px 14px; }
		.qtag-row { display: none; }
		.qtag-inline { display: inline-flex; flex: none; }
		/* Running time is the one column genuinely available elsewhere (the
		   album page lists it); the timestamp is what makes a history row mean
		   anything, so that stays. */
		.dur { display: none; }
		.when { width: 44px; }
	}

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
	/* Reads as the secondary line it replaces until you point at it — the row
	   shouldn't look like two links stapled under every title. */
	.xref {
		border: none;
		background: transparent;
		padding: 0;
		cursor: pointer;
		font: inherit;
		color: inherit;
	}
	.xref:hover { color: var(--td-accent); text-decoration: underline; }
</style>
