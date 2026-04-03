/**
 * Terminal-inspired language color map with enough hue separation.
 *
 * Low saturation (terminal feel) but distinct hue angles so categories
 * are instantly recognizable. Like colored output in a terminal — not
 * bright, but you can always tell green from yellow from blue.
 *
 *   Systems   → teal/cyan family  (cool, compiled)
 *   Managed   → amber/sand family (warm, runtime)
 *   Scripting → lilac/mauve       (distinct third axis)
 *   Static    → neutral gray      (baseline reference)
 */

const LANGUAGE_COLORS: Record<string, string> = {
  // Systems — teal, enough saturation to read as "cool"
  rust:              '#5b9e97', // teal
  go:                '#6a9ab8', // steel blue
  cpp:               '#7b94af', // slate blue

  // Managed — amber/sand, clearly "warm" against the dark bg
  'csharp-net48':    '#c49a4a', // warm gold — legacy
  'csharp-net6':     '#b08e52', // sand
  'csharp-net7':     '#a6865a', // khaki
  'csharp-net8':     '#be9648', // amber
  'csharp-net8-aot': '#aa8840', // bronze
  'csharp-net9':     '#b89250', // wheat
  'csharp-net9-aot': '#9e8038', // ochre
  'csharp-net10':    '#c8a04e', // gold-light
  'csharp-net10-aot':'#b49244', // gold-dark
  java:              '#8a7eb5', // muted indigo — JVM, distinct from .NET

  // Scripting — lilac/mauve, a third hue axis
  nodejs:            '#a67a9e', // mauve
  python:            '#9484ac', // periwinkle
  ruby:              '#a87888', // dusty rose
  php:               '#8c7e9e', // plum

  // Static — true neutral, recedes as baseline
  nginx:             '#6b7280', // gray-500 — clearly the reference line
};

// Fallback for unknown languages
const FALLBACK_COLORS = [
  '#6b7280', // gray-500
  '#78716c', // stone-500
  '#71717a', // zinc-500
  '#737373', // neutral-500
];

/**
 * Get the color for a language. Returns a deterministic color
 * based on the language name, falling back to neutral grays.
 */
export function languageColor(lang: string): string {
  return LANGUAGE_COLORS[lang] ?? FALLBACK_COLORS[
    Math.abs([...lang].reduce((h, c) => (h * 31 + c.charCodeAt(0)) | 0, 0)) % FALLBACK_COLORS.length
  ];
}

/**
 * Get colors for an ordered list of languages.
 * Use this when you need index-based access (e.g., chart series).
 */
export function languageColors(languages: string[]): string[] {
  return languages.map(languageColor);
}
