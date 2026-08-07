<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type PlaylistTarget, type PlaylistPushTrack, type PlaylistPushResult } from '$lib/api';
	import { playlistTargets, suggestPlaylistName } from '$lib/playlists';

	/**
	 * Send a generated list of tracks to a media server as a playlist.
	 *
	 * Deliberately generic: Rediscovery's collections use it today and the
	 * genre-map playlists will use the same component, because the question
	 * ("which of your servers, and what should it be called?") does not change
	 * with what produced the list.
	 *
	 * Two things it will not do. It never appears when the user has no server
	 * connected — a button that can only ever fail is worse than an absent one,
	 * the same call the Rediscovery cards already made about Play and Queue. And
	 * it never claims more than happened: a push that lands nine of twelve says
	 * so and names the three, because a library is not the same set as a
	 * listening history and the gap is normal rather than an error.
	 */

	export let tracks: PlaylistPushTrack[] = [];
	/** What the list is called, used to seed the playlist name. */
	export let title = 'Playlist';
	/** The collection's accent, so the button belongs to the card it sits on. */
	export let accent = '';
	/** Smaller button, for a card footer rather than a page header. */
	export let compact = false;

	let targets: PlaylistTarget[] = [];
	let open = false;
	let chosen: number | null = null;
	let name = '';
	let pushing = false;
	let result: PlaylistPushResult | null = null;
	let error = '';

	const clean = (e: unknown) => String(e).replace(/^API \d+: /, '');

	function start() {
		result = null;
		error = '';
		name = suggestPlaylistName(title);
		chosen = targets[0]?.source_id ?? null;
		open = true;
	}

	function close() {
		open = false;
		pushing = false;
	}

	async function send() {
		if (chosen === null || !name.trim() || !tracks.length) return;
		pushing = true;
		error = '';
		try {
			result = await api.pushPlaylist(chosen, name.trim(), tracks);
		} catch (e) {
			error = clean(e);
		}
		pushing = false;
	}

	onMount(async () => {
		targets = await playlistTargets();
	});
</script>

{#if targets.length && tracks.length}
	<button
		class="btn btn-secondary"
		style={compact
			? `font-size:13px;padding:8px 15px;${accent ? `color:${accent};` : ''}`
			: accent ? `color:${accent};` : ''}
		on:click={start}
	>
		<!-- A tray with an arrow going out: this sends the list somewhere else,
		     rather than playing it here. Drawn in currentColor like every other
		     glyph in the app, so it takes the card's accent. -->
		<svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor"
			stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
			<path d="M8 10V1.8" />
			<path d="M5 4.6 8 1.6l3 3" />
			<path d="M2.5 9.5v3.2a1.3 1.3 0 0 0 1.3 1.3h8.4a1.3 1.3 0 0 0 1.3-1.3V9.5" />
		</svg>
		Send to…
	</button>
{/if}

{#if open}
	<div class="dialog-backdrop" on:click={close} on:keydown={() => {}} role="presentation">
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			{#if result}
				<div class="dialog-title">
					{result.matched === result.requested ? 'Sent' : 'Sent, partly'}
				</div>
				<div class="dialog-body" style="color:var(--td-nav-fg);">
					<p style="margin:0 0 10px;">
						<strong>{result.matched}</strong> of {result.requested}
						track{result.requested === 1 ? '' : 's'} landed on {result.target.label} as
						“{result.name}”.
					</p>
					{#if result.missing.length}
						<p style="margin:0 0 6px;font-size:13px;color:var(--td-muted);">
							Not in that library — these are in your history but not on this server, or
							filed under a spelling it doesn't share:
						</p>
						<ul class="missing">
							{#each result.missing as m}
								<li>{m.title} <span style="color:var(--td-muted-3);">— {m.artist}</span></li>
							{/each}
						</ul>
					{/if}
				</div>
				<div class="dialog-actions">
					<button class="btn btn-primary" on:click={close}>Done</button>
				</div>
			{:else}
				<div class="dialog-title">Send to a server</div>
				<div class="dialog-body" style="color:var(--td-nav-fg);">
					<p style="margin:0 0 14px;font-size:13px;color:var(--td-muted);">
						{tracks.length} track{tracks.length === 1 ? '' : 's'}. Tapedeck stores names
						rather than your server's ids, so each one is looked up by artist and title —
						anything it can't find is reported rather than quietly dropped.
					</p>

					<div class="field" style="margin-bottom:14px;">
						<label for="pl-name">Playlist name</label>
						<input id="pl-name" class="input" bind:value={name} />
					</div>

					<div class="field">
						<span class="lbl">Where to</span>
						<div class="targets">
							{#each targets as t (t.source_id)}
								<button
									class="target"
									class:on={chosen === t.source_id}
									on:click={() => (chosen = t.source_id)}
								>
									<span class="t-name">{t.label}</span>
									<span class="t-proto">
										{t.protocol}{t.account ? ` · ${t.account}` : ''}
									</span>
								</button>
							{/each}
						</div>
					</div>

					{#if error}
						<p style="margin:12px 0 0;font-size:13px;color:#b2622d;">{error}</p>
					{/if}
				</div>
				<div class="dialog-actions">
					<button class="btn btn-secondary" on:click={close}>Cancel</button>
					<button
						class="btn btn-primary"
						disabled={pushing || chosen === null || !name.trim()}
						on:click={send}
					>
						{pushing ? 'Sending…' : 'Send'}
					</button>
				</div>
			{/if}
		</div>
	</div>
{/if}

<style>
	/* `.field > label` is global; this block has a group of buttons rather than
	   one control, so it gets the same look without claiming a `for`. */
	.lbl {
		display: block;
		font-size: 12px;
		color: var(--td-muted-2);
		margin-bottom: 6px;
	}

	.targets { display: flex; flex-direction: column; gap: 6px; }

	.target {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 1px;
		width: 100%;
		text-align: left;
		font: inherit;
		cursor: pointer;
		padding: 9px 14px;
		border-radius: 14px;
		border: 1px solid var(--td-border);
		background: transparent;
		color: var(--td-text);
	}
	.target:hover { background: var(--td-nav-hover); }
	.target.on { border-color: var(--td-accent); background: var(--td-surface2); }

	.t-name { font-size: 13.5px; font-weight: 600; }
	.t-proto { font-size: 11.5px; color: var(--td-muted-3); }

	.missing {
		margin: 0;
		padding-left: 18px;
		font-size: 13px;
		/* A long tail scrolls inside the dialog rather than pushing the buttons
		   off the bottom of the screen. */
		max-height: 180px;
		overflow-y: auto;
	}
	.missing li { margin-bottom: 2px; }
</style>
