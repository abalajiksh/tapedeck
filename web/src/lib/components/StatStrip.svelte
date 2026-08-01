<script lang="ts">
	import type { StatCard } from '$lib/api';

	export let stats: StatCard[] = [];
</script>

<!-- Values arrive pre-formatted; this only lays them out. The `dark` tone is
     the panel card the design uses for the one proportional stat in each strip
     (lossless share / share of listening). -->
<div class="strip">
	{#each stats as s}
		<div class="card" class:dark={s.tone === 'dark'}>
			<div class="k">{s.k}</div>
			<div class="v h-display">{s.v}</div>
			{#if s.sub}<div class="sub">{s.sub}</div>{/if}
		</div>
	{/each}
</div>

<style>
	.strip {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
		gap: 14px;
		margin-bottom: 30px;
	}
	.card {
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 22px;
		padding: 20px 22px;
	}
	.card.dark {
		background: var(--td-panel);
		border-color: transparent;
	}
	.k {
		font-size: 10.5px;
		letter-spacing: .11em;
		text-transform: uppercase;
		color: var(--td-muted-2);
		margin-bottom: 6px;
	}
	.card.dark .k { color: #c9a678; }
	.v { font-size: 30px; line-height: 1; }
	.card.dark .v { color: #f4e7cf; }
	.sub {
		font-size: 12px;
		color: var(--td-muted-2);
		margin-top: 5px;
	}
	.card.dark .sub { color: #b3a487; }
</style>
