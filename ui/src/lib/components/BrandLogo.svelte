<!-- ui/src/lib/components/BrandLogo.svelte: Drive Gauge + C brand mark -->
<script lang="ts">
  interface Props {
    size?: number;
    className?: string;
  }

  let { size = 28, className = '' }: Props = $props();
</script>

<svg
  xmlns="http://www.w3.org/2000/svg"
  width={size}
  height={size}
  viewBox="0 0 64 64"
  fill="none"
  class={className}
  aria-hidden="true"
>
  <defs>
    <linearGradient id="gc-bg" x1="6" y1="4" x2="58" y2="60" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#065F46" />
      <stop offset="100%" stop-color="#022C22" />
    </linearGradient>
    <linearGradient id="gc-arc" x1="14" y1="14" x2="52" y2="50" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#6EE7B7" />
      <stop offset="55%" stop-color="#10B981" />
      <stop offset="100%" stop-color="#059669" />
    </linearGradient>
  </defs>
  <!-- Squircle shell -->
  <rect x="2" y="2" width="60" height="60" rx="16" fill="url(#gc-bg)" />
  <rect x="2.75" y="2.75" width="58.5" height="58.5" rx="15.25" stroke="#34D399" stroke-opacity="0.28" stroke-width="1.5" />
  <!-- Subtle full drive track -->
  <circle cx="32" cy="32" r="18" stroke="#FFFFFF" stroke-opacity="0.14" stroke-width="6.5" />
  <!-- Drive Gauge arc forming open letter C -->
  <path
    d="M44.73 19.27A18 18 0 1 0 44.73 44.73"
    stroke="url(#gc-arc)"
    stroke-width="6.5"
    stroke-linecap="round"
  />
  <!-- Target quota tick at top-right -->
  <line x1="45" y1="19" x2="49.5" y2="14.5" stroke="#A7F3D0" stroke-width="2.6" stroke-linecap="round" />
  <!-- Center healthy green core + gauge needle -->
  <path d="M32 32L41.5 23.5" stroke="#D1FAE5" stroke-width="2.8" stroke-linecap="round" />
  <circle cx="32" cy="32" r="4.2" fill="#34D399" />
  <circle cx="32" cy="32" r="1.8" fill="#ECFDF5" />
</svg>
