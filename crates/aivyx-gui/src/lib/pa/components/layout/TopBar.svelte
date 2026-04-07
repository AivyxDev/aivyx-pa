<script lang="ts">
	import { theme } from '$lib/shared/stores/theme';

	let { title = 'Home' }: { title?: string } = $props();
</script>

<header
	class="fixed top-0 right-0 left-[220px] h-16 bg-background/80 backdrop-blur-xl flex items-center justify-between px-8 z-40 border-b border-outline-variant/15"
>
	<div class="flex items-center gap-8">
		<!-- Search -->
		<div
			class="flex items-center bg-surface-container-low px-4 py-1.5 rounded-full border border-outline-variant/15 w-80"
		>
			<span class="material-symbols-outlined text-on-surface-variant/40 text-sm mr-2">search</span>
			<input
				class="bg-transparent border-none text-xs focus:ring-0 placeholder:text-on-surface-variant/30 w-full font-mono outline-none text-on-surface"
				placeholder="Search or type a command..."
				type="text"
			/>
			<span
				class="text-[10px] text-on-surface-variant/30 font-mono bg-surface-container-high px-1.5 rounded"
				>⌘K</span
			>
		</div>

		<!-- Breadcrumb -->
		<nav class="hidden lg:flex items-center gap-2">
			<span class="text-sm text-primary font-bold">{title}</span>
		</nav>
	</div>

	<div class="flex items-center gap-4">
		<!-- Theme toggle -->
		<button
			class="w-10 h-10 flex items-center justify-center text-on-surface-variant/60 hover:text-primary transition-colors"
			onclick={() => theme.toggle()}
		>
			<span class="material-symbols-outlined text-xl">
				{$theme === 'dark' ? 'light_mode' : 'dark_mode'}
			</span>
		</button>

		<!-- Notifications -->
		<button
			class="w-10 h-10 flex items-center justify-center text-on-surface-variant/60 hover:text-primary transition-colors relative"
		>
			<span class="material-symbols-outlined">notifications</span>
			<span
				class="absolute top-2 right-2 w-2 h-2 bg-primary rounded-full ring-2 ring-background"
			></span>
		</button>

		<div class="h-6 w-px bg-on-surface/10 mx-2"></div>

		<!-- Version badge -->
		<span
			class="text-[10px] font-mono text-secondary px-2 py-0.5 bg-secondary-container rounded tracking-tighter"
		>
			PA v0.1.0
		</span>
	</div>
</header>
