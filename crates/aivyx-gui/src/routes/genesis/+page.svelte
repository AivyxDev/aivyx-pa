<script lang="ts">
	import { goto } from '$app/navigation';
	import AmbientBackground from '$lib/shared/components/AmbientBackground.svelte';
	import CandleLogo from '$lib/shared/components/CandleLogo.svelte';
	import Button from '$lib/shared/components/Button.svelte';
	import Input from '$lib/shared/components/Input.svelte';

	let currentStep = $state(1);
	const totalSteps = 5;

	let name = $state('');
	let timezone = $state('');
	let provider = $state('Ollama');
	let model = $state('qwen3:14b');
	let passphrase = $state('');
	let passphraseConfirm = $state('');
	let persona = $state('assistant');

	function nextStep() {
		if (currentStep < totalSteps) currentStep++;
		else goto('/');
	}

	function prevStep() {
		if (currentStep > 1) currentStep--;
	}

	const stepLabels = ['Identity', 'Provider', 'Vault', 'Persona', 'Genesis'];
</script>

<svelte:head>
	<title>Genesis — Aivyx Setup</title>
</svelte:head>

<AmbientBackground />

<div class="relative z-10 min-h-screen flex flex-col">
	<!-- Top bar -->
	<header class="flex items-center justify-between px-8 py-5 border-b border-outline-variant/15">
		<div class="flex items-center gap-3">
			<CandleLogo size={28} />
			<span class="font-headline font-bold text-primary tracking-tight">AIVYX</span>
		</div>
		<span class="font-mono text-[10px] text-on-surface-variant/30 tracking-widest">GENESIS PROTOCOL 0.1.0</span>
	</header>

	<!-- Main content -->
	<main class="flex-1 flex items-center justify-center px-6">
		<div class="w-full max-w-2xl">
			<div class="bg-surface-container-low backdrop-blur-xl rounded-xl p-10 ambient-glow border border-outline-variant/10 relative overflow-hidden">
				<!-- Step indicator -->
				<div class="flex items-center gap-2 mb-3">
					<span class="font-mono text-sm text-primary font-bold">STEP {String(currentStep).padStart(2, '0')}</span>
					<span class="font-mono text-sm text-on-surface-variant/30">/ {String(totalSteps).padStart(2, '0')}</span>
					<span class="flex-1"></span>
					<span class="font-mono text-[10px] text-on-surface-variant/30 uppercase tracking-widest">{stepLabels[currentStep - 1]}</span>
				</div>

				<!-- Progress bar -->
				<div class="flex gap-1 mb-10">
					{#each Array(totalSteps) as _, i}
						<div
							class="h-0.5 flex-1 rounded-full transition-colors duration-300"
							class:bg-primary={i < currentStep}
							class:bg-surface-container-highest={i >= currentStep}
						></div>
					{/each}
				</div>

				<!-- Steps -->
				{#if currentStep === 1}
					<div class="animate-fade-in">
						<h2 class="text-4xl font-headline font-bold tracking-tight mb-2">Who are <em class="text-primary italic">you?</em></h2>
						<p class="text-on-surface-variant/60 mb-10">To tailor your intelligence mapping, we need to calibrate your local context. Define your signature and temporal anchor.</p>
						<Input label="Signature Identifier" placeholder="Enter your full name" bind:value={name} class="mb-8" />
						<Input label="Temporal Anchor (Timezone)" placeholder="Select your current coordinate" bind:value={timezone} />
					</div>
				{:else if currentStep === 2}
					<div class="animate-fade-in">
						<h2 class="text-4xl font-headline font-bold tracking-tight mb-2">Choose your <em class="text-primary italic">AI</em></h2>
						<p class="text-on-surface-variant/60 mb-10">Select your LLM provider and model. You can change this later in Settings.</p>
						<div class="space-y-6">
							<div class="space-y-2">
								<label class="font-mono text-[10px] uppercase tracking-widest text-on-surface-variant block">Provider</label>
								<select class="w-full bg-surface-container-lowest ring-1 ring-outline-variant/30 focus:ring-primary/50 rounded-lg py-4 px-5 font-mono text-on-surface outline-none" bind:value={provider}>
									<option>Ollama</option>
									<option>OpenAI</option>
									<option>Anthropic</option>
									<option>OpenRouter</option>
									<option>Google Gemini</option>
								</select>
							</div>
							<Input label="Model" placeholder="e.g., qwen3:14b" bind:value={model} />
						</div>
					</div>
				{:else if currentStep === 3}
					<div class="animate-fade-in">
						<h2 class="text-4xl font-headline font-bold tracking-tight mb-2">Secure your <em class="text-primary italic">vault</em></h2>
						<p class="text-on-surface-variant/60 mb-10">Your data will be encrypted with ChaCha20-Poly1305. This passphrase is the only key.</p>
						<Input label="Passphrase" type="password" placeholder="Enter a strong passphrase (8+ chars)" bind:value={passphrase} class="mb-6" />
						<Input label="Confirm Passphrase" type="password" placeholder="Confirm your passphrase" bind:value={passphraseConfirm} />
						<!-- Strength meter -->
						<div class="flex gap-1 mt-4">
							<div class="h-1 flex-1 rounded-full" class:bg-error={passphrase.length >= 1 && passphrase.length < 8} class:bg-accent-glow={passphrase.length >= 8 && passphrase.length < 12} class:bg-sage={passphrase.length >= 12} class:bg-surface-container-highest={passphrase.length === 0}></div>
							<div class="h-1 flex-1 rounded-full" class:bg-accent-glow={passphrase.length >= 8 && passphrase.length < 12} class:bg-sage={passphrase.length >= 12} class:bg-surface-container-highest={passphrase.length < 8}></div>
							<div class="h-1 flex-1 rounded-full" class:bg-sage={passphrase.length >= 12} class:bg-surface-container-highest={passphrase.length < 12}></div>
						</div>
						<p class="font-mono text-[10px] mt-2 text-on-surface-variant/40">
							{passphrase.length === 0 ? '' : passphrase.length < 8 ? 'Weak — 8 characters minimum' : passphrase.length < 12 ? 'Fair — consider a longer passphrase' : 'Strong'}
						</p>
					</div>
				{:else if currentStep === 4}
					<div class="animate-fade-in">
						<h2 class="text-4xl font-headline font-bold tracking-tight mb-2">Choose a <em class="text-primary italic">persona</em></h2>
						<p class="text-on-surface-variant/60 mb-10">How should your assistant think and communicate?</p>
						<div class="grid grid-cols-2 gap-3">
							{#each ['assistant', 'researcher', 'writer', 'developer'] as role}
								<button
									class="p-4 rounded-xl border text-left transition-all {persona === role ? 'border-primary bg-primary/10' : 'border-outline-variant/20 bg-surface-container-high hover:border-primary/40'}"
									onclick={() => (persona = role)}
								>
									<span class="text-sm font-bold capitalize">{role}</span>
									<p class="text-[10px] text-on-surface-variant/40 mt-1 font-mono">
										{role === 'assistant' ? 'General-purpose, balanced' :
										 role === 'researcher' ? 'Analytical, thorough' :
										 role === 'writer' ? 'Creative, articulate' : 'Technical, precise'}
									</p>
								</button>
							{/each}
						</div>
					</div>
				{:else if currentStep === 5}
					<div class="animate-fade-in text-center">
						<div class="mb-8">
							<CandleLogo size={72} animate={true} />
						</div>
						<h2 class="text-4xl font-headline font-bold tracking-tight mb-4">Light the <em class="text-primary italic">candle</em></h2>
						<p class="text-on-surface-variant/60 mb-8">Review your configuration and initialize your assistant.</p>
						<div class="bg-surface-container-high p-6 rounded-xl text-left font-mono text-[11px] space-y-3 mb-8">
							<div class="flex justify-between"><span class="text-on-surface-variant/40">NAME</span><span class="text-on-surface">{name || '—'}</span></div>
							<div class="flex justify-between"><span class="text-on-surface-variant/40">PROVIDER</span><span class="text-primary">{provider} · {model}</span></div>
							<div class="flex justify-between"><span class="text-on-surface-variant/40">PERSONA</span><span class="text-secondary capitalize">{persona}</span></div>
							<div class="flex justify-between"><span class="text-on-surface-variant/40">ENCRYPTION</span><span class="text-sage">ChaCha20-Poly1305</span></div>
						</div>
					</div>
				{/if}

				<!-- Navigation -->
				<div class="flex items-center gap-4 mt-10">
					<Button variant="primary" onclick={nextStep} class="px-8">
						{currentStep === totalSteps ? 'Initialize Genesis' : 'Continue'}
						<span class="material-symbols-outlined text-sm">arrow_forward</span>
					</Button>
					{#if currentStep > 1}
						<button class="font-mono text-[11px] uppercase tracking-widest text-on-surface-variant/40 hover:text-on-surface transition-colors flex items-center gap-2" onclick={prevStep}>
							<span class="material-symbols-outlined text-xs">arrow_back</span>
							Back
						</button>
					{/if}
				</div>

				<!-- Decorative corner element -->
				<div class="absolute bottom-4 right-4 w-16 h-16 border border-secondary/10 rounded-sm rotate-45 opacity-20 pointer-events-none">
					<div class="w-10 h-10 border border-secondary/15 rounded-sm m-auto mt-3"></div>
				</div>
			</div>
		</div>
	</main>

	<!-- Bottom status -->
	<footer class="flex items-center justify-center gap-8 py-4 border-t border-outline-variant/15">
		<span class="flex items-center gap-2 font-mono text-[10px] text-on-surface-variant/20">
			<span class="w-1.5 h-1.5 rounded-full bg-sage"></span> SECURE ENCRYPTION ACTIVE
		</span>
		<span class="font-mono text-[10px] text-on-surface-variant/20">● NODE: LOCAL</span>
		<span class="flex items-center gap-2 font-mono text-[10px] text-on-surface-variant/20">
			<span class="w-1.5 h-1.5 rounded-full bg-on-surface-variant/20"></span> SYSTEM HEALTH: 100%
		</span>
	</footer>
</div>
