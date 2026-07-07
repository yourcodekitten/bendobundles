---
name: bendobundles
description: ben's 15 years of humble bundles as a gift shelf for friends — the attic arcade, pea soup world
colors:
  room: "oklch(87% 0.085 112)"
  floor: "oklch(91% 0.07 110)"
  shelf: "oklch(80% 0.09 112)"
  control: "oklch(73% 0.09 113)"
  control-bright: "oklch(69% 0.09 114)"
  ink: "oklch(30% 0.06 120)"
  ink-soft: "oklch(36% 0.06 118)"
  dust: "oklch(43% 0.06 116)"
  dust-faint: "oklch(48% 0.06 114)"
  line: "oklch(66% 0.075 112)"
  pixel: "oklch(35% 0.06 120)"
  give: "oklch(46% 0.155 355)"
  give-bright: "oklch(52% 0.16 355)"
  give-soft: "oklch(42% 0.14 350)"
  give-ink: "oklch(97% 0.012 350)"
typography:
  display:
    fontFamily: "ui-sans-serif, system-ui, sans-serif"
    fontSize: "2.25rem"
    fontWeight: 700
    lineHeight: 1.15
    letterSpacing: "-0.025em"
  headline:
    fontFamily: "ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.5rem"
    fontWeight: 600
    lineHeight: 1.3
  title:
    fontFamily: "ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.125rem"
    fontWeight: 600
    lineHeight: 1.4
  body:
    fontFamily: "ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.875rem"
    fontWeight: 400
    lineHeight: 1.5
  label:
    fontFamily: "ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.75rem"
    fontWeight: 400
    lineHeight: 1.4
  mono:
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace"
    fontSize: "0.75rem"
    fontWeight: 400
rounded:
  sm: "0.25rem"
  lg: "0.5rem"
  xl: "0.75rem"
spacing:
  chip: "2px 8px"
  control: "8px 16px"
  card: "16px"
  panel: "24px"
components:
  button-primary:
    backgroundColor: "{colors.give}"
    textColor: "{colors.give-ink}"
    rounded: "{rounded.sm}"
    padding: "{spacing.control}"
    typography: "{typography.body}"
  button-primary-hover:
    backgroundColor: "{colors.give-bright}"
  button-neutral:
    backgroundColor: "{colors.control}"
    textColor: "{colors.ink}"
    rounded: "{rounded.sm}"
    padding: "{spacing.control}"
  button-neutral-hover:
    backgroundColor: "{colors.control-bright}"
  chip:
    backgroundColor: "{colors.shelf}"
    textColor: "{colors.ink-soft}"
    rounded: "{rounded.sm}"
    padding: "{spacing.chip}"
    typography: "{typography.label}"
  card:
    backgroundColor: "{colors.floor}"
    rounded: "{rounded.lg}"
  dialog:
    backgroundColor: "{colors.floor}"
    rounded: "{rounded.xl}"
    padding: "{spacing.panel}"
---

# Design System: bendobundles

## 1. Overview

**Creative North Star: "The Attic Arcade" — pea soup world**

Fifteen years of Humble Bundle purchases live here the way a game collection lives in an attic —
and the most treasured find in that attic is the original 1989 handheld, still working. The whole
interface is its screen: the light pea-green olive of a monochrome LCD, dark-olive ink, and one
saturated accent borrowed from the machine's own A/B buttons — burgundy-magenta, spent only on
the act of giving. The original handheld was never backlit; you played it by window light. A
light theme is the historically honest choice, and it is also the anti-reflex one: nobody
guesses "light olive" from the category "game gifting app."

This system explicitly rejects its category's reflexes. It is not a storefront — no prices, no
urgency, no deals grammar. It is not SaaS dashboard chrome — no metric cards, no
sidebar-and-breadcrumb costume, not even on the admin workbench. It is not gamer-RGB edgelord —
the nostalgia here is matte plastic and pea soup, not neon. And it is not corporate minimalism —
the monochrome discipline is a loving constraint, never sterile restraint.

The component doctrine is **tactile and cozy** — plump enough to press, quiet enough to live in —
with the energy budget spent on one moment: a friend opening a gift chosen for them.

**Key Characteristics:**
- Light, tonal, and monochrome-disciplined: the room is one green, in four-plus shades
- One saturated accent (Button Burgundy) reserved for the act of giving/claiming
- Games are the color: cover art and the deterministic title-hash palette pop against the olive
- Lowercase voice everywhere; ceremony saved for the unwrap
- Bezel as motif: chunky dark-olive frames mark ceremony surfaces (dialogs, the landing art)

## 2. Colors: The Pea Soup Palette

One green room in stepped shades; the games provide every other color.

### Primary
- **Button Burgundy** (`give`, oklch(46% 0.155 355)): the burgundy-magenta of the original
  handheld's A/B buttons. It marks the path to claiming — the claim button, the "it's yours! ♡"
  celebration, the gifted badge. It is generosity's color, never decoration.
