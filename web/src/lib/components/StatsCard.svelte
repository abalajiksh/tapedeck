<script lang="ts">
	export let label: string;
	export let value: string | number;
	export let sub: string = '';
	export let accent: 'foam' | 'iris' | 'gold' | 'rose' | 'love' | 'pine' = 'foam';

	const accentColors: Record<string, string> = {
		foam:  'text-rp-foam',
		iris:  'text-rp-iris',
		gold:  'text-rp-gold',
		rose:  'text-rp-rose',
		love:  'text-rp-love',
		pine:  'text-rp-pine',
	};
</script>

<div class="rounded-lg bg-rp-surface border border-rp-hl-med p-4">
	<p class="text-xs text-rp-muted uppercase tracking-wider mb-1">{label}</p>
	<p class="text-2xl font-light {accentColors[accent]}">{value}</p>
	{#if sub}
		<p class="text-xs text-rp-subtle mt-1">{sub}</p>
	{/if}
</div>
