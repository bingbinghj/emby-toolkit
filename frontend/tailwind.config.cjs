// MoviePilot-Frontend v3 configuration, adapted to this frontend (MIT).
module.exports = {
  content: ['./index.html', './src/**/*.{vue,js,ts,jsx,tsx}'],
  theme: { extend: {} },
  future: { hoverOnlyWhenSupported: true },
  corePlugins: { aspectRatio: false },
  plugins: [require('@tailwindcss/aspect-ratio')],
}
