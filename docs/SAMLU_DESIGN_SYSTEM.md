# Samlu Design System

Status: Living product and interface specification  
Primary platform: macOS, Apple silicon  
Last updated: 2026-07-27

## Product posture

Samlu is a background-first AI development utility. Its settings window is a control center, not the primary daily interface. Voice capture, prompt transformation, launcher actions, and agent notifications should remain available while the settings window is closed and while the user works in another application, Space, or display.

The interface should feel calm, personable, and immediate. It should use macOS conventions where they improve familiarity, while retaining a recognizable Samlu visual identity.

## Firm decisions

### Platform and distribution

- Ship macOS first.
- Support Apple silicon for the first beta.
- Keep the app resident through its menu-bar process.
- iOS companion work begins only after the macOS build is stable.

### Voice interaction

- The everyday voice interface is a compact **Voice Capsule**, separate from the settings window.
- The Voice Capsule appears at the bottom center of the display containing the focused window when recording begins.
- The focused window is the frontmost app's frontmost on-screen window, read from the window server (no Screen Recording permission needed). If it cannot be determined, use the display containing the pointer.
- The Samlu Island and the launcher use the same rule, so every presence surface follows the window you switched to rather than where the pointer rests.
- All overlay placement happens in AppKit points. Physical-pixel positioning is avoided because it converts with the scale of the display the window is currently on, which misplaces it across mixed-DPI displays.
- Once recording begins, the Voice Capsule remains anchored to that display for the full recording session.
- It must not activate Samlu or steal focus from the target application.
- It must work across applications, displays, Spaces, and full-screen contexts.
- Support both press-and-hold and toggle recording shortcuts.
- Recording behavior is configurable per hotkey.
- Delivery behavior is configurable per voice mode.
- Supported delivery choices are:
  - Instant insertion
  - Compact editable preview
  - Copy only
- Initial recommended defaults:
  - Dictation: instant insertion
  - Prompt: compact editable preview
  - Summary: instant insertion
- Speech transcription and prompt transformation may use separate providers.

### Agent notifications

- Agent notifications use a **Samlu Island** around or immediately below the camera housing when the active display has one.
- The Samlu Island follows whichever display the user is currently using.
- Displays without a camera housing use a visually equivalent top-center floating capsule.
- Native macOS notifications provide durable Notification Center history.
- The Samlu Island provides immediate, contextual feedback and does not replace the native notification record.
- Agent summaries are configurable.
- Notification payload content remains encrypted.

### Visual direction

- The selected direction is **Soft Presence**.
- The appearance selector provides System, Light, and Dark.
- System is the default and follows the macOS appearance.
- The visual system should feel warm and personable without becoming playful, toy-like, or visually noisy.
- Translucent materials are reserved primarily for floating surfaces.
- State must never be communicated by color alone.
- Samlu's character on presence surfaces is the **breath bead**: a small yellow bead with a breathing halo (`.samlu-bead` in `presence.css`). The flying mascot and its pointer-delivered cards are retired.
- Every `<select>` is drawn by `samlu-select.js` as a Samlu dropdown or, with `data-variant="segmented"`, a segmented control. Native select chrome never shows.

## Interaction architecture

Samlu has three interface layers:

1. **Background service**
   - Owns global shortcuts, recording, transcription, transformation, text delivery, launcher actions, agent monitoring, and notification routing.

2. **Presence surfaces**
   - Voice Capsule
   - Samlu Island
   - Launcher
   - Compact editable preview
   - Recovery surfaces

3. **Control center**
   - Settings
   - Provider configuration
   - Voice mode configuration
   - Agent integration settings
   - Notification privacy and summary settings
   - History and diagnostics

The presence surfaces must never depend on the control-center window being visible.

## Voice state model

```text
Idle
  → Listening
  → Processing
  → Delivering
  → Confirmed

Listening → Cancelled
Processing → Cancelled
Delivering → Recovery
Processing → Recovery
```

### Idle

- Voice Capsule is hidden.
- Menu-bar state remains available.

### Listening

- Appears immediately after the shortcut is recognized.
- Shows a real microphone-driven waveform.
- Shows the active voice mode without unnecessary explanatory text.
- Provides a clear cancel path.

### Processing

- Preserves spatial continuity with the Listening state.
- Distinguishes transcription from optional prompt transformation.
- Avoids indeterminate decorative motion when meaningful progress is available.

### Delivering

- Attempts insertion into the exact field and window where recording began.
- Does not bring the settings window forward.

### Confirmed

- Shows restrained success feedback.
- Dismisses quickly.

### Recovery

- Persists until the result is safely delivered, copied, explicitly dismissed, or opened in the editable preview.
- Never discards the transcript because insertion failed.
- Offers Retry, Copy, and Preview actions as appropriate.

## Voice Capsule specification

### Geometry and placement

- Listening is a 44-point pill sized to its content: breath bead, voice beads and timer (160 points), plus a mode chip for Summary and Prompt, plus Cancel and Stop once a tap makes it a toggle recording. Windows add an 8-point inset on every side for the shadow.
- Working is a 236 × 44 pill; Landed folds to 120 × 36; Kept safe grows to 360 × 118.
- Place it at the horizontal center of the selected display, above the Dock and safe area.
- It may expand horizontally to reveal contextual controls.
- Editable preview expands upward from the same anchor.
- Position changes between states preserve spatial continuity.

