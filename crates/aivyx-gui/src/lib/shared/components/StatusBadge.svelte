<script lang="ts">
	let {
		status,
		class: className = ''
	}: {
		status: string;
		class?: string;
	} = $props();

	const colorMap: Record<string, string> = {
		'in progress': 'text-primary bg-primary/10',
		active: 'text-sage bg-sage/10',
		completed: 'text-sage bg-sage/10',
		staged: 'text-on-surface-variant bg-surface-container',
		failed: 'text-error bg-error/10',
		planning: 'text-secondary bg-secondary/10',
		executing: 'text-primary bg-primary/10',
		verifying: 'text-teal bg-teal/10',
		awaiting_approval: 'text-accent-glow bg-accent-glow/10',
		paused: 'text-on-surface-variant bg-surface-container',
		abandoned: 'text-error bg-error/10'
	};

	const colors = $derived(colorMap[status.toLowerCase()] ?? 'text-on-surface-variant bg-surface-container');
</script>

<span class="text-[10px] font-mono px-2 py-0.5 rounded uppercase {colors} {className}">
	{status}
</span>
