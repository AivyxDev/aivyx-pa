<svelte:head>
	<title>Approvals — Aivyx PA</title>
</svelte:head>

<section class="mb-8">
	<h1 class="text-3xl font-headline font-bold text-on-background tracking-tight">Approvals</h1>
	<p class="text-on-surface-variant/40 text-sm mt-1">Pending actions that require your approval before execution.</p>
</section>

<div class="space-y-4 max-w-3xl">
	{#each [
		{ title: 'Send reply to Design Team', detail: 'Re: Meeting reschedule — Draft response confirms new 3pm slot', tool: 'email_send', risk: 'low', time: '2m ago' },
		{ title: 'Execute file cleanup script', detail: 'Remove 23 temp files from ~/Downloads older than 30 days', tool: 'file_delete', risk: 'medium', time: '15m ago' },
		{ title: 'Post standup summary to Slack', detail: 'Channel: #daily-standup — 3 items completed, 2 in progress', tool: 'slack_post', risk: 'low', time: '28m ago' }
	] as approval}
		<div class="bg-surface-container-high/40 p-6 rounded-xl border border-accent-glow/20 hover:border-accent-glow/40 transition-all">
			<div class="flex items-start justify-between mb-4">
				<div class="flex gap-4">
					<div class="w-10 h-10 bg-accent-glow/10 rounded-lg flex items-center justify-center border border-accent-glow/20">
						<span class="material-symbols-outlined text-accent-glow">approval</span>
					</div>
					<div>
						<h3 class="text-sm font-bold">{approval.title}</h3>
						<p class="text-xs text-on-surface-variant/40 mt-0.5">{approval.detail}</p>
					</div>
				</div>
				<span class="text-[10px] font-mono text-on-surface-variant/30">{approval.time}</span>
			</div>
			<div class="flex items-center justify-between">
				<div class="flex items-center gap-3">
					<span class="font-mono text-[10px] text-on-surface-variant/30 bg-surface-container px-2 py-0.5 rounded uppercase">{approval.tool}</span>
					<span class="font-mono text-[10px] uppercase px-2 py-0.5 rounded
						{approval.risk === 'low' ? 'text-sage bg-sage/10' : 'text-accent-glow bg-accent-glow/10'}">
						Risk: {approval.risk}
					</span>
				</div>
				<div class="flex gap-2">
					<button class="px-4 py-1.5 bg-sage/20 text-sage text-xs font-bold rounded-lg hover:bg-sage/30 transition-colors flex items-center gap-1">
						<span class="material-symbols-outlined text-sm">check</span> Approve
					</button>
					<button class="px-4 py-1.5 bg-error/10 text-error text-xs font-bold rounded-lg hover:bg-error/20 transition-colors flex items-center gap-1">
						<span class="material-symbols-outlined text-sm">close</span> Deny
					</button>
				</div>
			</div>
		</div>
	{/each}

	<!-- Empty state indicator -->
	{#if false}
		<div class="text-center py-16">
			<span class="material-symbols-outlined text-4xl text-on-surface-variant/20 mb-4">task_alt</span>
			<p class="text-on-surface-variant/40 font-mono text-sm">No pending approvals</p>
		</div>
	{/if}
</div>
