---
name: LagHound
description: Terminal-grade network diagnostics — dark glass, phosphor accents, monospace confidence
colors:
  brand-violet: "#863bff"
  phosphor-cyan: "#47bfff"
  interactive-cyan: "oklch(0.609 0.126 221.723)"
  interactive-cyan-hover: "oklch(0.655 0.127 221.723)"
  bg-base: "#0a0b0f"
  bg-surface: "#0d0e14"
  bg-sidebar: "#0f1015"
  bg-raised: "#12131a"
  border-default: "#1a1b25"
  border-strong: "#374151"
  text-primary: "#e5e7eb"
  text-muted: "#9ca3af"
  text-faint: "#6b7280"
  text-placeholder: "#4b5563"
  status-success: "#4ade80"
  status-failure: "#f87171"
  status-attention: "#fbbf24"
typography:
  headline:
    fontFamily: "'Cascadia Code', 'JetBrains Mono', ui-monospace, Consolas, monospace"
    fontSize: "1.25rem"
    fontWeight: 700
    lineHeight: 1.3
  body:
    fontFamily: "'Cascadia Code', 'JetBrains Mono', ui-monospace, Consolas, monospace"
    fontSize: "0.875rem"
    fontWeight: 400
    lineHeight: 1.5
  data:
    fontFamily: "'Cascadia Code', 'JetBrains Mono', ui-monospace, Consolas, monospace"
    fontSize: "0.75rem"
    fontWeight: 400
    lineHeight: 1.4
  label:
    fontFamily: "'Cascadia Code', 'JetBrains Mono', ui-monospace, Consolas, monospace"
    fontSize: "0.75rem"
    fontWeight: 500
    letterSpacing: "0.05em"
rounded:
  sm: "4px"
  md: "6px"
  lg: "8px"
  full: "9999px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "16px"
  lg: "24px"
components:
  button-primary:
    backgroundColor: "{colors.interactive-cyan}"
    textColor: "#ffffff"
    rounded: "{rounded.sm}"
    padding: "8px 16px"
    typography: "{typography.body}"
  button-primary-hover:
    backgroundColor: "{colors.interactive-cyan-hover}"
  input:
    backgroundColor: "{colors.bg-base}"
    textColor: "{colors.text-primary}"
    rounded: "{rounded.sm}"
    padding: "6px 12px"
    typography: "{typography.body}"
  card:
    backgroundColor: "{colors.bg-base}"
    rounded: "{rounded.md}"
---

# Design System: LagHound

## Overview

**Creative North Star: "The Signal Terminal"**

