<script lang="ts">
	import StatusBadge from '$lib/shared/components/StatusBadge.svelte';
</script>

<svelte:head>
	<title>Missions — Aivyx PA</title>
</svelte:head>

<section class="mb-8">
	<div class="flex items-end justify-between">
		<div>
			<h1 class="text-3xl font-headline font-bold text-on-background tracking-tight">Missions</h1>
			<p class="text-on-surface-variant/40 text-sm mt-1">Multi-step mission orchestration with autonomous planning.</p>
		</div>
		<button class="flex items-center gap-2 px-4 py-2 bg-gradient-to-r from-primary to-primary-container text-on-primary font-bold text-sm rounded-lg hover:scale-[0.98] transition-transform relative overflow-hidden">
			<div class="absolute inset-0 bg-gradient-to-tr from-white/10 to-transparent pointer-events-none"></div>
			<span class="material-symbols-outlined text-sm relative">rocket_launch</span>
			<span class="relative">New Mission</span>
		</button>
	</div>
</section>

<div class="space-y-4 max-w-4xl">
	{#each [
		{ title: 'Research AI agent frameworks', objective: 'Compare top 5 agent frameworks and produce a recommendation report', status: 'executing', progress: 65, steps: ['Gather candidates', 'Read documentation', 'Compare features', 'Write report'], currentStep: 2 },
		{ title: 'Organize photo library', objective: 'Sort ~/Photos by date, deduplicate, and generate thumbnails', status: 'planning', progress: 12, steps: ['Scan directory', 'Identify duplicates', 'Sort by date', 'Generate thumbnails'], currentStep: 0 },
		{ title: 'Weekly email digest', objective: 'Compile a summary of this week\'s important emails', status: 'completed', progress: 100, steps: ['Fetch emails', 'Classify importance', 'Summarize', 'Format digest'], currentStep: 4 }
	] as mission}
		<div class="bg-surface-container-low/50 p-6 rounded-2xl border border-outline-variant/15 backdrop-blur-sm hover:border-primary/20 transition-all">
			<div class="flex items-start justify-between mb-4">
				<div class="flex gap-4">
					<div class="w-10 h-10 bg-surface-container-highest rounded-lg flex items-center justify-center border border-outline-variant/15">
						<span class="material-symbols-outlined text-{mission.status === 'completed' ? 'sage' : mission.status === 'executing' ? 'primary' : 'secondary'}">
							{mission.status === 'completed' ? 'check_circle' : mission.status === 'executing' ? 'monitoring' : 'edit_note'}
						</span>
					</div>
					<div>
						<h3 class="text-sm font-bold">{mission.title}</h3>
						<p class="text-xs text-on-surface-variant/40 mt-0.5">{mission.objective}</p>
					</div>
				</div>
				<StatusBadge status={mission.status} />
			</div>

			<!-- Steps -->
			<div class="ml-14 mb-4 space-y-2">
				{#each mission.steps as step, i}
					<div class="flex items-center gap-2 text-xs">
						<span class="material-symbols-outlined text-sm
							{i < mission.currentStep ? 'text-sage' :
							 i === mission.currentStep ? 'text-primary' :
							 'text-on-surface-variant/20'}">
							{i < mission.currentStep ? 'check_circle' :
							 i === mission.currentStep ? 'pending' : 'radio_button_unchecked'}
						</span>
						<span class="{i < mission.currentStep ? 'text-on-surface-variant/40 line-through' :
							i === mission.currentStep ? 'text-on-surface font-bold' :
							'text-on-surface-variant/30'}">{step}</span>
					</div>
				{/each}
			</div>

			<!-- Progress bar -->
			<div class="flex items-center gap-4">
				<div class="flex-1 h-1.5 bg-surface-container rounded-full overflow-hidden">
					<div class="h-full bg-primary transition-all shadow-[0_0_10px_rgba(255,183,125,0.4)]" style="width: {mission.progress}%"></div>
				</div>
				<span class="text-[10px] font-mono text-on-surface-variant/40">{mission.progress}%</span>
			</div>
		</div>
	{/each}
</div>
