<script lang="ts">
	import Sidebar from '$lib/pa/components/layout/Sidebar.svelte';
	import TopBar from '$lib/pa/components/layout/TopBar.svelte';
	import StatusBar from '$lib/pa/components/layout/StatusBar.svelte';

	let { children } = $props();
</script>

<div class="min-h-screen bg-background">
	<Sidebar />
	<TopBar />
	<main class="ml-[220px] mt-16 p-8 h-[calc(100vh-64px-32px)] overflow-y-auto relative scroll-smooth bg-gradient-to-br from-surface to-surface-container-lowest">
		{@render children()}
	</main>
	<StatusBar />
</div>
