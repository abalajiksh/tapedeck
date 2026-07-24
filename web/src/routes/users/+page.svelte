<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type User } from '$lib/api';
	import { formatDateTime } from '$lib/utils';

	let users: User[] = [];
	let meId = 0;
	let loading = true;
	let error = '';

	// create
	let showCreate = false;
	let cUser = '', cDisplay = '', cPass = '';
	let creating = false;
	let createdToken = '';
	let tokenCopied = false;

	// reset password
	let resetFor: User | null = null;
	let rpPass = '';
	let resetting = false;

	let confirmDelete: User | null = null;

	async function load() {
		loading = true;
		error = '';
		try {
			const [u, me] = await Promise.all([api.getUsers(), api.me()]);
			users = u.users;
			meId = me.user_id;
		} catch (e) {
			error = String(e).includes('403') ? 'Admin privileges required.' : (e instanceof Error ? e.message : 'Failed to load users');
		}
		loading = false;
	}

	async function createUser() {
		if (!cUser.trim()) return;
		creating = true;
		createdToken = '';
		try {
			const res = await api.createUser(cUser.trim(), cDisplay.trim() || undefined, cPass || undefined);
			createdToken = res.token;
			cUser = ''; cDisplay = ''; cPass = '';
			await load();
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		creating = false;
	}

	async function setRole(u: User, role: string) {
		try {
			await api.updateUser(u.id, { role });
			users = users.map((x) => (x.id === u.id ? { ...x, role } : x));
		} catch (e) {
			alert(String(e).replace(/^API \d+: /, ''));
			await load(); // revert the select to the true state
		}
	}

	async function toggleDisabled(u: User) {
		try {
			await api.updateUser(u.id, { disabled: !u.disabled });
			users = users.map((x) => (x.id === u.id ? { ...x, disabled: !u.disabled } : x));
		} catch (e) { alert(String(e).replace(/^API \d+: /, '')); }
	}

	async function resetPassword() {
		if (!resetFor || rpPass.length < 8) return;
		resetting = true;
		try {
			await api.updateUser(resetFor.id, { password: rpPass });
			resetFor = null; rpPass = '';
		} catch (e) { alert(String(e).replace(/^API \d+: /, '')); }
		resetting = false;
	}

	async function doDelete(u: User) {
		try {
			await api.deleteUser(u.id);
			users = users.filter((x) => x.id !== u.id);
		} catch (e) { alert(String(e).replace(/^API \d+: /, '')); }
		confirmDelete = null;
	}

	function copyToken() {
		navigator.clipboard.writeText(createdToken);
		tokenCopied = true;
		setTimeout(() => (tokenCopied = false), 2000);
	}

	onMount(load);
</script>

<svelte:head><title>Users — Tapedeck</title></svelte:head>

<div style="display:flex;align-items:flex-end;justify-content:space-between;gap:16px;margin-bottom:24px;flex-wrap:wrap;">
	<div>
		<h1 class="h-display" style="font-size:38px;">Users</h1>
		<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;">Manage accounts, roles, and access to this instance.</p>
	</div>
	<button class="btn btn-primary" on:click={() => { showCreate = true; createdToken = ''; }}>+ New user</button>
</div>

{#if loading}
	<div style="display:flex;justify-content:center;padding:60px;"><div class="spin" style="width:22px;height:22px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
{:else if error}
	<div class="panel" style="border-radius:22px;padding:24px;text-align:center;color:#b2622d;">{error}</div>
{:else}
	<div class="panel" style="border-radius:22px;overflow:hidden;padding:6px 20px 14px;">
		<table class="table">
			<thead><tr><th>User</th><th>Role</th><th>Created</th><th style="text-align:right;">Actions</th></tr></thead>
			<tbody>
				{#each users as u (u.id)}
					<tr style={u.disabled ? 'opacity:.55;' : ''}>
						<td>
							<span style="font-weight:600;">{u.username}</span>
							{#if u.display_name}<span style="color:var(--td-muted);margin-left:6px;">({u.display_name})</span>{/if}
							{#if u.id === meId}<span class="tag tag-neutral" style="margin-left:6px;">you</span>{/if}
							{#if u.disabled}<span class="tag tag-outline" style="margin-left:6px;">disabled</span>{/if}
						</td>
						<td>
							<select class="input" style="width:auto;min-width:100px;padding:4px 12px;min-height:32px;"
								value={u.role} on:change={(e) => setRole(u, e.currentTarget.value)}>
								<option value="user">user</option>
								<option value="admin">admin</option>
							</select>
						</td>
						<td style="color:var(--td-muted);">{formatDateTime(u.created_at)}</td>
						<td style="text-align:right;white-space:nowrap;">
							<button class="btn btn-secondary" style="padding:5px 12px;font-size:12.5px;" on:click={() => { resetFor = u; rpPass = ''; }}>Reset password</button>
							{#if u.id !== meId}
								<button class="btn btn-secondary" style="padding:5px 12px;font-size:12.5px;margin-left:6px;" on:click={() => toggleDisabled(u)}>{u.disabled ? 'Enable' : 'Disable'}</button>
								<button class="btn btn-secondary" style="padding:5px 12px;font-size:12.5px;color:#b2622d;margin-left:6px;" on:click={() => (confirmDelete = u)}>Delete</button>
							{/if}
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
{/if}

<!-- Create user dialog -->
{#if showCreate}
	<div class="dialog-backdrop" on:click={() => (showCreate = false)} on:keydown={() => {}} role="presentation">
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">New user</div>
			<div class="dialog-body">
				{#if createdToken}
					<div style="background:var(--td-panel);border-radius:16px;padding:16px;margin-bottom:6px;">
						<div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:#e0a06a;margin-bottom:8px;">User created — their API token (shown once)</div>
						<div style="display:flex;gap:8px;align-items:center;">
							<code style="flex:1;background:var(--td-panel2);color:#a9c48a;border-radius:10px;padding:10px 12px;font-size:13px;word-break:break-all;">{createdToken}</code>
							<button class="btn btn-primary" on:click={copyToken}>{tokenCopied ? '✓' : 'Copy'}</button>
						</div>
					</div>
				{:else}
					<div style="display:flex;flex-direction:column;gap:13px;">
						<div class="field"><label for="nu">Username</label><input id="nu" class="input" bind:value={cUser} placeholder="partner"></div>
						<div class="field"><label for="nd">Display name <span style="opacity:.6">(optional)</span></label><input id="nd" class="input" bind:value={cDisplay}></div>
						<div class="field"><label for="np">Password <span style="opacity:.6">(optional — token-only login if blank)</span></label><input id="np" class="input" type="password" bind:value={cPass} placeholder="At least 8 characters"></div>
					</div>
				{/if}
			</div>
			<div class="dialog-actions">
				{#if createdToken}
					<button class="btn btn-primary" on:click={() => (showCreate = false)}>Done</button>
				{:else}
					<button class="btn btn-secondary" on:click={() => (showCreate = false)}>Cancel</button>
					<button class="btn btn-primary" on:click={createUser} disabled={creating || !cUser.trim() || (cPass.length > 0 && cPass.length < 8)}>{creating ? 'Creating…' : 'Create user'}</button>
				{/if}
			</div>
		</div>
	</div>
{/if}

<!-- Reset password dialog -->
{#if resetFor}
	<div class="dialog-backdrop" on:click={() => (resetFor = null)} on:keydown={() => {}} role="presentation">
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">Reset password — {resetFor.username}</div>
			<div class="dialog-body">
				<div class="field"><label for="rp">New password</label><input id="rp" class="input" type="password" bind:value={rpPass} placeholder="At least 8 characters"></div>
			</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (resetFor = null)}>Cancel</button>
				<button class="btn btn-primary" on:click={resetPassword} disabled={resetting || rpPass.length < 8}>{resetting ? 'Saving…' : 'Set password'}</button>
			</div>
		</div>
	</div>
{/if}

<!-- Confirm delete -->
{#if confirmDelete}
	<div class="dialog-backdrop" on:click={() => (confirmDelete = null)} on:keydown={() => {}} role="presentation">
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">Delete “{confirmDelete.username}”?</div>
			<div class="dialog-body" style="color:var(--td-nav-fg);">This permanently removes the user and <strong>all their listens, chains, tokens, and history</strong>. This can't be undone.</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (confirmDelete = null)}>Cancel</button>
				<button class="btn btn-primary" style="background:#b2622d;" on:click={() => confirmDelete && doDelete(confirmDelete)}>Delete user</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.spin { animation: td-spin 0.7s linear infinite; }
</style>
