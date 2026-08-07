<script lang="ts">
	/** The three headline trends under the band: value, change, and what it was. */
	import { delta, deltaText } from './bits';

	export let rows: { label: string; value: string; now: number; before: number | null | undefined; was: string | null }[] = [];
	export let size = 30;
</script>

<div class="trio">
	{#each rows as r}
		{@const d = delta(r.now, r.before)}
		<div class="t">
			<div class="k">{r.label}</div>
			<div class="v">
				<span class="h-display" style="font-size:{size}px;">{r.value}</span>
				{#if d != null}
					<span class="d" class:up={d > 0} class:down={d < 0}>{deltaText(d)}</span>
				{/if}
			</div>
			{#if r.was}<div class="was">was {r.was}</div>{/if}
		</div>
	{/each}
</div>

<style>
	.trio { display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; margin-bottom: 24px; }
	.t { background: var(--td-track); border-radius: 16px; padding: 16px 18px; min-width: 0; }
	.k {
		font-size: 10.5px; letter-spacing: 0.1em; text-transform: uppercase;
		color: var(--td-muted-2); margin-bottom: 8px;
	}
	.v { display: flex; align-items: baseline; gap: 8px; flex-wrap: wrap; }
	.d { font-size: 13px; font-weight: 600; color: var(--td-muted-2); }
	.d.up { color: var(--td-good-fg); }
	.d.down { color: #b2622d; }
	.was { font-size: 12px; color: var(--td-muted-3); margin-top: 3px; }
	@media (max-width: 620px) { .trio { grid-template-columns: 1fr; } }
</style>
