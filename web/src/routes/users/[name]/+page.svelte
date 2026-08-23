<script lang="ts">
	// A deck, seen through an accepted patch.
	//
	// **This route is the actor URL.** `GET /users/:name` content-negotiates on
	// the server: a caller naming `activity+json` gets the actor document, a
	// browser gets the SPA — and until now the SPA rendered its own 404 into
	// that slot. This is what fills it, and no routing change was needed.
	//
	// Everything here comes from the deck endpoint, which returns nothing at all
	// without an accepted patch. "No such deck" and "you are not patched into
	// it" are one answer, the rule the whole of Patch follows.
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { api } from '$lib/api';
	import type { DeckView } from '$lib/api';
	import ChainIcon from '$lib/components/ChainIcon.svelte';

	let deck: DeckView | null = null;
	let loading = true;
	let denied = false;

	$: username = $page.params.name ?? '';

	onMount(async () => {
		try {
			deck = await api.getDeck(username);
		} catch {
			denied = true;
		} finally {
			loading = false;
		}
	});

	function initials(d: DeckView) {
		const n = (d.display_name || d.username).trim();
		const parts = n.split(/\s+/).filter(Boolean);
		return (
			((parts[0]?.[0] ?? '') + (parts[1]?.[0] ?? '')).toUpperCase() ||
			n.slice(0, 2).toUpperCase()
		);
	}

	function steps(c: { components: string }): { name: string; type: string }[] {
		try {
			return JSON.parse(c.components ?? '[]');
		} catch {
			return [];
		}
	}

	const ROLE_COLOURS: Record<string, string> = {
		source: 'var(--td-accent)',
		dac: 'var(--td-accent-2)',
		amp: 'var(--td-muted-2)',
		transducer: 'var(--td-accent-hi)'
	};
	const roleColour = (t: string) => ROLE_COLOURS[t] ?? 'var(--td-muted-3)';

	const hours = (h: number) => (h >= 1 ? `${h.toFixed(1)}h` : `${Math.round(h * 60)}min`);

	let copied = '';
	async function copy(text: string, which: string) {
		try {
			await navigator.clipboard.writeText(text);
			copied = which;
			setTimeout(() => (copied = ''), 1600);
		} catch {
			/* a browser refusing the clipboard is not worth a dialog */
		}
	}
</script>

<svelte:head><title>{deck ? deck.display_name || deck.username : 'Deck'} — Tapedeck</title></svelte:head>

