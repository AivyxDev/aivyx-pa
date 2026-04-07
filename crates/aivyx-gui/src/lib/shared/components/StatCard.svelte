<script lang="ts">
	let {
		label,
		value,
		icon = '',
		trend = '',
		trendColor = 'text-sage',
		valueColor = 'text-primary'
	}: {
		label: string;
		value: string;
		icon?: string;
		trend?: string;
		trendColor?: string;
		valueColor?: string;
	} = $props();
</script>

<div
	class="glass-panel p-5 rounded-xl ambient-glow relative overflow-hidden group animate-fade-in"
>
	<!-- Ghost icon background -->
	{#if icon}
		<div
			class="absolute top-0 right-0 p-2 opacity-10 group-hover:opacity-20 transition-opacity text-6xl"
		>
			<span class="material-symbols-outlined text-6xl">{icon}</span>
		</div>
	{/if}

	<p class="text-[10px] font-mono text-on-surface-variant/60 uppercase tracking-widest">
		{label}
	</p>
	<h3 class="text-3xl font-headline font-bold mt-2 {valueColor}">
		{value}
	</h3>

	{#if trend}
		<div class="mt-4 flex items-center gap-1 text-[10px] font-mono {trendColor}">
			{trend}
		</div>
	{/if}
</div>