- **Button Burgundy Bright** (`give-bright`, oklch(52% 0.16 355)): hover state.
- **Burgundy Ink** (`give-soft`, oklch(42% 0.14 350)): accent *text* on light surfaces — the
  highlighted game title in a dialog, the word "key" on the landing.
- **On-Burgundy** (`give-ink`, oklch(97% 0.012 350)): text on burgundy backgrounds. Every
  `bg-give` element pairs with `text-give-ink` — inherited dark ink on burgundy is a bug.

### Neutral (the screen, light to dark)
- **Room** (oklch(87% 0.085 112)): the page background — the LCD at rest.
- **Floor** (oklch(91% 0.07 110)): cards, dialogs, panels — one shade lighter, like a lit segment.
- **Shelf** (oklch(80% 0.09 112)): chips, raised rows.
- **Control** (oklch(73% 0.09 113)) / **Control Bright** (69%): neutral buttons and their hover.
- **Ink** (oklch(30% 0.06 120)): primary text — the darkest LCD shade.
- **Ink Soft** (36%) / **Dust** (43%) / **Dust Faint** (48%): secondary, muted, faintest text.
- **Line** (oklch(66% 0.075 112)): hairline borders.
- **Pixel** (oklch(35% 0.06 120)): the bezel color — dialog rings, focus borders, art frames.

### Status tints (deep bg + pale text, always paired; semantic hues survive every theme)
- **Claimed / success**: green-700 chip bg with green-100 text; inline success text uses
  green-700 on light surfaces.
- **Pending / caution**: amber-700 bg with amber-100 text; inline caution text uses amber-800.
- **Error / danger**: red-700/900 bg with red-100 text; inline error text uses red-700.
- **Info / steam**: blue-700/900 bg with blue-100/200 text.

### Named Rules
**The Button Burgundy Rule.** Burgundy appears only where giving or claiming happens — never as
ambient decoration, never above 10% of a screen. Its rarity is what makes the claim button feel
like pressing A to accept a gift.

**The Title-Hash Rule.** A game without cover art gets its color from the shared deterministic
hash (`titleColor.ts`): violet, blue, green, amber, red, pink, teal, indigo — all at the -800
step. The SAME game must render the SAME color on every surface, forever. Any palette change must
preserve determinism and cross-surface agreement. (The hash palette deliberately survived the
pea soup repaint: deep art-blocks read as cartridge labels against the light olive.)

**The Light Text Rule.** Status *text* on light surfaces uses the deep end of its hue
(green-700, amber-800, red-700) — the pale 300/400 tones that read on dark are invisible on
olive and are forbidden as inline text here.

## 3. Typography

**Display Font:** system-ui stack (ui-sans-serif, system-ui, sans-serif)
**Body Font:** same system stack
**Label/Mono Font:** ui-monospace (keys, tokens, and technical identifiers only)

**Character:** Unassuming and honest — plain type on a plain screen, the way the handheld's
manual was set. No display face has been chosen yet; if one ever is, it must be warm and a
little nostalgic, never corporate or aggressive-gamer. The personality lives in the lowercase
voice, not the letterforms.

### Hierarchy
- **Display** (700, 2.25rem, tight tracking -0.025em): the landing wordmark moment only.
- **Headline** (600, 1.5rem): page-level headings on admin surfaces.
- **Title** (600, 1.125rem): dialog headings — "claim {game}?", "it's yours! ♡".
- **Body** (400, 0.875rem): the default UI size.
- **Label** (400–500, 0.75rem): chips, bundle names, helper text. The most-used size in the app.
- **Mono** (400, 0.75rem): gift keys, link tokens, appids. Never for voice copy.

### Named Rules
**The Lowercase Rule.** UI copy is lowercase — headings, buttons, labels, errors. Caps do not
exist in the attic. (Game titles render as their owners spell them; the rule governs OUR words.)

**The One Heart Rule.** The ♡ appears at sincere moments — the landing line, the successful
claim — never scattered as decoration. One per screen, maximum, and it must be earned.

## 4. Elevation

Depth is tonal, not shadowed: surfaces step the olive ladder (Room → Floor → Shelf → Control)
and that layering IS the elevation system. Real shadows are reserved for ceremony, and on this
light theme they are joined by the bezel.

### Shadow Vocabulary
- **Ceremony** (`box-shadow: 0 25px 50px -12px rgb(0 0 0 / 0.25)`, shadow-2xl): dialogs and the
  gift/claim moments only — paired with a Pixel ring and a 60% black backdrop.

### Named Rules
**The Ceremony Rule.** A drop shadow means a gift moment is happening. Cards, chips, nav, and
buttons are flat at every state; if you're adding a shadow anywhere but a claim/detail dialog,
you're diluting the unwrap.

**The Bezel Rule.** Chunky dark-olive (Pixel) borders mark the handheld's screen: the landing
art frame and dialog rings wear it. Hairlines everywhere else are Line. A thick border on an
ordinary card is costume.

