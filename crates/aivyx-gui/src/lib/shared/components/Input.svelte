<script lang="ts">
	let {
		label = '',
		type = 'text',
		placeholder = '',
		value = $bindable(''),
		class: className = '',
		hint = ''
	}: {
		label?: string;
		type?: string;
		placeholder?: string;
		value?: string;
		class?: string;
		hint?: string;
	} = $props();
</script>

<div class="space-y-2 {className}">
	{#if label}
		<label class="font-mono text-[10px] uppercase tracking-widest text-on-surface-variant block">
			{label}
		</label>
	{/if}
	<input
		class="w-full bg-surface-container-lowest border-none ring-1 ring-outline-variant/30 focus:ring-primary/50 rounded-lg py-4 px-5 font-mono text-on-surface placeholder:text-on-surface-variant/30 transition-all outline-none"
		{type}
		{placeholder}
		bind:value
	/>
	{#if hint}
		<p class="font-mono text-[10px] text-primary/60">{hint}</p>
	{/if}
</div>
