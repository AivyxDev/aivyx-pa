<svelte:head>
	<title>Activity — Aivyx PA</title>
</svelte:head>

<section class="mb-8">
	<h1 class="text-3xl font-headline font-bold text-on-background tracking-tight">Activity</h1>
	<p class="text-on-surface-variant/40 text-sm mt-1">Background loop activity and heartbeat suggestions.</p>
</section>

<!-- Activity timeline -->
<div class="space-y-6 relative before:content-[''] before:absolute before:left-[11px] before:top-6 before:bottom-0 before:w-px before:bg-on-surface/5 max-w-3xl">
	{#each [
		{ time: '09:45:02', title: 'Heartbeat cycle #142', detail: 'Scanned calendar, email, and task queues. Generated 2 suggestions.', type: 'heartbeat', icon: 'favorite' },
		{ time: '09:44:30', title: 'Suggestion: Block focus time', detail: 'Calendar shows no meetings 2-4pm. Recommend blocking for deep work on PR reviews.', type: 'suggestion', icon: 'lightbulb' },
		{ time: '09:43:12', title: 'Email scan completed', detail: '18 new emails. 3 classified urgent, 12 informational, 3 promotions archived.', type: 'tool', icon: 'email' },
		{ time: '09:30:00', title: 'Heartbeat cycle #141', detail: 'Context fusion: correlated 2 calendar events with pending emails.', type: 'heartbeat', icon: 'favorite' },
		{ time: '09:15:00', title: 'Goal progress update', detail: 'Goal "Monitor inbox" advanced to step 3/4. Auto-promoted to T3.', type: 'goal', icon: 'flag' },
		{ time: '09:00:00', title: 'System startup', detail: 'Engine initialized. Heartbeat interval: 30m. Active goals: 5.', type: 'system', icon: 'power_settings_new' }
	] as event}
		<div class="relative pl-10 animate-fade-in">
			<div class="absolute left-0 top-1.5 w-[22px] h-[22px] rounded-full border-4 border-surface flex items-center justify-center
				{event.type === 'heartbeat' ? 'bg-sage/20' :
				 event.type === 'suggestion' ? 'bg-accent-glow/20' :
				 event.type === 'tool' ? 'bg-primary/20' :
				 event.type === 'goal' ? 'bg-secondary/20' :
				 'bg-surface-variant/50'}">
				<div class="w-1.5 h-1.5 rounded-full
					{event.type === 'heartbeat' ? 'bg-sage' :
					 event.type === 'suggestion' ? 'bg-accent-glow' :
					 event.type === 'tool' ? 'bg-primary' :
					 event.type === 'goal' ? 'bg-secondary' :
					 'bg-on-surface-variant/40'}"></div>
			</div>
			<p class="text-[11px] font-mono text-on-surface-variant/30">{event.time}</p>
			<div class="flex items-center gap-2 mt-0.5">
				<span class="material-symbols-outlined text-sm text-on-surface-variant/40">{event.icon}</span>
				<h5 class="text-sm font-bold">{event.title}</h5>
			</div>
			<p class="text-[11px] text-on-surface-variant/40 mt-1">{event.detail}</p>
		</div>
	{/each}
</div>
