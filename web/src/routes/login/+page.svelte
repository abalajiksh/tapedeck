<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';

	let username = '';
	let password = '';
	let error = '';
	let loading = false;

	/// Where to land after signing in.
	///
	/// Only a same-site path is honoured — a `next` of `https://evil.example`
	/// would turn the login page into an open redirect, and one that arrives
	/// after a successful login is the most credible kind.
	function nextPath(): string {
		const next = new URLSearchParams(window.location.search).get('next');
		if (next && next.startsWith('/') && !next.startsWith('//')) return next;
		return '/';
	}

	onMount(async () => {
		try {
			const status = await api.authStatus();
			if (status.needs_setup) return (window.location.href = '/setup');
			if (status.authenticated) return (window.location.href = nextPath());
		} catch { /* proceed to login form */ }
	});

	async function handleLogin() {
		error = '';
		loading = true;
		try {
			await api.login(username, password);
			window.location.href = nextPath();
		} catch (e) {
			error = String(e).includes('401') ? 'Invalid username or password' : 'Connection error';
		}
		loading = false;
	}

	function onKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && username && password) handleLogin();
	}
</script>

<svelte:head><title>Login — Tapedeck</title></svelte:head>

<div style="min-height:100vh;display:flex;align-items:center;justify-content:center;padding:32px;background:radial-gradient(120% 120% at 50% 0%, var(--td-surface) 0%, var(--td-bg) 100%);">
	<div style="width:100%;max-width:420px;">
		<div style="text-align:center;margin-bottom:26px;">
			<div style="display:inline-flex;align-items:center;justify-content:center;width:64px;height:64px;border-radius:20px;background:var(--td-panel);box-shadow:0 8px 24px rgba(46,43,37,.25);">
				<svg viewBox="9 17 46 30" width="34"><line x1="23" y1="43" x2="45" y2="43" stroke="#F2E6D0" stroke-width="4.5" stroke-linecap="round"/><circle cx="23" cy="31" r="12" fill="#E9A23B"/><circle cx="45" cy="35" r="8" fill="#E9A23B"/><circle cx="23" cy="31" r="3.6" fill="#261A15"/><circle cx="45" cy="35" r="3" fill="#261A15"/></svg>
			</div>
			<h1 class="h-display" style="font-size:40px;margin:14px 0 2px;">Tapedeck</h1>
			<p style="margin:0;color:var(--td-muted);font-size:14.5px;">Sign in to your instance</p>
		</div>

		<div class="panel" style="border-radius:26px;box-shadow:0 12px 32px rgba(46,43,37,.12);padding:26px;">
			{#if error}
				<div style="background:var(--td-good-bg);filter:hue-rotate(300deg);border-radius:14px;padding:10px 14px;margin-bottom:14px;">
					<p style="margin:0;font-size:13px;color:#b2622d;">{error}</p>
				</div>
			{/if}
			<div style="display:flex;flex-direction:column;gap:14px;">
				<div class="field"><label for="u">Username</label>
					<input id="u" class="input" bind:value={username} on:keydown={onKeydown} autocomplete="username" placeholder="admin"></div>
				<div class="field"><label for="p">Password</label>
					<input id="p" class="input" type="password" bind:value={password} on:keydown={onKeydown} autocomplete="current-password" placeholder="••••••••"></div>
				<button class="btn btn-primary btn-block" on:click={handleLogin} disabled={loading || !username || !password}>
					{loading ? 'Signing in…' : 'Sign in'}
				</button>
				<p style="text-align:center;margin:2px 0 0;font-size:13px;color:var(--td-muted);">First run? <a href="/setup">Set up your instance</a></p>
			</div>
		</div>
		<p style="text-align:center;margin:20px 0 0;font-size:12px;color:var(--td-muted-2);">A self-hosted listening journal</p>
	</div>
</div>
