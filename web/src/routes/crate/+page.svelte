<script lang="ts">
	// The Crate — things somebody told you about that you don't own yet.
	//
	// The Shelf is what you took home; the Crate is what you're still flipping
	// through. "Got it" moves one to the other.
	//
	// This is the one page in Tapedeck that names music outside the collection,
	// and it is deliberately a place you go rather than something that follows
	// you around — the vision rule is that Tapedeck never *points* at music you
	// don't own, and recording what somebody else pointed at is a different act.
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { CrateEntry } from '$lib/api';
	import { formatDateTime } from '$lib/utils';

	let entries: CrateEntry[] = [];
	let landed = 0;
	let loading = true;
	let filter: 'open' | 'landed' | 'all' = 'open';

	onMount(async () => {
		await load();
		loading = false;
	});

	async function load() {
		try {
			const r = await api.getCrate();
			entries = r.crate;
			landed = r.landed;
		} catch (e) {
			console.error(e);
		}
	}

	$: shown = entries.filter((e) =>
		filter === 'all' ? true : filter === 'landed' ? e.first_played_at : e.status === 'open'
	);

	// Who suggested what, and how often it stuck. A recommendation is a
	// prediction, and this is the only place it can be scored.
	$: byOrigin = Object.entries(
		entries.reduce<Record<string, { n: number; hit: number }>>((acc, e) => {
			const k = e.origin;
			acc[k] ??= { n: 0, hit: 0 };
			acc[k].n++;
			if (e.first_played_at) acc[k].hit++;
			return acc;
		}, {})
	).sort((a, b) => b[1].n - a[1].n);

	async function mark(e: CrateEntry, status: 'acquired' | 'dismissed' | 'open') {
		try {
			await api.setCrateStatus(e.id, status);
			await load();
		} catch (err) {
			console.error(err);
		}
	}

	async function remove(e: CrateEntry) {
		if (!confirm(`Remove ${label(e)} from the crate?`)) return;
		try {
			await api.deleteCrateEntry(e.id);
			await load();
		} catch (err) {
			console.error(err);
		}
	}

	function label(e: CrateEntry) {
		return e.name ? `${e.artist} — ${e.name}` : e.artist;
	}

	// Identification, not shopping. MusicBrainz for the canonical record,
	// Discogs for the pressing — both are catalogue lookups, and neither is
	// linked to a marketplace listing or a price.
	function musicbrainz(e: CrateEntry) {
		if (e.mbid) return `https://musicbrainz.org/${e.kind === 'artist' ? 'artist' : 'release'}/${e.mbid}`;
		const q = encodeURIComponent(e.name ? `${e.artist} ${e.name}` : e.artist);
		return `https://musicbrainz.org/search?query=${q}&type=${e.kind === 'artist' ? 'artist' : 'release'}`;
	}
	function discogs(e: CrateEntry) {
		const q = encodeURIComponent(e.name ? `${e.artist} ${e.name}` : e.artist);
		return `https://www.discogs.com/search/?q=${q}&type=${e.kind === 'artist' ? 'artist' : 'release'}`;
	}
</script>

<svelte:head><title>The Crate · Tapedeck</title></svelte:head>

<div class="head">
	<div>
		<h1 class="h-display">The Crate</h1>
		<p class="sub">
			Records and artists somebody put in front of you. Nothing here is yours yet —
			<a href="/shelf">The Shelf</a> is what you took home.
		</p>
	</div>
</div>

