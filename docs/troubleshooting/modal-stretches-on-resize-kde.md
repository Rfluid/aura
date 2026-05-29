---
title: "Troubleshooting: modal stretches / animates when it resizes (KDE Plasma)"
status: draft
version: 0.1.0
last_updated: 2026-05-29
last_verified: 2026-05-29
source_refs:
  - crates/aura/src/app.rs
  - crates/aura/src/platform.rs
  - crates/aura/src/placement.rs
  - vendor/gpui/src/platform/linux/x11/window.rs
owner: "@rfluid"
tags: [troubleshooting, linux, kde, docs]
---

# Modal stretches / animates when it resizes (KDE Plasma)

## Symptom

On KDE Plasma, when the Aura modal changes height — on open, when switching
tabs/periods, or when `anchor = "bottom"` grows it upward — the window's
**contents visibly stretch vertically** and the resize plays out over roughly
**0.3–1 second** instead of snapping to the new size instantly. Text looks
squashed/stretched mid-animation, then "normalizes" once the window settles.

This is purely cosmetic — placement is correct and the final size is correct —
but the animation feels sluggish, especially for the bottom-anchored
grow-upward behaviour where you want the resize to look instant.

## Cause

It is **not** an Aura animation — Aura has no resize transition. It is KWin's
**Morphing Popups** desktop effect (`kwin4_effect_morphingpopups`), which is
enabled by default on Plasma. That effect smoothly cross-fades/stretches the
geometry of *popup-type* windows (tooltips, combo-box dropdowns, notifications)
as they change size.

Aura's modal is created as a popup: GPUI maps `WindowKind::PopUp` to
`_NET_WM_WINDOW_TYPE_NOTIFICATION` (see
`vendor/gpui/src/platform/linux/x11/window.rs`), and KWin treats notification
windows as popups for this effect. So every resize of the modal gets morphed —
KWin scales the old window buffer to the new size while it animates, which is
the vertical "stretch" you see.

## Confirm it

```bash
# qdbus6 on Qt6 Plasma; qdbus on older setups.
qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.isEffectLoaded morphingpopups \
  || qdbus org.kde.KWin /Effects org.kde.kwin.Effects.isEffectLoaded morphingpopups
# -> "true" means the effect is active and is the cause.
```

You can prove it live by unloading the effect for the current session (no
config change, reverts on next login or `reconfigure`):

```bash
qdbus org.kde.KWin /Effects org.kde.kwin.Effects.unloadEffect morphingpopups
```

Open the modal and switch tabs — the stretch should be gone. To restore it:

```bash
qdbus org.kde.KWin /Effects org.kde.kwin.Effects.loadEffect morphingpopups
# or just:
qdbus org.kde.KWin /KWin reconfigure
```

## Fix

### Option A — System Settings (GUI, persistent)

1. Open **System Settings** and search for **Desktop Effects**.
2. Under the **Appearance** category, find **Morphing Popups**.
3. Untick it and **Apply**.

This makes tooltips and combo-box dropdowns resize instantly too — a very
minor cosmetic change most people never notice.

### Option B — command line (persistent)

```bash
# Qt6 Plasma:
kwriteconfig6 --file kwinrc --group Plugins --key morphingpopupsEnabled false
# Qt5 Plasma:  kwriteconfig5 --file kwinrc --group Plugins --key morphingpopupsEnabled false

# Apply without logging out:
qdbus org.kde.KWin /KWin reconfigure
```

Revert by setting the key to `true` (or deleting it) and running
`reconfigure` again.

## Notes & scope

- **This is KDE/KWin-specific.** Windows and macOS have no equivalent
  resize animation on the modal; their repositions are already instant.
- **Aura does not change this for you.** Disabling Morphing Popups is a
  *global* KWin setting that also affects unrelated tooltips and dropdowns,
  so the installer deliberately does not touch it — it is left as an opt-in
  user choice documented here.
- **This is separate from placement.** If the modal opens in the wrong
  corner or does not hug the taskbar, that is the `anchor` setting, not this
  effect — see [Modal anchoring](../configuration.md#modal-anchoring-anchor)
  and the Wayland note in [configuration.md](../configuration.md).
- **Wayland:** the same effect applies, but on Wayland Aura cannot reposition
  itself after a resize anyway (the compositor owns placement), so the
  bottom-anchored grow-upward behaviour is X11-only regardless.
