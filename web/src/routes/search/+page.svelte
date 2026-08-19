<script lang="ts">
	import { onMount } from 'svelte';
	import { syncUrl } from '$lib/url';
	import { api } from '$lib/api';
	import type { SearchResults } from '$lib/api';
	import { openAlbum, openArtist } from '$lib/entities';

	let q = '';
	let results: SearchResults | null = null;
	let searching = false;
	let searched = false;
	let input: HTMLInputElement;

	$: empty =
		results && !results.artists.length && !results.albums.length && !results.tracks.length;

	let timer: ReturnType<typeof setTimeout>;
	function onInput() {
		clearTimeout(timer);
		// Long enough that typing a word doesn't fire five queries, short enough
		// that it still feels like it's keeping up.
		timer = setTimeout(run, 250);
	}

	async function run() {
		const term = q.trim();
		if (!term) {
			results = null;
			searched = false;
			syncUrl('/search');
			return;
		}
		searching = true;
		try {
			results = await api.search(term);
			searched = true;
			// Keep the query in the URL so a search is linkable and survives a
			// reload — the same reason history carries its scope there.
			syncUrl(`/search?q=${encodeURIComponent(term)}`);
		} catch (e) {
			console.warn('search', e);
		}
		searching = false;
	}

	/** Same rule everywhere a track is listed: there is no track page, so it
	 *  opens the record it's on, and only falls back to the raw listens when
	 *  there's no album to open. */
	function openTrack(t: { title: string; artist: string; album: string | null }) {
		if (t.album) return openAlbum(t.artist, t.album);
		window.location.href = `/history?track=${encodeURIComponent(t.title)}`;
	}

	const plays = (n: number) => `${n.toLocaleString()} ${n === 1 ? 'play' : 'plays'}`;

	onMount(() => {
		q = new URLSearchParams(window.location.search).get('q') ?? '';
		if (q) run();
		input?.focus();
	});
</script>

<svelte:head><title>Search — Tapedeck</title></svelte:head>

<div style="margin-bottom:24px;">
	<h1 class="h-display" style="font-size:38px;">Search</h1>
	<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;">Everything you've played, by name.</p>
</div>

<div style="position:relative;margin-bottom:26px;">
	<input
		bind:this={input}
		class="input"
		style="width:100%;padding-left:42px;height:48px;font-size:16px;"
		bind:value={q}
		on:input={onInput}
		placeholder="An artist, an album, a song…"
		aria-label="Search your listening history"
	>
	<svg viewBox="0 0 24 24" width="18" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"
		style="position:absolute;left:15px;top:50%;transform:translateY(-50%);color:var(--td-muted-3);pointer-events:none;">
		<circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/>
	</svg>
</div>

{#if searching && !results}
	<div style="display:flex;justify-content:center;padding:50px;"><div class="spin" style="width:22px;height:22px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
{:else if empty}
	<div class="panel" style="border-radius:22px;padding:44px;text-align:center;color:var(--td-muted);">
		Nothing in your history matches “{q}”.
	</div>
{:else if results}
	{#if results.artists.length}
		<h3 class="h-display sec">Artists</h3>
		<div class="panel list">
			{#each results.artists as a}
				<button class="hit" on:click={() => openArtist(a.name)}>
					<span class="name">{a.name}</span>
					<span class="count">{plays(a.plays)}</span>
				</button>
			{/each}
		</div>
	{/if}

	{#if results.albums.length}
		<h3 class="h-display sec">Albums</h3>
		<div class="panel list">
			{#each results.albums as al}
				<button class="hit" on:click={() => openAlbum(al.artist, al.name)}>
					<span class="name">{al.name}<span class="sub">&nbsp;— {al.artist}</span></span>
					<span class="count">{plays(al.plays)}</span>
				</button>
			{/each}
		</div>
	{/if}

	{#if results.tracks.length}
		<h3 class="h-display sec">Tracks</h3>
		<div class="panel list">
			{#each results.tracks as t}
				<button class="hit" on:click={() => openTrack(t)}>
					<span class="name">{t.title}<span class="sub">&nbsp;— {t.artist}{t.album ? ` · ${t.album}` : ''}</span></span>
					<span class="count">{plays(t.plays)}</span>
				</button>
			{/each}
		</div>
	{/if}
{:else if !searched}
	<div class="panel" style="border-radius:22px;padding:44px;text-align:center;color:var(--td-muted);">
		Searches your own listening history — not a catalogue. If you've played it, it's here.
	</div>
{/if}

<style>
	.spin { animation: td-spin 0.7s linear infinite; }
	.sec {
		font-size: 19px;
		margin: 26px 0 10px;
	}
	.list {
		border-radius: 22px;
		overflow: hidden;
	}
	/* Buttons rather than anchors: an entity id has to be resolved (and possibly
	   created) before there is a URL to go to. */
	.hit {
		width: 100%;
		border: none;
		background: transparent;
		font: inherit;
		cursor: pointer;
		display: flex;
		align-items: center;
		gap: 14px;
		padding: 13px 18px;
		border-bottom: 1px solid var(--td-border);
		text-decoration: none;
		color: inherit;
	}
	.hit:last-child { border-bottom: none; }
	.hit:hover { background: var(--td-nav-hover); }
	.name {
		flex: 1;
		min-width: 0;
		font-size: 14.5px;
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.sub {
		font-weight: 400;
		color: var(--td-muted);
	}
	.count {
		flex: none;
		font-size: 12.5px;
		color: var(--td-muted-3);
	}
</style>