{#if loading}
	<div class="spin"></div>
{:else if !entries.length}
	<div class="panel empty">
		<p>The crate is empty.</p>
		<p class="dim">
			Connect an assistant in <a href="/settings">Settings → AI Connections</a> with the
			crate grant, and anything it suggests lands here for you to look at.
		</p>
	</div>
{:else}
	{#if byOrigin.length}
		<div class="panel scores">
			<div class="eyebrow">Who was right</div>
			<div class="rows">
				{#each byOrigin as [origin, s]}
					<div class="score">
						<span class="who">{origin === 'user' ? 'You' : origin}</span>
						<span class="num">{s.hit} of {s.n}</span>
						<span class="dim">played</span>
					</div>
				{/each}
			</div>
			<p class="dim foot">
				A suggestion is a prediction. Tapedeck knows what you actually went on to play,
				so it can tell you whose taste is worth listening to.
			</p>
		</div>
	{/if}

	<div class="tabs">
		{#each [['open', 'Open'], ['landed', `Played (${landed})`], ['all', 'Everything']] as [k, lbl]}
			<button class="seg-btn" class:on={filter === k} on:click={() => (filter = k as typeof filter)}>
				{lbl}
			</button>
		{/each}
	</div>

	<div class="list">
		{#each shown as e (e.id)}
			<div class="card" class:played={e.first_played_at}>
				<div class="top">
					<div>
						<div class="title">{label(e)}</div>
						<div class="meta">
							<span class="kind">{e.kind}</span>
							· suggested by {e.origin === 'user' ? 'you' : e.origin}
							· {formatDateTime(e.created_at)}
							{#if e.status !== 'open'}· <span class="status">{e.status}</span>{/if}
						</div>
					</div>
					{#if e.first_played_at}
						<div class="landed" title="You've played this since it was suggested">
							played {e.plays}×
						</div>
					{/if}
				</div>

				{#if e.reason}<p class="reason">{e.reason}</p>{/if}
				{#if e.because}
					<!-- The grounding. Required at the door, because a suggestion with
					     no root in your own listening is free association. -->
					<p class="because"><span>because</span> {e.because}</p>
				{/if}

				<div class="actions">
					{#if e.status === 'open'}
						<button class="btn btn-primary" on:click={() => mark(e, 'acquired')}>Got it</button>
						<button class="btn btn-secondary" on:click={() => mark(e, 'dismissed')}>Not for me</button>
					{:else}
						<button class="btn btn-secondary" on:click={() => mark(e, 'open')}>Back to open</button>
					{/if}
					<a class="btn btn-secondary" href={musicbrainz(e)} target="_blank" rel="noopener noreferrer">MusicBrainz</a>
					<a class="btn btn-secondary" href={discogs(e)} target="_blank" rel="noopener noreferrer">Discogs</a>
					<button class="del" on:click={() => remove(e)}>Remove</button>
				</div>
			</div>
		{/each}
		{#if !shown.length}
			<p class="dim">Nothing in that view.</p>
		{/if}
	</div>
{/if}

<style>
	.head { margin-bottom: 20px; }
	h1 { font-size: 28px; margin: 0 0 4px; }
	.sub { font-size: 14px; color: var(--td-muted); margin: 0; }
	.eyebrow {
		font-size: 10.5px; letter-spacing: 0.1em; text-transform: uppercase;
		color: var(--td-muted-3); margin-bottom: 10px;
	}
	.scores { border-radius: 18px; padding: 18px; margin-bottom: 18px; }
	.scores .rows { display: flex; gap: 22px; flex-wrap: wrap; }
	.score { display: flex; align-items: baseline; gap: 7px; }
	.who { font-weight: 600; font-size: 14px; }
	.num { font-size: 18px; color: var(--td-accent); }
	.foot { margin: 12px 0 0; }
	.tabs { display: flex; gap: 6px; margin-bottom: 14px; flex-wrap: wrap; }
	/* Svelte scopes styles per component, so this is defined here rather than
	   inherited — matching the existing definition exactly matters more than
	   deduplicating it. */
	.seg-btn {
		background: var(--td-surface2); border: 1px solid var(--td-border);
		color: var(--td-muted); border-radius: 999px; padding: 6px 14px;
		font-size: 12.5px; cursor: pointer;
	}
	.seg-btn.on { background: var(--td-accent); border-color: var(--td-accent); color: var(--td-bg); }
	.list { display: flex; flex-direction: column; gap: 10px; }
	.card {
		background: var(--td-surface2); border: 1px solid var(--td-border);
		border-radius: 16px; padding: 16px;
	}
	.card.played { border-color: var(--td-accent); }
	.top { display: flex; justify-content: space-between; gap: 12px; align-items: flex-start; }
	.title { font-weight: 600; font-size: 15.5px; }
	.meta { font-size: 12px; color: var(--td-muted-2); margin-top: 4px; }
	.kind {
		font-size: 10px; letter-spacing: 0.07em; text-transform: uppercase;
		color: var(--td-muted-3);
	}
	.status { color: var(--td-muted); }
	.landed {
		font-size: 11.5px; color: var(--td-accent); white-space: nowrap;
		border: 1px solid var(--td-accent); border-radius: 999px; padding: 3px 10px;
	}
	.reason { font-size: 13.5px; color: var(--td-nav-fg); margin: 10px 0 0; }
	.because { font-size: 12.5px; color: var(--td-muted); margin: 6px 0 0; }
	.because span {
		font-size: 10px; letter-spacing: 0.07em; text-transform: uppercase;
		color: var(--td-muted-3); margin-right: 5px;
	}
	.actions { display: flex; gap: 8px; flex-wrap: wrap; margin-top: 14px; align-items: center; }
	.actions .btn { font-size: 12.5px; text-decoration: none; }
	.del {
		background: none; border: 0; color: #b2622d; font-size: 12.5px;
		cursor: pointer; margin-left: auto;
	}
	.empty { border-radius: 18px; padding: 28px; text-align: center; }
	.dim { font-size: 13px; color: var(--td-muted-2); }
	.spin {
		width: 22px; height: 22px; border: 2px solid var(--td-track);
		border-top-color: var(--td-accent); border-radius: 50%;
		margin: 40px auto; animation: spin 0.8s linear infinite;
	}
	@keyframes spin { to { transform: rotate(360deg); } }
	@media (prefers-reduced-motion: reduce) { .spin { animation: none; } }
</style>
