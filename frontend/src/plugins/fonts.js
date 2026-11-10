// Same font families and weights as MoviePilot v3's webfontloader, without
// blocking application startup when the external font service is unavailable.
export function loadMoviePilotFonts() {
  if (typeof document === 'undefined' || document.getElementById('moviepilot-fonts')) return;
  const stylesheet = document.createElement('link');
  stylesheet.id = 'moviepilot-fonts';
  stylesheet.rel = 'stylesheet';
  stylesheet.href = 'https://fonts.googleapis.com/css2?family=Inter:wght@100;200;300;400;500;600;700&family=Noto+Sans+SC:wght@400;500;700&display=swap';
  document.head.appendChild(stylesheet);
}
