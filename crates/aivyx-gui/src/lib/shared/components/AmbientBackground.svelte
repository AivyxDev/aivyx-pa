<!-- Ambient background layers used on unlock/genesis screens -->
<div class="fixed inset-0 z-0 overflow-hidden pointer-events-none">
	<!-- Noise texture -->
	<div class="absolute inset-0 noise-bg"></div>

	<!-- Purple blur orb -->
	<div class="absolute top-[-10%] left-[-10%] w-[40%] h-[40%] bg-secondary/10 blur-[120px] rounded-full"></div>

	<!-- Amber blur orb -->
	<div class="absolute bottom-[-10%] right-[-10%] w-[50%] h-[50%] bg-primary/5 blur-[150px] rounded-full"></div>

	<!-- Abstract vector lines -->
	<svg class="absolute inset-0 w-full h-full opacity-10" xmlns="http://www.w3.org/2000/svg">
		<path class="text-secondary" d="M-100 200 L400 600 L1200 400" fill="none" stroke="currentColor" stroke-width="0.5" />
		<path class="text-primary" d="M200 -100 L600 400 L400 1200" fill="none" stroke="currentColor" stroke-width="0.5" />
	</svg>
</div>