## 5. Components

Tactile and cozy: quiet, pressable, unhurried — with all saved energy spent on the claim.

### Buttons
- **Shape:** softly squared (0.25rem radius)
- **Primary (give):** Button Burgundy bg, On-Burgundy text, 8px × 16px padding, 0.875rem medium.
  The claim button is the canonical instance. Hover brightens to Button Burgundy Bright;
  disabled drops to 40% opacity with `cursor: not-allowed` (dim it, never hide it).
- **Neutral:** Control bg with inherited Ink text, hover Control Bright — retry, connect steam,
  cancel-type actions.
- **Ghost:** transparent bg, Dust text brightening to Ink Soft on hover — the "never mind".

### Chips
- **Style:** Shelf bg, Ink Soft text, 0.25rem radius, 2px × 8px padding, 0.75rem text.
- **State:** status chips swap to the deep-bg/pale-text status pairs; the "gifted" badge wears
  Button Burgundy (it records a completed act of giving).

### Cards / Containers
- **Corner Style:** 0.5rem radius (one step softer than controls)
- **Background:** Floor on Room
- **Shadow Strategy:** none — tonal only (The Ceremony Rule)
- **Border:** none at rest
- **Internal Padding:** 16px body below an edge-to-edge 16:9 art area
- The **game card** is the signature container: cover art (or Title-Hash color block) bleeding
  to the edges on top, title + bundle + chips below, the claim button last. Against the light
  olive room, the art carries all the saturation — the card is its matte frame.

### Inputs / Fields
- **Style:** Floor bg, Line 1px border, 0.25rem radius, Ink text, Dust placeholders
- **Focus:** border deepens to Pixel (keep focus visible; never remove outlines without
  replacement)
- **Error:** inline red-700 text below the field, lowercase, in voice.

### Navigation
- **Admin nav:** a single quiet row — bottom border in Line, 24px × 12px padding, body-size
  links in Dust; the active link holds Ink. No sidebar, no breadcrumbs, no chrome.

### Dialogs (signature)
The gift moments live here. Fixed 60% black backdrop (z-40), centered panel (z-50): max-w-md,
Floor bg, 0.75rem radius, 24px padding, Ceremony shadow, Pixel ring (The Bezel Rule). Focus is
trapped and returned (ClaimDialog / GameDetailModal pattern). The unwrap sequence — question →
"claiming…" → "it's yours! ♡" in Burgundy Ink with the key revealed — is the most important
interaction in the product.

### The Landing (signature)
Room-drenched single viewport: the pixel-art scene (`/art/landing.png` — a four-shade
adventurer walking toward a treasure chest) in a Pixel bezel with the DMG's asymmetric corner
(`14px 14px 44px 14px`), wordmark beneath, and the one line where "key" wears Burgundy Ink.
Entrance is a single quiet rise with a `prefers-reduced-motion` bypass.

## 6. Do's and Don'ts

### Do:
- **Do** reserve Button Burgundy for giving and claiming (The Button Burgundy Rule; ≤10% of any
  screen), and pair every `bg-give` with `text-give-ink`.
- **Do** keep every word lowercase and in voice — playful, cozy, sincere (The Lowercase Rule).
- **Do** let cover art and the Title-Hash palette carry the saturation; the room stays one
  green.
- **Do** keep the thin fallback path working: a game with no enrichment still renders name,
  color block, and a working claim ("delight never gates").
- **Do** use deep status text on light surfaces (green-700 / amber-800 / red-700) and deep
  chip backgrounds with pale text (The Light Text Rule).
- **Do** trap and return focus in every dialog, and give any motion a
  `prefers-reduced-motion` alternative.

### Don't:
- **Don't** import storefront / commerce grammar — no prices, no urgency, no "deals" visual
  language, no conversion patterns. It's a gift shelf, never a shop (PRODUCT.md anti-reference).
- **Don't** build SaaS dashboard chrome — no metric-card grids, no sidebar-and-breadcrumb
  scaffolding, no BI styling, even on admin (PRODUCT.md anti-reference).
- **Don't** go gamer-RGB edgelord — no neon-on-black, no angular esports styling, no
  glow-everything (PRODUCT.md anti-reference).
- **Don't** drift into corporate minimalism — sterile white-space-and-gray-text restraint is as
  off-brand as neon (PRODUCT.md anti-reference).
- **Don't** reintroduce the zinc+violet defaults this system replaced, and don't add new hue
  families to the room — one green, stepped, is the discipline.
- **Don't** add shadows outside dialogs (The Ceremony Rule), side-stripe accent borders,
  gradient text, or glassmorphism. The attic doesn't do costume.
- **Don't** break Title-Hash determinism: the same game showing two different fallback colors on
  two surfaces is a bug, not a variant.
- **Don't** scatter hearts. One ♡ per screen, earned (The One Heart Rule).
