<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import TapedeckLogo from '$lib/components/TapedeckLogo.svelte';

	let username = 'admin';
	let displayName = '';
	let password = '';
	let confirmPassword = '';
	let error = '';
	let loading = false;

	let setupComplete = false;
	let generatedToken = '';
	let tokenCopied = false;

	onMount(async () => {
		try {
			const status = await api.authStatus();
			if (!status.needs_setup) window.location.href = status.authenticated ? '/' : '/login';
		} catch { /* proceed to setup form */ }
	});

	async function handleSetup() {
		error = '';
		if (password !== confirmPassword) return (error = 'Passwords do not match');
		if (password.length < 8) return (error = 'Password must be at least 8 characters');
		if (!username.trim()) return (error = 'Username is required');

		loading = true;
		try {
			const result = await api.setup(username.trim(), password, displayName.trim() || undefined);
			generatedToken = result.token;
			setupComplete = true;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Setup failed';
		}
		loading = false;
	}

	function copyToken() {
		navigator.clipboard.writeText(generatedToken);
		tokenCopied = true;
		setTimeout(() => (tokenCopied = false), 2000);
	}
</script>

<svelte:head><title>Setup — Tapedeck</title></svelte:head>

<div style="min-height:100vh;display:flex;align-items:center;justify-content:center;padding:32px;background:radial-gradient(120% 120% at 50% 0%, var(--td-surface) 0%, var(--td-bg) 100%);">
	<div style="width:100%;max-width:440px;">
		<div style="text-align:center;margin-bottom:26px;">
			<div style="display:flex;justify-content:center;color:var(--td-text);"><TapedeckLogo width={88} /></div>
			<h1 class="h-display" style="font-size:40px;margin:14px 0 2px;">Tapedeck</h1>
			<p style="margin:0;color:var(--td-muted);font-size:14.5px;">Welcome — set up your admin account</p>
		</div>

		<div class="panel" style="border-radius:26px;box-shadow:0 12px 32px rgba(46,43,37,.12);padding:26px;">
			{#if setupComplete}
				<div style="display:flex;flex-direction:column;gap:16px;">
					<div style="text-align:center;">
						<div style="font-family:'Caprasimo',serif;font-size:22px;">Setup complete</div>
						<p style="margin:4px 0 0;color:var(--td-muted);font-size:14px;">Your admin account is ready.</p>
					</div>
					<div style="background:var(--td-panel);border-radius:16px;padding:16px;">
						<div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:#e0a06a;margin-bottom:8px;">Your API token — copy it now, it won't be shown again</div>
						<div style="display:flex;gap:8px;align-items:center;">
							<code style="flex:1;background:var(--td-panel2);color:#a9c48a;border-radius:10px;padding:10px 12px;font-size:13px;word-break:break-all;">{generatedToken}</code>
							<button class="btn btn-primary" on:click={copyToken}>{tokenCopied ? '✓ Copied' : 'Copy'}</button>
						</div>
					</div>
					<a class="btn btn-primary btn-block" href="/" style="text-decoration:none;">Go to Dashboard →</a>
				</div>
			{:else}
				{#if error}
					<div style="background:var(--td-good-bg);filter:hue-rotate(300deg);border-radius:14px;padding:10px 14px;margin-bottom:14px;">
						<p style="margin:0;font-size:13px;color:#b2622d;">{error}</p>
					</div>
				{/if}
				<div style="display:flex;flex-direction:column;gap:14px;">
					<div class="field"><label for="u">Username</label><input id="u" class="input" bind:value={username} autocomplete="username"></div>
					<div class="field"><label for="d">Display name <span style="opacity:.6">(optional)</span></label><input id="d" class="input" bind:value={displayName} placeholder="Ashwin"></div>
					<div class="field"><label for="p">Password</label><input id="p" class="input" type="password" bind:value={password} autocomplete="new-password" placeholder="At least 8 characters"></div>
					<div class="field"><label for="c">Confirm password</label><input id="c" class="input" type="password" bind:value={confirmPassword} autocomplete="new-password" placeholder="••••••••"></div>
					<button class="btn btn-primary btn-block" on:click={handleSetup} disabled={loading || !username || !password || !confirmPassword}>
						{loading ? 'Creating account…' : 'Create admin account'}
					</button>
					<p style="text-align:center;margin:2px 0 0;font-size:13px;color:var(--td-muted);">Already set up? <a href="/login">Sign in</a></p>
				</div>
			{/if}
		</div>
	</div>
</div>
