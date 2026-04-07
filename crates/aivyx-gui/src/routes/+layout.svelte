<script lang="ts">
	import './layout.css';
	import favicon from '$lib/assets/favicon.svg';
	import { theme } from '$lib/shared/stores/theme';
	import { startDashboardPolling } from '$lib/shared/stores/dashboard';
	import { startNotificationStream } from '$lib/shared/stores/notifications';
	import { onMount, onDestroy } from 'svelte';

	let { children } = $props();

	let stopDashboard: (() => void) | null = null;
	let stopNotifications: (() => void) | null = null;

	onMount(() => {
		// Apply stored theme
		const stored = localStorage.getItem('aivyx-theme') as 'dark' | 'light' | null;
		const t = stored ?? 'dark';
		document.documentElement.classList.remove('dark', 'light');
		document.documentElement.classList.add(t);
		theme.set(t);

		// Start live data feeds
		stopDashboard = startDashboardPolling(30_000);
		stopNotifications = startNotificationStream();
	});

	onDestroy(() => {
		stopDashboard?.();
		stopNotifications?.();
	});
</script>

<svelte:head><link rel="icon" href={favicon} /></svelte:head>
{@render children()}
