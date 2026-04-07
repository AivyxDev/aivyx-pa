<script lang="ts">
	import { theme } from '$lib/shared/stores/theme';

	let provider = $state('Ollama');
	let model = $state('qwen3:14b');
	let autonomy = $state('trust');
	let heartbeatInterval = $state('30');
	let emailEnabled = $state(true);
</script>

<svelte:head>
	<title>Settings — Aivyx PA</title>
</svelte:head>

<section class="mb-8">
	<h1 class="text-3xl font-headline font-bold text-on-background tracking-tight">Settings</h1>
	<p class="text-on-surface-variant/40 text-sm mt-1">Configure your personal assistant.</p>
</section>

<div class="space-y-6 max-w-3xl">
	<!-- LLM Provider -->
	<div class="bg-surface-container-high/40 p-6 rounded-xl border border-outline-variant/15">
		<h3 class="font-headline font-bold text-sm tracking-wide mb-4 flex items-center gap-2">
			<span class="material-symbols-outlined text-sm text-primary">psychology</span>
			LLM PROVIDER
		</h3>
		<div class="grid grid-cols-2 gap-4">
			<div class="space-y-2">
				<label class="font-mono text-[10px] uppercase tracking-widest text-on-surface-variant/40 block">Provider</label>
				<select class="w-full bg-surface-container-lowest ring-1 ring-outline-variant/30 focus:ring-primary/50 rounded-lg py-3 px-4 font-mono text-sm text-on-surface outline-none" bind:value={provider}>
					<option>Ollama</option>
					<option>OpenAI</option>
					<option>Anthropic</option>
					<option>OpenRouter</option>
					<option>Google Gemini</option>
				</select>
			</div>
			<div class="space-y-2">
				<label class="font-mono text-[10px] uppercase tracking-widest text-on-surface-variant/40 block">Model</label>
				<input class="w-full bg-surface-container-lowest ring-1 ring-outline-variant/30 focus:ring-primary/50 rounded-lg py-3 px-4 font-mono text-sm text-on-surface outline-none" bind:value={model} />
			</div>
		</div>
	</div>

	<!-- Autonomy -->
	<div class="bg-surface-container-high/40 p-6 rounded-xl border border-outline-variant/15">
		<h3 class="font-headline font-bold text-sm tracking-wide mb-4 flex items-center gap-2">
			<span class="material-symbols-outlined text-sm text-secondary">shield</span>
			AUTONOMY TIER
		</h3>
		<div class="space-y-3">
			{#each [
				{ id: 'locked', label: 'Locked (T1)', desc: 'All actions require approval' },
				{ id: 'cautious', label: 'Cautious (T2)', desc: 'Low-risk actions auto-approved' },
				{ id: 'trust', label: 'Trust (T3)', desc: 'Most actions auto-approved, high-risk flagged' },
				{ id: 'autonomous', label: 'Autonomous (T4)', desc: 'Full autonomy, audit-only oversight' }
			] as tier}
				<button
					class="w-full text-left p-3 rounded-lg border transition-all
						{autonomy === tier.id ? 'border-primary/40 bg-primary/5' : 'border-outline-variant/10 hover:border-primary/20'}"
					onclick={() => (autonomy = tier.id)}
				>
					<div class="flex items-center justify-between">
						<span class="text-sm font-bold">{tier.label}</span>
						{#if autonomy === tier.id}
							<span class="material-symbols-outlined text-primary text-sm">check_circle</span>
						{/if}
					</div>
					<p class="text-[10px] text-on-surface-variant/40 mt-0.5 font-mono">{tier.desc}</p>
				</button>
			{/each}
		</div>
	</div>

	<!-- Heartbeat -->
	<div class="bg-surface-container-high/40 p-6 rounded-xl border border-outline-variant/15">
		<h3 class="font-headline font-bold text-sm tracking-wide mb-4 flex items-center gap-2">
			<span class="material-symbols-outlined text-sm text-sage">favorite</span>
			HEARTBEAT
		</h3>
		<div class="space-y-2">
			<label class="font-mono text-[10px] uppercase tracking-widest text-on-surface-variant/40 block">Interval (minutes)</label>
			<input type="number" class="w-32 bg-surface-container-lowest ring-1 ring-outline-variant/30 focus:ring-primary/50 rounded-lg py-3 px-4 font-mono text-sm text-on-surface outline-none" bind:value={heartbeatInterval} min="5" max="120" />
		</div>
	</div>

	<!-- Appearance -->
	<div class="bg-surface-container-high/40 p-6 rounded-xl border border-outline-variant/15">
		<h3 class="font-headline font-bold text-sm tracking-wide mb-4 flex items-center gap-2">
			<span class="material-symbols-outlined text-sm text-accent-glow">palette</span>
			APPEARANCE
		</h3>
		<div class="flex items-center justify-between">
			<div>
				<p class="text-sm font-bold">Theme</p>
				<p class="text-[10px] text-on-surface-variant/40 font-mono mt-0.5">Switch between dark and light mode</p>
			</div>
			<button
				class="px-4 py-2 rounded-lg border border-outline-variant/20 font-mono text-xs hover:border-primary/40 transition-colors flex items-center gap-2"
				onclick={() => theme.toggle()}
			>
				<span class="material-symbols-outlined text-sm">
					{$theme === 'dark' ? 'dark_mode' : 'light_mode'}
				</span>
				{$theme === 'dark' ? 'Dark' : 'Light'}
			</button>
		</div>
	</div>
</div>
