<script lang="ts">
	// Imports moved into Settings → Data. This stays so old links and
	// bookmarks to /import land there instead of on a 404.
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';

	onMount(() => goto('/settings#data', { replaceState: true }));
</script>
