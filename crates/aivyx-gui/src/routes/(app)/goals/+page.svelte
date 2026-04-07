<script lang="ts">
	import StatusBadge from '$lib/shared/components/StatusBadge.svelte';

	let filter = $state('all');

	const goals = [
		{ title: 'Monitor inbox', description: 'Scan for urgent emails every heartbeat cycle', status: 'active', steps: '3/4', priority: 'high' },
		{ title: 'Daily standup prep', description: 'Summarize yesterday\'s work and today\'s blockers before 9am', status: 'active', steps: '2/3', priority: 'medium' },
		{ title: 'PR review tracking', description: 'Track open PRs older than 24h and surface them', status: 'active', steps: '1/2', priority: 'medium' },
		{ title: 'Weekly reflection', description: 'Generate a weekly summary every Friday at 5pm', status: 'active', steps: '0/1', priority: 'low' },
		{ title: 'Calendar sync', description: 'Cross-reference calendar events with task deadlines', status: 'completed', steps: '4/4', priority: 'high' },
		{ title: 'News digest', description: 'Compile AI/tech news digest from RSS feeds', status: 'abandoned', steps: '1/3', priority: 'low' }
	];

	const filtered = $derived(filter === 'all' ? goals : goals.filter(g => g.status === filter));
</script>

<svelte:head>
	<title>Goals — Aivyx PA</title>
</svelte:head>

<section class="mb-8">
	<div class="flex items-end justify-between">
		<div>
			<h1 class="text-3xl font-headline font-bold text-on-background tracking-tight">Goals</h1>
			<p class="text-on-surface-variant/40 text-sm mt-1">Brain goals that guide your assistant's autonomous behavior.</p>
		</div>
		<div class="flex gap-2">
			{#each ['all', 'active', 'completed', 'abandoned'] as f}
				<button
					class="px-3 py-1 rounded-lg text-xs font-mono uppercase tracking-wider transition-colors {filter === f ? 'bg-primary/10 text-primary' : 'text-on-surface-variant/40 hover:text-on-surface'}"
					onclick={() => (filter = f)}
				>{f}</button>
			{/each}
		</div>
	</div>
</section>

<div class="grid grid-cols-1 lg:grid-cols-2 gap-4 max-w-5xl">
	{#each filtered as goal}
		<div class="bg-surface-container-high/40 p-5 rounded-xl border border-outline-variant/15 hover:border-primary/20 transition-all group">
			<div class="flex items-start justify-between mb-3">
				<div>
					<h3 class="text-sm font-bold group-hover:text-primary transition-colors">{goal.title}</h3>
					<p class="text-xs text-on-surface-variant/40 mt-1">{goal.description}</p>
				</div>
				<StatusBadge status={goal.status} />
			</div>
			<div class="flex items-center justify-between mt-4">
				<span class="font-mono text-[10px] text-on-surface-variant/30 uppercase">Steps: {goal.steps}</span>
				<span class="font-mono text-[10px] uppercase px-2 py-0.5 rounded
					{goal.priority === 'high' ? 'text-error bg-error/10' :
					 goal.priority === 'medium' ? 'text-accent-glow bg-accent-glow/10' :
					 'text-on-surface-variant/40 bg-surface-container'}">
					{goal.priority}
				</span>
			</div>
			<!-- Progress bar -->
			{#if goal.status === 'active'}
				{@const [done, total] = goal.steps.split('/').map(Number)}
				<div class="mt-3 h-1 bg-surface-container rounded-full overflow-hidden">
					<div class="h-full bg-primary transition-all" style="width: {(done / total) * 100}%"></div>
				</div>
			{/if}
		</div>
	{/each}
</div>
