<svelte:head>
	<title>Memory — Aivyx PA</title>
</svelte:head>

<section class="mb-8">
	<div class="flex items-end justify-between">
		<div>
			<h1 class="text-3xl font-headline font-bold text-on-background tracking-tight">Memory</h1>
			<p class="text-on-surface-variant/40 text-sm mt-1">Knowledge graph and semantic memory browser.</p>
		</div>
		<div class="flex items-center gap-3 bg-surface-container-low px-4 py-1.5 rounded-full border border-outline-variant/15 w-72">
			<span class="material-symbols-outlined text-on-surface-variant/40 text-sm">search</span>
			<input class="bg-transparent border-none text-xs focus:ring-0 placeholder:text-on-surface-variant/30 w-full font-mono outline-none text-on-surface" placeholder="Query semantic triples..." type="text" />
		</div>
	</div>
</section>

<div class="grid grid-cols-12 gap-6">
	<!-- Graph view -->
	<div class="col-span-12 lg:col-span-8 bg-surface-container-low/50 rounded-2xl border border-outline-variant/15 relative overflow-hidden" style="min-height: 420px;">
		<div class="absolute top-4 left-4 flex items-center gap-2">
			<span class="text-[10px] font-mono text-sage bg-sage/10 px-2 py-0.5 rounded flex items-center gap-1">
				<span class="w-1.5 h-1.5 rounded-full bg-sage animate-pulse-glow"></span> LIVE SYNTHESIZING
			</span>
		</div>

		<!-- Placeholder graph -->
		<svg class="w-full h-full" viewBox="0 0 800 400" xmlns="http://www.w3.org/2000/svg">
			<!-- Edges -->
			<line x1="300" y1="120" x2="400" y2="210" stroke="var(--color-secondary)" stroke-opacity="0.2" stroke-width="1" stroke-dasharray="4,4" />
			<line x1="400" y1="210" x2="360" y2="310" stroke="var(--color-secondary)" stroke-opacity="0.2" stroke-width="1" stroke-dasharray="4,4" />
			<line x1="360" y1="310" x2="530" y2="340" stroke="var(--color-secondary)" stroke-opacity="0.2" stroke-width="1" stroke-dasharray="4,4" />
			<!-- Nodes -->
			<circle cx="300" cy="120" r="6" fill="var(--color-primary)" opacity="0.8" />
			<circle cx="400" cy="210" r="6" fill="var(--color-secondary)" opacity="0.8" />
			<circle cx="360" cy="310" r="6" fill="var(--color-primary)" opacity="0.6" />
			<circle cx="530" cy="340" r="6" fill="var(--color-secondary)" opacity="0.6" />
			<!-- Labels -->
			<text x="310" y="115" fill="var(--color-on-surface)" font-family="JetBrains Mono" font-size="10" opacity="0.6">ENTITY_ID::0019</text>
			<text x="345" y="200" fill="var(--color-secondary)" font-family="JetBrains Mono" font-size="9" opacity="0.5">DERIVED_FROM</text>
			<text x="370" y="305" fill="var(--color-on-surface)" font-family="JetBrains Mono" font-size="10" opacity="0.6">ACTIVE_PERSONA</text>
			<text x="430" y="345" fill="var(--color-secondary)" font-family="JetBrains Mono" font-size="9" opacity="0.5">ASSOCIATES_WITH</text>
		</svg>

		<!-- Stats bar -->
		<div class="absolute bottom-0 left-0 right-0 bg-surface-container-lowest/80 backdrop-blur-sm border-t border-outline-variant/15 px-6 py-3 flex items-center justify-between">
			<div class="flex gap-6">
				<span class="font-mono text-[10px] text-on-surface-variant/30">NODES: <strong class="text-on-surface">1,248</strong></span>
				<span class="font-mono text-[10px] text-on-surface-variant/30">TRIPLES: <strong class="text-on-surface">4,892</strong></span>
			</div>
			<span class="font-mono text-[10px] text-primary/40 italic">OPTIMIZING GRAPH TRAVERSAL...</span>
		</div>
	</div>

	<!-- Right sidebar -->
	<div class="col-span-12 lg:col-span-4 space-y-4">
		<!-- Agent info -->
		<div class="bg-surface-container-high p-5 rounded-xl border border-outline-variant/15">
			<div class="flex items-center justify-between mb-4">
				<div class="flex items-center gap-3">
					<div class="w-10 h-10 rounded-full bg-secondary-container flex items-center justify-center">
						<span class="material-symbols-outlined text-secondary text-lg">smart_toy</span>
					</div>
					<div>
						<h4 class="font-bold text-sm">Assistant</h4>
						<p class="font-mono text-[10px] text-secondary">SYNTHESIZER GRADE: S</p>
					</div>
				</div>
				<span class="font-mono text-[10px] text-on-surface-variant/30 bg-surface-container px-2 py-0.5 rounded">98.2% SYNC</span>
			</div>
			<div class="flex justify-between font-mono text-[10px] mb-2">
				<span class="text-on-surface-variant/40">MEMORY RETENTION</span>
				<span>842 / 2048</span>
			</div>
			<div class="h-1 bg-surface-container-lowest rounded-full overflow-hidden">
				<div class="h-full bg-primary w-[41%]"></div>
			</div>
		</div>

		<!-- Discovered Patterns -->
		<div class="bg-surface-container-high p-5 rounded-xl border border-outline-variant/15">
			<div class="flex items-center justify-between mb-4">
				<h4 class="font-headline font-bold text-sm flex items-center gap-2">
					<span class="material-symbols-outlined text-sm text-secondary">pattern</span>
					Discovered Patterns
				</h4>
				<span class="font-mono text-[10px] text-on-surface-variant/30">COUNT: 14</span>
			</div>
			<div class="space-y-4">
				{#each [
					{ name: 'TEMPORAL_LOOP_ALPHA', detail: 'Recurring semantic overlap detected in morning email patterns between 08:00-09:00', tags: ['High Confidence'], time: '2m ago' },
					{ name: 'CONTEXT_BRIDGE_V2', detail: 'Calendar events correlated with email deadlines at 85% accuracy', tags: ['Stable'], time: '1h ago' },
					{ name: 'PRIORITY_DRIFT', detail: 'User attention shifts from code to email after 3pm daily', tags: ['Anomalous'], time: '4h ago' }
				] as pattern}
					<div class="border-t border-outline-variant/15 pt-3 first:border-0 first:pt-0">
						<div class="flex justify-between items-start">
							<span class="font-mono text-[11px] text-primary font-bold">{pattern.name}</span>
							<span class="font-mono text-[9px] text-on-surface-variant/20">{pattern.time}</span>
						</div>
						<p class="text-[10px] text-on-surface-variant/40 mt-1 leading-relaxed">{pattern.detail}</p>
						<div class="flex gap-1 mt-2">
							{#each pattern.tags as tag}
								<span class="text-[9px] font-mono px-2 py-0.5 rounded border border-outline-variant/20 text-on-surface-variant/40">{tag}</span>
							{/each}
						</div>
					</div>
				{/each}
			</div>
		</div>
	</div>
</div>
