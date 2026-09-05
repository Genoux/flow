# Flow: an editorial, analog direction

Working direction for the Style-page experiment, guided by the user's
Perplexity references and preference for vintage, argentique photography.

## The promise

Your thoughts reach the page with less effort, while your voice remains yours.
The interface should feel calm, personal, and carefully composed.

## Photography

Use atmospheric natural photography: water, grasses, reflected light, and
quiet landscapes. Favor film grain, gentle optical softness, muted olive
and warm neutral tones, and deep shadows. The photographs should feel tactile
and observed, with a specific subject and deliberate framing.

Compose banner images with negative space for copy. Let a photograph establish
the mood of a page, with enough darkness behind text to keep it readable. The
Style-page study uses a mossy mushroom close-up from the user-selected Joey
Bania clip: tactile detail, soft light, and an unhurried woodland atmosphere.

## Typography

Pair an expressive display serif with the app's existing practical sans-serif.
Noto Serif Display is reserved for the banner headline. Navigation, settings,
card descriptions, and transcript examples stay sans-serif. This gives the
page an editorial voice while preserving everyday readability.

## Interface decisions

- Keep the existing near-black ground, quiet surfaces, and off-white text.
- Use photography for warmth; keep green for active and selected controls.
- Integrate the photograph into one banner, with restrained corners and no
  decorative outline. Use antialiased continuous corners for photographic
  banners, with a shared 24 px corner extent. Preserve stable text and control positions.
- Keep the three style choices simple, readable, and directly selectable.
- Show selection through the radio and subtle tint. Active cards rest on hover.
- Use plain language that emphasizes the user's authorship and control.
- Static photography establishes the identity; animation is reserved for
  meaningful interaction feedback. Buttons, navigation, style cards, history
  rows, field focus, sliders, and toggles share a 200 ms duration
  (`theme::FADE`) and smoothstep easing (`motion::ease`). New controls use
  `interaction::hover` or `interaction::field`; state-driven highlights use
  `motion::Transition`. Actions and typing respond immediately. Interrupted
  transitions continue from their current brightness; nothing slides or grows.
- Style and Vocabulary share banner construction and headline typography.
  Vocabulary uses the user-selected Nick Fancher photograph, with a distinct
  palette and composition within the same editorial treatment.
  Vocabulary uses a more compact header so the editing task comes into view.
- Vocabulary is organized alphabetically with search and quiet removal
  controls. Supporting messages reserve space and body text stays sans-serif.

## Review criteria

The image should hold up as a photographic composition and the type should
remain readable over it. At a glance, the page should feel composed and the
style choice should be obvious. Future pages should share the photographic
palette and typography while choosing imagery appropriate to their purpose.

## Overview

Make this a view of usage: a full-width photographic banner leads with the last
seven days' word count and comparison. Dictations, speaking time, and streak
sit below it, followed by the longer activity calendar. Use the user-selected Mouthwash Studios micro footage as a still image,
with shared continuous corners. Keep the Overview banner at 180 px high. Use the extended photographic
composition to pull the subject back, with quiet negative space behind the word count. Keep transcript previews, shortcuts,
microphone details, and model counts on their dedicated pages. Service controls
and actionable problems remain available.
