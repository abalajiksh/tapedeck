<script lang="ts">
	// The Signal Chains inbox: every question about plays that landed in no
	// chain, in one place. Signal Chains shows the first as a card; this lists
	// them all. Answers can be changed later from the chain they went to.
	import { onMount } from 'svelte';
	import { api, type Equipment, type InboxQuestion, type SignalChain } from '$lib/api';
	import InboxCard from '$lib/components/chains/InboxCard.svelte';

	let questions: InboxQuestion[] = [];
	let chains: SignalChain[] = [];
	let equipment: Equipment[] = [];
	let windowDays = 30;
	let loading = true;
	let error = '';
	let done: string[] = [];

	async function load() {
		try {
			const [q, c, g] = await Promise.all([api.getChainInbox(), api.getChains(), api.getEquipment()]);
			questions = q.questions;
			windowDays = q.window_days;
			chains = c.chains;
			equipment = g.equipment;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not load the inbox';
		}
		loading = false;
	}
	onMount(load);

	async function answered(text: string) {
		done = [text, ...done].slice(0, 5);
		await load();
	}
	$: headline = !questions.length
		? 'All caught up'
		: `${questions.length} ${questions.length === 1 ? 'thing' : 'things'} to answer`;
</script>

<svelte:head><title>Inbox · Signal Chains — Tapedeck</title></svelte:head>

<a class="back" href="/chains">‹ Signal Chains</a>
<div class="kicker">Signal Chains · inbox</div>
<h1 class="h-display">{loading ? 'Inbox' : headline}</h1>
<p class="intro">
	Tapedeck only asks when a play would otherwise land in no chain. Each question is asked once,
	and your answer covers future plays too. It looks back {windowDays} days.
</p>

{#each done as d}
	<div class="done">✓ {d}</div>
{/each}

{#if loading}
	<div class="panel note">Loading…</div>
{:else if error}
	<div class="panel note">{error}</div>
{:else if !questions.length}
	<div class="panel note">Nothing to answer. Every play from the last {windowDays} days landed in a chain.</div>
{:else}
	<div class="list">
		{#each questions as q (q.key)}
			<InboxCard {q} {chains} {equipment} on:done={(e) => answered(e.detail)} />
		{/each}
	</div>
{/if}

<p class="foot">Answers can be changed later from the chain they went to, or from the Outputs and Machines tabs.</p>

<style>
	.back {
		font-size: 13.5px;
		color: var(--td-accent);
	}
	.kicker {
		margin-top: 8px;
		font-size: 11px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		font-weight: 700;
		color: var(--td-muted-2);
	}
	h1 {
		font-size: 34px;
		margin: 4px 0 0;
	}
	.intro {
		margin: 6px 0 18px;
		max-width: 66ch;
		color: var(--td-muted);
		font-size: 14px;
	}
	.note {
		border-radius: 20px;
		padding: 22px;
		color: var(--td-muted);
	}
	.list {
		display: flex;
		flex-direction: column;
		gap: 14px;
		max-width: 900px;
	}
	.done {
		max-width: 900px;
		margin-bottom: 10px;
		padding: 10px 14px;
		border-radius: 14px;
		background: var(--td-good-bg);
		color: var(--td-good-fg);
		font-size: 13.5px;
	}
	.foot {
		margin-top: 18px;
		font-size: 12.5px;
		color: var(--td-muted-2);
	}
</style>
