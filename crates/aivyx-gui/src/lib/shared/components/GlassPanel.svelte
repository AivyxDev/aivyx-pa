<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		children,
		class: className = '',
		rounded = 'xl',
		padding = '5',
		glow = false
	}: {
		children: Snippet;
		class?: string;
		rounded?: string;
		padding?: string;
		glow?: boolean;
	} = $props();
</script>

<div
	class="glass-panel rounded-{rounded} p-{padding} relative overflow-hidden {glow
		? 'ambient-glow'
		: ''} {className}"
>
	<!-- Internal gradient overlay -->
	<div
		class="absolute inset-0 bg-gradient-to-br from-surface-container-highest/10 to-transparent pointer-events-none"
	></div>
	<div class="relative z-10">
		{@render children()}
	</div>
</div>
