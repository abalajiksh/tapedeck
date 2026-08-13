<script lang="ts">
	// The OAuth consent screen.
	//
	// `/oauth/authorize` validates the request and redirects here carrying the
	// parameters. This page is where a human decides, which is the one step of
	// the whole flow that cannot be automated — everything before it is a
	// request, and this is the consent.
	//
	// Listed as public in `+layout.svelte` so the root auth gate does not
	// hard-redirect and destroy the query string on the way. It gates itself
	// instead, sending an unauthenticated visitor to the login page with a
	// `next` that brings them back here with the parameters intact.
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { AuthorizeInfo } from '$lib/api';

	let info: AuthorizeInfo | null = null;
	let error = '';
	let working = false;
	let params: URLSearchParams;

	onMount(async () => {
		params = new URLSearchParams(window.location.search);

		try {
			const status = await api.authStatus();
			if (!status.authenticated) {
				const next = window.location.pathname + window.location.search;
				window.location.href = `/login?next=${encodeURIComponent(next)}`;
				return;
			}
		} catch {
			error = 'Could not reach Tapedeck.';
			return;
		}

		try {
			info = await api.getAuthorizeInfo(params);
		} catch (e) {
			error = e instanceof Error ? e.message : 'That authorization request is not valid.';
		}
	});

	async function approve() {
		if (!info) return;
		working = true;
		try {
			const r = await api.approveAuthorization({
				client_id: params.get('client_id') ?? '',
				redirect_uri: params.get('redirect_uri') ?? '',
				code_challenge: params.get('code_challenge') ?? '',
				scope: params.get('scope'),
				state: params.get('state'),
				resource: params.get('resource')
			});
			window.location.href = r.redirect_to;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not complete the authorization.';
			working = false;
		}
	}

	function deny() {
		// Bounce back through the client's own redirect with an error, which is
		// what lets it tell the user the request was declined rather than
		// hanging forever waiting for a code.
		const uri = params.get('redirect_uri');
		if (!uri) return (window.location.href = '/');
		const sep = uri.includes('?') ? '&' : '?';
		const state = params.get('state');
		window.location.href =
			`${uri}${sep}error=access_denied&error_description=${encodeURIComponent('The user declined')}` +
			(state ? `&state=${encodeURIComponent(state)}` : '');
	}
</script>

<svelte:head><title>Authorize · Tapedeck</title></svelte:head>

<div class="wrap">
	<div class="card">
		{#if error}
			<h1 class="h-display">Something is wrong</h1>
			<p class="sub">{error}</p>
			<a class="btn btn-secondary" href="/">Back to Tapedeck</a>
		{:else if !info}
			<div class="spin"></div>
		{:else}
			<div class="eyebrow">Authorization request</div>
			<h1 class="h-display">{info.client_name}</h1>
			<p class="sub">wants to connect to your Tapedeck.</p>

			{#if !info.verified}
				<!-- An anonymously registered client picked its own name. Saying so
				     is the difference between informed consent and a rubber stamp. -->
				<p class="warn">
					This name was chosen by the application itself and has not been verified.
					Only continue if you started this from somewhere you trust.
				</p>
			{/if}

			<div class="grants">
				<div class="eyebrow">It will be able to</div>
				<ul>
					{#each info.scopes as s}
						<li>{s.description}</li>
					{/each}
				</ul>
			</div>

			<p class="note">
				You can revoke this at any time in Settings → AI Connections, where every call it
				makes is also recorded.
			</p>

			<div class="actions">
				<button class="btn btn-secondary" on:click={deny} disabled={working}>Deny</button>
				<button class="btn btn-primary" on:click={approve} disabled={working}>
					{working ? 'Authorizing…' : 'Approve'}
				</button>
			</div>

			<div class="dest">Redirects to <code>{info.redirect_uri}</code></div>
		{/if}
	</div>
</div>

<style>
	.wrap {
		min-height: 100vh;
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 24px;
		background: var(--td-bg);
	}
	.card {
		width: 100%;
		max-width: 460px;
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 22px;
		padding: 32px;
	}
	.eyebrow {
		font-size: 10.5px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		color: var(--td-muted-3);
		margin-bottom: 8px;
	}
	h1 {
		font-size: 26px;
		margin: 0 0 4px;
	}
	.sub {
		font-size: 14px;
		color: var(--td-muted);
		margin: 0 0 20px;
	}
	.warn {
		font-size: 12.5px;
		color: #b2622d;
		background: color-mix(in srgb, #b2622d 10%, transparent);
		border-radius: 12px;
		padding: 10px 13px;
		margin: 0 0 18px;
	}
	.grants {
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
		border-radius: 16px;
		padding: 16px;
		margin-bottom: 16px;
	}
	.grants ul {
		margin: 0;
		padding-left: 20px;
		display: flex;
		flex-direction: column;
		gap: 7px;
	}
	.grants li {
		font-size: 13.5px;
		color: var(--td-nav-fg);
	}
	.note {
		font-size: 12px;
		color: var(--td-muted-2);
		margin: 0 0 20px;
	}
	.actions {
		display: flex;
		gap: 10px;
	}
	.actions .btn {
		flex: 1;
	}
	.dest {
		font-size: 11px;
		color: var(--td-muted-3);
		margin-top: 16px;
		text-align: center;
		word-break: break-all;
	}
	.spin {
		width: 22px;
		height: 22px;
		border: 2px solid var(--td-track);
		border-top-color: var(--td-accent);
		border-radius: 50%;
		margin: 24px auto;
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
