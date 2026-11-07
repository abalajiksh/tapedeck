<script lang="ts">
	// Signal Chains with no chain yet: what a chain is, its shape, and four
	// ways to start one. Each opens the new-chain wizard with that start in
	// mind. When something already reports plays as a box you own (tied in
	// the inbox), it is offered first: a chain from it files its plays.
	import type { Device, Equipment } from '$lib/api';
	import ChainIcon from '$lib/components/ChainIcon.svelte';
	import { groupOf } from './gear';

	export let equipment: Equipment[] = [];
	export let devices: Device[] = [];
	/** Plays from Shared Spools are kept At a friend's already. */
	export let hasFriendChain = false;

	const STARTS = [
		{ key: 'desk', icon: 'desk', title: 'At a desk', sub: 'A computer into speakers or headphones', shape: 'Computer → DAC? → Speakers' },
		{ key: 'go', icon: 'phone', title: 'On the go', sub: 'A phone or player into whatever’s in your ears', shape: 'Phone → Headphones ▾' },
		{ key: 'records', icon: 'turntable', title: 'Records & tapes', sub: 'A turntable or deck, logged from your shelf', shape: 'Turntable → Phono → Speakers' },
		{ key: 'blank', icon: null, title: 'Something else', sub: 'Car, living room, a friend’s place', shape: 'Start blank' }
	];

	// A box you own that something already reports as.
	$: heard = equipment.find((e) => groupOf(e.equipment_type) === 'from' && (e.reporters?.length ?? 0) > 0) ?? null;
	// Otherwise a machine that has sent plays, not tied to a box yet.
	$: machine = heard ? null : (devices.find((d) => d.total_listens > 0) ?? null);
</script>

<section class="first" aria-label="Start with a signal chain">
	<div class="intro panel">
		<h2 class="h-display">Where do you listen?</h2>
		<p>
			A signal chain is a place or a moment: your desk, the commute, the record player. Tell Tapedeck about one,
			and every play from it gets the gear it ran through, so you can see hours on your headphones, wear on your
			stylus, and which gear you actually use.
		</p>
		<p class="muted">Start with one. Tapedeck asks about the rest as plays come in.</p>
		<div class="shape" aria-label="The shape of a chain">
			<span class="sbox">Plays from</span><span class="ar">→</span>
			<span class="sbox opt">in between (optional)</span><span class="ar">→</span>
			<span class="sbox">Heard on</span>
		</div>
		<p class="muted small">Every chain is this shape. Most only need the two ends.</p>
	</div>

	{#if heard}
		<a class="heard panel" href="/chains/new?src={heard.id}">
			<span class="hb">
				<b>Tapedeck has already heard from your {heard.name} through {heard.reporters.join(', ')}</b>
				<small>Its plays are kept and waiting. Start a chain from it and new ones file themselves; the inbox offers the earlier ones.</small>
			</span>
			<span class="btn btn-primary">Start from this {heard.equipment_type === 'phone' ? 'phone' : 'box'}</span>
		</a>
	{:else if machine}
		<div class="heard panel">
			<span class="hb">
				<b>Tapedeck has already heard from {machine.name ?? machine.machine_id}</b>
				<small>{machine.total_listens} plays are kept and waiting. Make a chain for it, and the inbox will offer to file them.</small>
			</span>
		</div>
	{/if}

	<div class="kicker">Pick a starting point</div>
	<div class="starts">
		{#each STARTS as s (s.key)}
			<a class="start panel" href="/chains/new?start={s.key}">
				<span class="si" aria-hidden="true">{#if s.icon}<ChainIcon name={s.icon} size={20} title="" />{:else}+{/if}</span>
				<b>{s.title}</b>
				<small>{s.sub}</small>
				<span class="ss">{s.shape}</span>
			</a>
		{/each}
	</div>

	<p class="muted foot">
		Not now? Nothing is lost. Plays without a chain are kept, and you can file them whenever you set one up.
		{#if hasFriendChain}Plays you accepted from Shared Spools are already kept At a friend’s.{/if}
	</p>
</section>

<style>
	.first {
		display: flex;
		flex-direction: column;
		gap: 14px;
		max-width: 980px;
	}
	.intro {
		border-radius: 22px;
		padding: 22px 24px;
	}
	.intro h2 {
		font-size: 24px;
		margin: 0 0 8px;
	}
	.intro p {
		margin: 0 0 8px;
		font-size: 14px;
		line-height: 1.5;
		max-width: 72ch;
	}
	.muted {
		color: var(--td-muted);
	}
	.small {
		font-size: 12.5px !important;
	}
	.shape {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
		margin: 12px 0 6px;
	}
	.sbox {
		padding: 6px 14px;
		border-radius: 999px;
		background: var(--td-surface2);
		font-size: 13px;
		font-weight: 600;
	}
	.sbox.opt {
		background: none;
		border: 1px dashed var(--td-border-2);
		color: var(--td-muted);
		font-weight: 500;
	}
	.ar {
		color: var(--td-muted-3);
	}
	.heard {
		display: flex;
		align-items: center;
		gap: 14px;
		flex-wrap: wrap;
		border-radius: 20px;
		padding: 16px 20px;
		color: inherit;
		text-decoration: none;
		border: 1px solid color-mix(in srgb, var(--td-accent) 32%, transparent);
		background: color-mix(in srgb, var(--td-accent) 7%, var(--td-surface));
	}
	.hb {
		flex: 1 1 320px;
		display: flex;
		flex-direction: column;
		gap: 3px;
	}
	.hb small {
		color: var(--td-muted);
		font-size: 12.5px;
	}
	.kicker {
		font-size: 11px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		font-weight: 700;
		color: var(--td-muted-2);
		margin-top: 4px;
	}
	.starts {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
		gap: 10px;
	}
	.start {
		display: flex;
		flex-direction: column;
		gap: 4px;
		border-radius: 20px;
		padding: 16px;
		color: inherit;
		text-decoration: none;
		border: 1px solid var(--td-border);
	}
	.start:hover {
		border-color: var(--td-accent);
	}
	.si {
		width: 36px;
		height: 36px;
		border-radius: 12px;
		display: grid;
		place-items: center;
		background: var(--td-surface2);
		color: var(--td-muted);
		margin-bottom: 6px;
		font-weight: 700;
	}
	.start small {
		color: var(--td-muted);
		font-size: 12.5px;
	}
	.ss {
		margin-top: 8px;
		font-size: 12px;
		color: var(--td-muted-2);
	}
	.foot {
		font-size: 12.5px;
		margin: 0;
	}
</style>