{#if loading}
	<div class="panel note">Loading…</div>
{:else if denied || !deck}
	<div class="panel empty">
		<div class="h-display" style="font-size:20px;margin-bottom:6px;">Nothing here</div>
		<p class="esub">
			There's no deck called <code>{username}</code> that you're patched into. If they're on
			this instance, ask to patch in from <a href="/patch?tab=discover">Discover</a>.
		</p>
	</div>
{:else}
	<div class="hero">
		<span class="avatar">{initials(deck)}</span>
		<div class="who">
			<h1 class="h-display">{deck.display_name || deck.username}</h1>
			<div class="handle">@{deck.username}</div>
			{#if deck.bio}<p class="bio">{deck.bio}</p>{/if}
		</div>
		<div class="count">
			<div class="big">{deck.listens.toLocaleString()}</div>
			<div class="clabel">listens shared</div>
		</div>
	</div>

	{#if deck.both_play.length}
		<h2 class="h-display sech">You both play</h2>
		<p class="sub">Overlap, shown as overlap — the artists, not a score.</p>
		<div class="chips">
			{#each deck.both_play as a}<span class="chip">{a}</span>{/each}
		</div>
	{/if}

	<h2 class="h-display sech">Chains they share</h2>
	<p class="sub">
		Only the chains they switched to shared. What isn't here isn't theirs to be asked about.
	</p>
	{#if deck.chains.length}
		<div class="chaingrid">
			{#each deck.chains as c}
				<div class="chaincard">
					<div class="cname"><ChainIcon name={c.icon} size={16} title={c.name} />{c.name}</div>
					<div class="cpath">
						{#each steps(c) as st, i}
							<span class="cstep">
								<span class="dot" style="background:{roleColour(st.type)};"></span>{st.name}
							</span>
							{#if i < steps(c).length - 1}<span class="arrow">→</span>{/if}
						{:else}
							<span class="cnone">No steps listed</span>
						{/each}
					</div>
					<div class="chrs">{hours(c.hours)} logged</div>
				</div>
			{/each}
		</div>
	{:else}
		<div class="panel empty small">
			<p class="esub">
				They publish no chains yet, so nothing of theirs reaches your Reel. That's theirs to
				switch on, not yours.
			</p>
		</div>
	{/if}

	<h2 class="h-display sech">Where this deck answers</h2>
	{#if deck.actor}
		<p class="sub">The addresses another server uses to find them.</p>
		<div class="uris">
			{#each [['Actor', deck.actor], ['Inbox', `${deck.actor}/inbox`], ['Outbox', `${deck.actor}/outbox`], ['Followers', `${deck.actor}/followers`]] as [label, uri]}
				<div class="urirow">
					<span class="urilabel">{label}</span>
					<code>{uri}</code>
					<button class="btn btn-secondary" on:click={() => copy(uri, label)}>
						{copied === label ? '✓' : 'Copy'}
					</button>
				</div>
			{/each}
		</div>
	{:else}
		<p class="sub">
			They haven't switched federation on, so this deck has no address outside the instance —
			a normal state, and nothing to do with whether you can see it here.
		</p>
	{/if}

	<p class="footnote">
		You're seeing this because they accepted your patch. It shows listens from their shared
		chains and nothing else — no history, no counts, no notes.
	</p>
{/if}

<style>
	.note,
	.empty {
		border-radius: 22px;
		padding: 30px;
	}
	.empty { text-align: center; }
	.empty.small { text-align: left; padding: 20px; }
	.esub { font-size: 13.5px; color: var(--td-muted); margin: 0; line-height: 1.5; }

	.hero {
		display: flex;
		align-items: center;
		gap: 18px;
		flex-wrap: wrap;
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 24px;
		padding: 22px;
		margin-bottom: 26px;
	}
	.avatar {
		width: 64px;
		height: 64px;
		border-radius: 999px;
		background: var(--td-surface2);
		color: var(--td-muted);
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: 20px;
		font-weight: 700;
		flex: none;
	}
	.who { flex: 1; min-width: 200px; }
	h1 { font-size: 28px; margin: 0; }
	.handle {
		font: 500 12px ui-monospace, SFMono-Regular, Menlo, monospace;
		color: var(--td-muted-3);
		margin-top: 2px;
	}
	.bio { margin: 8px 0 0; font-size: 13.5px; color: var(--td-muted); max-width: 56ch; }
	.count { text-align: right; flex: none; }
	.big { font-family: 'Caprasimo', serif; font-size: 30px; line-height: 1; }
	.clabel {
		font-size: 11px;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--td-muted-2);
		margin-top: 5px;
	}

	.sech { font-size: 20px; margin: 0 0 4px; }
	.sub { margin: 0 0 14px; font-size: 12.5px; color: var(--td-muted-2); max-width: 62ch; }
	.chips { display: flex; flex-wrap: wrap; gap: 7px; margin-bottom: 28px; }
	.chip {
		font-size: 12.5px;
		padding: 5px 12px;
		border-radius: 999px;
		background: var(--td-good-bg);
		color: var(--td-good-fg);
	}

	.chaingrid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
		gap: 12px;
		margin-bottom: 28px;
	}
	.chaincard {
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 18px;
		padding: 16px 18px;
	}
	.cname {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 14.5px;
		font-weight: 600;
		margin-bottom: 8px;
	}
	.cpath { display: flex; flex-wrap: wrap; align-items: center; gap: 7px; font-size: 12.5px; }
	.cstep { display: inline-flex; align-items: center; gap: 6px; color: var(--td-muted); }
	.dot { width: 7px; height: 7px; border-radius: 999px; display: block; }
	.arrow { color: var(--td-muted-3); }
	.cnone { color: var(--td-muted-3); font-style: italic; }
	.chrs { font-size: 12px; color: var(--td-muted-3); margin-top: 10px; }

	.uris { display: flex; flex-direction: column; gap: 8px; margin-bottom: 24px; }
	.urirow {
		display: flex;
		align-items: center;
		gap: 12px;
		flex-wrap: wrap;
		background: var(--td-surface2);
		border-radius: 14px;
		padding: 10px 14px;
	}
	.urilabel {
		font-size: 11px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--td-muted-2);
		width: 74px;
		flex: none;
	}
	.urirow code {
		flex: 1;
		min-width: 0;
		font: 500 11.5px ui-monospace, SFMono-Regular, Menlo, monospace;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.footnote {
		margin: 8px 0 0;
		font-size: 12.5px;
		color: var(--td-muted-2);
		max-width: 62ch;
		line-height: 1.55;
	}
</style>