LagHound's interface is a premium terminal where the data itself is the light
source. Everything is dark glass — near-black layered surfaces (#0a0b0f base)
with thin borders instead of shadows — and the only things that glow are
signals: phosphor-cyan interactions, green/red/amber status lamps, white-hot
numbers. Monospace type everywhere gives the whole product the confidence of a
tool built by engineers for engineers; nothing is decorated, everything is
calibrated.

Density is a feature. The workhorse type size is 12px monospace; tables,
badges, and labels pack per-attempt phase data into scannable grids. Controls
behave like **tactile instrument keys**: crisp 4px-radius edges, a 1px press on
click, a border-color shift on focus — immediate, physical, quiet. Confirmed
anti-reference: generic SaaS dashboards with gradient heroes, decorative
spacing, and low information density.

**Key Characteristics:**
- Dark glass surfaces, flat: depth from four background steps + 1px borders, never shadows
- Monospace-first at every level, 12px data type as the default voice
- One interactive accent (Phosphor Cyan); brand violet reserved for the mark
- Semantically fixed status colors, identical everywhere
- Tactile instrument-key controls: press feedback, focus border shifts, 150–200ms motion

## Colors

A near-black neutral stack lit by one phosphor accent and a fixed status lamp
ramp.

### Primary
- **Phosphor Cyan** (#47bfff): the single interactive accent — links, active
  nav, focus states, in-flight status, primary data highlights. Its Tailwind
  working ramp is cyan-300/400 for text on dark, with **Interactive Cyan**
  (oklch(0.609 0.126 221.723), cyan-600) as the button fill and its hover step
  one notch lighter.

### Secondary
- **Brand Violet** (#863bff): the logo/wordmark identity color. Appears in the
  mark, the report branding, and nowhere else.

### Neutral
- **Terminal Black** (#0a0b0f): page background and input wells.
- **Surface** (#0d0e14): table headers, tooltips.
- **Sidebar** (#0f1015): navigation rail.
- **Raised** (#12131a): panel inputs, subtle elevation step.
- **Hairline** (#1a1b25): default borders, table rules, dividers.
- **Strong Border** (#374151, gray-700): input strokes, emphasized edges.
- Text ramp: **Primary** #e5e7eb (gray-200) → **Muted** #9ca3af (gray-400,
  the labels/metadata workhorse, ~7:1 on base) → **Faint** #6b7280 (gray-500,
  4.07:1 — de-emphasis floor for meaningful text) → **Placeholder** #4b5563
  (placeholders and disabled states only).

### Status
- **Success Green** (#4ade80), **Failure Red** (#f87171), **Attention Amber**
  (#fbbf24), **In-flight Cyan** (phosphor ramp), **Inert Grey** (muted ramp).

### Named Rules
**The Logo-Only Violet Rule.** Brand Violet appears only in the mark and brand
assets — never on statuses, KPIs, buttons, or charts. (Enforced since audit
F12; the comment lives in `index.css`.)

**The One Voice Rule.** Phosphor Cyan is the only interactive accent. If a
second accent seems needed, the answer is hierarchy, not a new color.

**The Status Semantics Rule.** Green = success, red = failure, amber =
attention/partial, cyan = in-flight, grey = inert — identically in every
table, badge, chart, and report. A color is a claim about state; never
re-purpose one decoratively.

**The AA Floor Rule.** Meaningful text never drops below 4.5:1 on its
background: Muted #9ca3af is the workhorse, Faint #6b7280 the de-emphasis
floor. Only placeholders and disabled states may go dimmer (#4b5563). This
lifted 950+ instances one step in v0.28.106 — don't regress it.

## Typography

**Display/Body/Data Font:** Cascadia Code (with JetBrains Mono, ui-monospace,
Consolas fallbacks — named faces first so macOS and Windows resolve the same
family)

**Character:** One monospace voice at every level — headlines are just louder
telemetry. The pairing of bold monospace headings with 12px data grids reads
as a calibrated instrument, not a document.

### Hierarchy
- **Headline** (700, 1.25rem): page titles (`h2`), drawer titles.
- **Body** (400, 0.875rem): controls, form text, prose moments.
- **Data** (400, 0.75rem): the workhorse — tables, badges, metadata. Most of
  any screen is set at this size.
- **Label** (500, 0.75rem, 0.05em tracking, often UPPERCASE): section labels,
  column headers, in Muted #9ca3af.

### Named Rules
**The Data-First Rule.** 12px monospace is the default voice; larger sizes are
the exception and must earn their space. Never inflate type to fill a layout.

## Layout

Dense, left-railed application shell: a fixed sidebar (#0f1015) with grouped
monospace nav, and a content column padded 16–24px. Vertical rhythm comes from
1.5rem section breaks ruled by hairline dividers (`.section-divider`) rather
than card wrappers. Tables are the primary layout organism: full-width inside
a 1px-bordered, 6px-radius container that scrolls horizontally on narrow
viewports rather than reflowing the page. Spacing follows the Tailwind 4px
grid; compact steps (4/8/16/24) dominate — generous whitespace is not part of
this system's vocabulary.

## Elevation & Depth

Strictly flat. **No box-shadows anywhere.** Depth is conveyed tonally by the
four-step background stack (base → surface → sidebar → raised) plus 1px
hairline borders; overlays (drawers, dialogs) sit on a dimmed backdrop instead
of casting shadows.

### Named Rules
**The Borders-Not-Shadows Rule.** If a surface needs separation, step its
background one token and rule it with Hairline #1a1b25. A shadow is never the
answer.

## Shapes

Tight, technical corner language: 4px radius on controls (buttons, inputs,
badges), 6px on table containers, 8px reserved for larger panels, and full
rounds only for status dots and pills. Edges are always crisp — 1px borders,
no outlines thicker than the focus border-shift. The recurring silhouette is
the ruled rectangle: content framed by hairlines, never floated.

## Components

Controls are **tactile instrument keys**: crisp edges, immediate press
feedback, quiet at rest.

### Buttons
- **Shape:** tight corners (4px radius)
- **Primary:** Interactive Cyan fill (cyan-600 oklch), white text, 8px 16px
  padding, 0.875rem monospace
- **Hover / Focus:** fill lightens one step (cyan-500) over 150ms; every
  enabled button presses down 1px on `:active` (`translateY(1px)`)
- **Secondary/Ghost:** transparent with gray-700 border, gray-300 text; border
  lightens on hover
- **Disabled:** 50% opacity, `cursor: not-allowed` (never invisible
  disabling — a disabled-looking button must look disabled)

### Chips / Badges
- **Style:** 12px monospace, 4px radius, tinted background at ~10–20% of its
  status color with the full-strength status color as text
- **State:** colors follow the Status Semantics Rule; selected toggles use a
  cyan-tinted fill (`bg-cyan-900/40`, cyan-300 text) with an inset 2px cyan
  left bar on list options

### Cards / Containers
- **Corner Style:** 6px
- **Background:** Terminal Black; headers step to Surface
- **Shadow Strategy:** none — see Borders-Not-Shadows
- **Border:** 1px Hairline
- **Internal Padding:** 16px

### Inputs / Fields
- **Style:** Terminal Black well, 1px gray-700 stroke, 4px radius, 6px 12px
  padding, 0.875rem
- **Focus:** stroke shifts to cyan over 150ms — no glow, no ring
- **Placeholder:** Placeholder #4b5563

### Navigation
- Grouped rail with uppercase 12px section labels in Muted; items are 0.875rem
  monospace in gray-400, hover to gray-200, active in cyan-400. Collapsible;
  connection status lives at the top of the rail.

### Motion (system-wide)
150–200ms, `cubic-bezier(0.16, 1, 0.3, 1)`; drawers slide in from the right,
toasts rise 8px, badges scale in from 0.8. `prefers-reduced-motion` disables
all entrance animations.

## Do's and Don'ts

### Do:
- **Do** set data in 12px monospace and let density carry the design; every
  pixel should communicate information.
- **Do** use the four-step background stack + 1px Hairline borders for all
  separation and depth.
- **Do** keep status colors semantically exact everywhere (green/red/amber/
  cyan/grey), including reports and charts.
- **Do** give every control press feedback (1px translate) and a 150ms
  border/fill transition — tactile, not animated.
- **Do** honor `prefers-reduced-motion` on every entrance animation.

### Don't:
- **Don't** use Brand Violet outside the logo/brand mark (Logo-Only Violet
  Rule).
- **Don't** add box-shadows, gradients, or glassmorphism — flat surfaces and
  borders only.
- **Don't** introduce a second interactive accent or re-purpose a status color
  decoratively.
- **Don't** put gray text on colored fills — use white/near-white or a darker
  shade of the fill (the detector's standing gray-on-color finding class).
- **Don't** disable a control without disabled styling; a dead-looking-alive
  button is a bug (the v0.28.104 launch-button lesson).
