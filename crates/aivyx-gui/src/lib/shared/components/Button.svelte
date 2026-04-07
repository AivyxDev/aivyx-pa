<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		children,
		variant = 'primary',
		class: className = '',
		disabled = false,
		onclick
	}: {
		children: Snippet;
		variant?: 'primary' | 'secondary' | 'ghost';
		class?: string;
		disabled?: boolean;
		onclick?: () => void;
	} = $props();

	const variants = {
		primary:
			'bg-gradient-to-r from-primary to-primary-container text-on-primary font-headline font-bold glow-hover active:scale-[0.98]',
		secondary:
			'glass-panel text-on-surface border border-secondary/20 font-bold hover:bg-surface-bright',
		ghost:
			'text-on-secondary-container hover:text-secondary font-mono text-[11px] uppercase tracking-widest'
	};
</script>

<button
	class="inline-flex items-center justify-center gap-2 rounded-lg px-4 py-3 text-sm transition-all {variants[variant]} {className}"
	{disabled}
	{onclick}
>
	<!-- Glossy overlay for primary -->
	{#if variant === 'primary'}
		<div class="absolute inset-0 bg-gradient-to-tr from-white/10 to-transparent pointer-events-none rounded-lg"></div>
	{/if}
	<span class="relative z-10 flex items-center gap-2">
		{@render children()}
	</span>
</button>