### Recording interaction

- Press-and-hold recording shows the waveform and mode with minimal visible controls.
- Releasing the configured shortcut finishes press-and-hold recording.
- Toggle recording shows a visible stop control.
- Escape cancels from either recording behavior.
- Cancel remains available during processing while the active work can still be stopped.
- Contextual controls may appear on hover but keyboard access cannot depend on hover.

### Motion and waveform

- Initial appearance should complete in approximately 100 milliseconds.
- The waveform is driven by real microphone levels.
- Nine rounded voice beads, tallest in the middle, follow the real microphone level; quiet speech stays as dots.
- During processing, the beads gather into a calm three-dot pulse, and the pill names the step honestly: "Heard" once transcription is done, then the shaping step.
- Success folds the pill into a drawn check with a single word ("Inserted", "Copied") and dismisses.
- Successful delivery resolves into a small confirmation mark and dismisses quickly.
- Failure expands into the persistent Recovery state.
- Reduce Motion replaces spatial morphs with short fades while retaining all state feedback.

### Audio feedback

- Provide restrained start, stop, success, and failure sounds.
- Audio feedback is enabled by default.
- A global setting can mute interface sounds.
- Visual feedback remains complete when sounds are disabled.

### Compact editable preview

- Initial target size is approximately 520 × 220 points.
- It expands upward from the Voice Capsule anchor.
- Opening the editor intentionally takes focus.
- Samlu preserves the original text target while the editor is open.
- Command-Return inserts the result into the original target.
- Escape closes the preview without losing the generated result.
- The preview provides explicit Insert, Copy, and Cancel or Close actions.

## Target preservation requirements

When recording begins, Samlu should preserve enough context to return text accurately:

- Active display
- Active application and process
- Active application window
- Focused accessibility element
- Existing selection or insertion point when accessible

Clipboard-based simulated paste is a fallback, not the primary mental model. When fallback paste is used, Samlu should verify success where practical and preserve the generated text for recovery.

## Notification behavior

Initial policy:

| Event | Samlu Island | Native notification |
|---|---|---|
| Agent completed | Seven seconds, then dismiss | Yes |
| Agent failed | Extended with action | Yes |
| Input or approval required | Persistent until acted on or dismissed | Yes |
| Finished turn | Six seconds, then dismiss | Configurable |
| Multiple completions | Grouped summary | Grouped where supported |

Sensitive summaries should respect the configured privacy level. A privacy mode may hide content until interaction while still showing the event source and status.

## Soft Presence foundations

The foundation is:

- Warm neutral backgrounds rather than blue-gray enterprise surfaces
- Soft white and charcoal text with strong contrast
- Samlu yellow used selectively for presence, listening, and primary actions
- Restrained translucency on capsules and islands
- Gentle depth with thin borders and controlled shadows
- System typography for interface text
- Monospaced typography only for code, commands, models, providers, and technical metadata
- Compact, tactile controls with generous hit targets
- Fast state transitions and quiet dismissal
- Light and dark themes designed independently rather than mechanically inverted

### Theme choices

- System follows the macOS appearance and is the default.
- Light uses warm ivory canvas and raised soft-white surfaces.
- Dark uses warm charcoal rather than pure black.
- The Samlu Island remains near-black in either appearance so it relates visually to the camera housing.

### Core brand tokens

- Brand yellow: `#efd01d`
- Brand dark olive: `#44390c`
- Light canvas: `#f5f2e8`
- Dark canvas: `#15140f`
- Light ink: `#29250f`
- Dark ink: `#f7f2df`

Implementation tokens live in `public/presence.css` and are shared by Settings, Voice Capsule, Launcher, and Samlu Island.

## Accessibility and motion

- Respect Reduce Motion.
- Respect Increase Contrast and Reduce Transparency.
- Do not require waveform motion to understand recording state.
- Keep keyboard control complete.
- Floating surfaces must not take focus unless the user intentionally opens an editable preview.
- Repeated shortcut interactions should appear immediately and avoid elaborate entrance animation.

## Open design decisions

- Notification grouping for simultaneous agent events
- A privacy mode that automatically obscures summaries during screen sharing
- A caret glint at the insertion point after delivery (needs the focused element's caret bounds through Accessibility)

## Implemented refinement status

The first refined macOS slice includes:

- Background AppKit overlay behavior across Spaces and full-screen applications
- Menu-bar background mode after the Settings window closes
- Active-display Voice Capsule anchored for the recording session
- Real microphone-driven waveform
- Listening, processing, success, error, preview, and recovery states
- Per-mode Instant insert, Editable preview, and Copy only settings
- Dictation and Summary instant-insert defaults
- Prompt editable-preview default
- Accessibility readiness status and direct link to macOS settings
- Persistent recovery when automatic insertion fails
- Native notifications plus the configurable Samlu Island
- System, Light, and Dark appearance choices
- Shared Soft Presence tokens across Settings, Launcher, Voice, and notifications

## Competitive reference principles

Wispr Flow and Superwhisper validate separating background dictation feedback from the settings application. Samlu should adopt the underlying principles—persistent cross-app feedback, clear recording state, and automatic delivery—without copying either product's visual styling.
