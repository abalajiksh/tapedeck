<script lang="ts">
	/**
	 * Anniversaries among what you listen to — composers' and artists' births
	 * and deaths, bands forming and splitting, albums turning 30 — round
	 * numbers only, and only artists and albums in your own history.
	 *
	 * One component, three shapes: the Dashboard card (`card`), Rediscovery's
	 * "turning 30 this year" albums you have not played for a while
	 * (`revisit`), and a report year's line (`year`). Renders nothing when
	 * there is nothing to mark.
	 */
	import { onMount } from 'svelte';
	import { api, type Anniversary } from '$lib/api';
	import { openAlbum, openArtist } from '$lib/entities';

	export let mode: 'card' | 'revisit' | 'year' = 'card';
	export let year: number | null = null;
	export let limit = 6;

	let items: Anniversary[] = [];
	let today: Anniversary[] = [];
	let shownYear = 0;

	async function load(y: number | null) {
		try {
			const a = await api.getAnniversaries(y ?? undefined);
			shownYear = a.year;
			today = mode === 'card' ? a.today : [];
			items = (mode === 'revisit' ? a.revisit : a.items).slice(0, limit);
		} catch {
			items = [];
			today = [];
		}
	}
	onMount(() => load(year));

	function open(a: Anniversary) {
		if (a.kind === 'released' && a.artist) return openAlbum(a.artist, a.name);
		return openArtist(a.name);
	}
	const title = { card: 'Anniversaries this year', revisit: 'Turning round numbers this year', year: 'Anniversaries' };
</script>

{#if items.length || today.length}
	<section class="panel ann" class:inline={mode === 'year'}>
		<div class="head">
			<h3 class="h-display">{mode === 'year' ? `${title.year} in ${shownYear}` : title[mode]}</h3>
			{#if mode === 'revisit'}<p>Albums you loved that have a birthday this year and haven't been on lately.</p>{/if}
		</div>
		{#each today as a}
			<button class="row today" on:click={() => open(a)}><span class="dot" aria-hidden="true"></span>{a.label}</button>
		{/each}
		{#each items as a}
			<button class="row" on:click={() => open(a)}>
				<span class="n h-display">{a.years}</span>
				<span class="label">{a.label}{#if a.artist} <span class="by">· {a.artist}</span>{/if}</span>
				<span class="plays">{a.plays.toLocaleString()} plays</span>
			</button>
		{/each}
	</section>
{/if}

<style>
	.ann { border-radius: 22px; padding: 18px 20px; margin-bottom: 22px; }
	.ann.inline { margin-top: 16px; }
	.head h3 { font-size: 19px; }
	.head p { margin: 4px 0 0; font-size: 12.5px; color: var(--td-muted); }
	.head { margin-bottom: 8px; }
	.row {
		display: flex; align-items: baseline; gap: 12px; width: 100%; text-align: left;
		border: none; border-top: 1px solid var(--td-border); background: transparent; color: inherit;
		font: inherit; font-size: 13.5px; padding: 7px 0; cursor: pointer;
	}
	.row:hover .label { color: var(--td-accent); }
	.row.today { font-weight: 600; }
	.dot { width: 8px; height: 8px; border-radius: 50%; background: var(--td-accent); flex: none; align-self: center; }
	.n { width: 38px; flex: none; font-size: 18px; color: var(--td-accent); font-variant-numeric: tabular-nums; }
	.label { flex: 1; min-width: 0; }
	.by { color: var(--td-muted); }
	.plays { color: var(--td-muted-2); font-size: 12px; white-space: nowrap; font-variant-numeric: tabular-nums; }
</style>
