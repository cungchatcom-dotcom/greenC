/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{svelte,js,ts,jsx,tsx}'],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        canvas: 'rgb(var(--bg-canvas-rgb) / <alpha-value>)',
        card: 'rgb(var(--bg-card-rgb) / <alpha-value>)',
        subtle: 'rgb(var(--bg-subtle-rgb) / <alpha-value>)',
        border: 'var(--border-subtle)',
        primary: 'rgb(var(--text-primary-rgb) / <alpha-value>)',
        secondary: 'rgb(var(--text-secondary-rgb) / <alpha-value>)',
        disabled: 'var(--text-disabled)',
        accent: {
          DEFAULT: 'rgb(var(--accent-rgb) / <alpha-value>)',
          hover: 'rgb(var(--accent-hover-rgb) / <alpha-value>)',
          subtle: 'var(--accent-subtle)'
        },
        danger: 'rgb(var(--danger-rgb) / <alpha-value>)',
        warning: 'rgb(var(--warning-rgb) / <alpha-value>)'
      },
      fontFamily: {
        sans: ['"Segoe UI Variable Text"', '"Segoe UI"', 'system-ui', 'sans-serif'],
        mono: ['"Cascadia Code"', 'Consolas', 'monospace']
      }
    }
  },
  plugins: []
};
