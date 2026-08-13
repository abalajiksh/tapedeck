<script lang="ts">
	// Playlists that live in Tapedeck.
	//
	// This is the review step the MCP design turns on: a list an assistant built
	// is saved here first and reaches a media server only when the person whose
	// server it is says so. It is also just a useful place to keep a list —
	// Playlist Lab next door generates them and had nowhere to put one.
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { SavedPlaylist, SavedTrack, PlaylistTarget } from '$lib/api';
	import { formatDateTime } from '$lib/utils';

	let playlists: SavedPlaylist[] = [];
	let targets: PlaylistTarget[] = [];
	let openId: number | null = null;
	let tracks: SavedTrack[] = [];
	let loading = true;
	let busy = false;
	let result = '';

	onMount(async () => {
		await load();
		try {
			// An empty list is a normal answer — plenty of instances have no
			// writable source at all — and the page says so rather than showing
			// a button that cannot work.
			targets = (await api.playlistTargets()).targets;
		} catch { /* no targets is a normal state */ }
		loading = false;
	});

	async function load() {
		try {
			playlists = (await api.getSavedPlaylists()).playlists;
		} catch (e) {
			console.error(e);
		}
	}

	async function open(p: SavedPlaylist) {
		if (openId === p.id) {
			openId = null;
			return;
		}
		openId = p.id;
		tracks = [];
		result = '';
		try {
			tracks = (await api.getSavedPlaylist(p.id)).tracks;
		} catch (e) {
			console.error(e);
		}
	}

	async function push(p: SavedPlaylist, sourceId: number) {
		busy = true;
		result = '';
		try {
			const r = await api.pushSavedPlaylist(p.id, sourceId);
			// A partial match is the normal outcome, so it is reported as one.
			// Saying "sent" when three tracks did not land is the failure this
			// codebase keeps finding in other forms.
			result =
				r.matched === r.requested
					? `Sent all ${r.matched} tracks to ${r.protocol}.`
					: `Sent ${r.matched} of ${r.requested} to ${r.protocol}. Not found: ` +
						r.missing.map((m) => `${m.artist} — ${m.title}`).join(', ');
			await load();
		} catch (e) {
			result = e instanceof Error ? e.message : 'Could not send the playlist.';
		}
		busy = false;
	}

	async function remove(p: SavedPlaylist) {
		if (!confirm(`Delete “${p.name}”?`)) return;
		try {
			await api.deleteSavedPlaylist(p.id);
			if (openId === p.id) openId = null;
			await load();
		} catch (e) {
			console.error(e);
		}
	}
</script>

<svelte:head><title>Playlists · Tapedeck</title></svelte:head>

<div class="head">
	<div>
		<h1 class="h-display">Playlists</h1>
		<p class="sub">Lists kept in Tapedeck — yours, and any an assistant has handed back.</p>
	</div>
	<a class="btn btn-secondary" href="/playlists">Playlist Lab →</a>
</div>

{#if loading}
	<div class="spin"></div>
{:else if !playlists.length}
	<div class="panel empty">
		<p>No playlists yet.</p>
		<p class="dim">
			Build one in <a href="/playlists">Playlist Lab</a>, or connect an assistant in
			<a href="/settings">Settings → AI Connections</a> and ask it for one.
		</p>
	</div>
{:else}
	<div class="list">
		{#each playlists as p (p.id)}
			<div class="row" class:open={openId === p.id}>
				<button class="rowmain" on:click={() => open(p)} aria-label="Open {p.name}">
					<div class="title">
						{p.name}
						{#if p.origin !== 'user'}
							<!-- Provenance, the same rule notes follow: a list a chatbot
							     wrote stays distinguishable from one you built. -->
							<span class="badge">by {p.origin}</span>
						{/if}
					</div>
					{#if p.description}<div class="desc">{p.description}</div>{/if}
					<div class="meta">
						{p.tracks} track{p.tracks === 1 ? '' : 's'} · {formatDateTime(p.updated_at)}
						{#if p.pushed_at}· sent to {p.pushed_to} {formatDateTime(p.pushed_at)}{/if}
					</div>
				</button>
				<button class="del" on:click={() => remove(p)} aria-label="Delete {p.name}">Delete</button>
			</div>

			{#if openId === p.id}
				<div class="detail panel">
					{#if tracks.length}
						<ol class="tracks">
							{#each tracks as t}
								<li>
									<span class="tt">{t.title}</span>
									<span class="ta">{t.artist}{t.album ? ` · ${t.album}` : ''}</span>
									{#if t.note}<span class="tn">{t.note}</span>{/if}
								</li>
							{/each}
						</ol>
					{:else}
						<p class="dim">Loading tracks…</p>
					{/if}

					<div class="actions">
						{#if targets.length}
							{#each targets as t}
								<button class="btn btn-primary" disabled={busy} on:click={() => push(p, t.source_id)}>
									Send to {t.label}
								</button>
							{/each}
						{:else}
							<p class="dim">
								No media server is connected, so there is nowhere to send this yet.
								Add one in <a href="/settings">Settings → Sources</a>.
							</p>
						{/if}
					</div>

					{#if result}<p class="result">{result}</p>{/if}
				</div>
			{/if}
		{/each}
	</div>
{/if}

<style>
	.head {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 16px;
		flex-wrap: wrap;
		margin-bottom: 20px;
	}
	h1 {
		font-size: 28px;
		margin: 0 0 4px;
	}
	.sub {
		font-size: 14px;
		color: var(--td-muted);
		margin: 0;
	}
	.list {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.row {
		display: flex;
		align-items: stretch;
		gap: 8px;
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
		border-radius: 14px;
		padding: 4px;
	}
	.row.open {
		border-color: var(--td-accent);
	}
	.rowmain {
		flex: 1;
		min-width: 0;
		text-align: left;
		background: none;
		border: 0;
		padding: 12px;
		cursor: pointer;
		color: inherit;
	}
	.title {
		font-weight: 600;
		font-size: 15px;
	}
	.badge {
		font-size: 10.5px;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--td-muted-3);
		margin-left: 8px;
	}
	.desc {
		font-size: 13px;
		color: var(--td-muted);
		margin-top: 3px;
	}
	.meta {
		font-size: 12px;
		color: var(--td-muted-2);
		margin-top: 5px;
	}
	.del {
		background: none;
		border: 0;
		color: #b2622d;
		font-size: 12.5px;
		padding: 0 14px;
		cursor: pointer;
	}
	.detail {
		border-radius: 16px;
		padding: 18px;
		margin: -2px 0 6px;
	}
	.tracks {
		margin: 0 0 16px;
		padding-left: 22px;
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.tt {
		font-size: 13.5px;
		display: block;
	}
	.ta {
		font-size: 12px;
		color: var(--td-muted-2);
		display: block;
	}
	.tn {
		font-size: 12px;
		color: var(--td-muted);
		font-style: italic;
		display: block;
		margin-top: 2px;
	}
	.actions {
		display: flex;
		gap: 8px;
		flex-wrap: wrap;
	}
	.result {
		font-size: 12.5px;
		color: var(--td-muted);
		margin: 12px 0 0;
	}
	.empty {
		border-radius: 18px;
		padding: 28px;
		text-align: center;
	}
	.dim {
		font-size: 13px;
		color: var(--td-muted-2);
	}
	.spin {
		width: 22px;
		height: 22px;
		border: 2px solid var(--td-track);
		border-top-color: var(--td-accent);
		border-radius: 50%;
		margin: 40px auto;
		animation: spin 0.8s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.spin {
			animation: none;
		}
	}
</style>
