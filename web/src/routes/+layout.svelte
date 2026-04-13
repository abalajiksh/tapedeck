<script lang="ts">
	import '../app.css';
	import Nav from '$lib/components/Nav.svelte';
</script>

<div class="flex min-h-screen">
	<Nav />
	<main class="flex-1 overflow-y-auto">
		<slot />
	</main>
</div>
