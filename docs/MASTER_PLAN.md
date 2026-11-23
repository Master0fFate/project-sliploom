# SEAM — Master Product, Design, and Engineering Plan

> **Working title:** SEAM (not a final brand; name and trademark checks are still required)  
> **Product:** A local-first file transfer app for Android, Windows, macOS, and Linux  
> **Core interaction:** Android **Share → SEAM → trusted computer → sent**  
> **Desktop UI:** Rust + **zgx**, using the *design approach* of Zeron  
> **Android UI:** Kotlin + Jetpack Compose, with a shared Rust engine  
> **Status:** Planning and research. **No application code has been implemented yet.**  
> **Research date:** 2026-11-23  
> **Document version:** 0.1

---

## Quick navigation

| Start here | Go to |
|---|---|
| What we are building | [Product brief](#1-product-brief) · [User journeys](#2-user-journeys-and-acceptance-criteria) |
| The Zeron look and behavior | [zgx research and design system](#3-research-zgx-and-zeron-design-paradigm) |
| How it works | [Architecture](#4-platform-architecture) · [Protocol](#6-transfer-protocol-and-connection-flow) · [Security](#7-security-and-privacy-plan) |
| Where code will live | [Full folder tree](#5-repository-and-folder-structure) |
| What to build next | [Phases](#12-phased-implementation-plan-and-checklist) · [First work session](#18-first-work-session-exact-order) |
| What is uncertain | [USB study](#10-usb-and-wired-transfer-separate-research-track) · [Decision log](#16-decision-log) |
| Evidence | [Research sources](#17-sources-and-research-notes) |

---

## 0. How to use this plan

This file is the project control center. Keep it at `docs/MASTER_PLAN.md` after creating the repository.

- `[ ]` = not yet complete. `[x]` = complete and verified.
- Finish a task only after it meets its **Done when** test.
- Track changes to choices in [Decision log](#16-decision-log).
- Each phase has a clear gate. Do **not** mark a phase complete because its UI looks good.
- Prefer complete vertical slices: share a real file, receive it, then make the screen beautiful.
- A **Research verified** box means that source material was checked, not that code was built.
- Check off sub-tasks one by one. Parent items are only done when all required sub-tasks pass.

### Project at a glance

```text
SEAM
├── 00 Product and research               [research done; product work pending]
├── 01 Engineering foundation             [ ]
├── 02 Core network transfer              [ ]
├── 03 Android Share integration          [ ]
├── 04 zgx desktop receiver               [ ]
├── 05 Pairing and security               [ ]
├── 06 Cross-platform packaging           [ ]
├── 07 Polished design and accessibility  [ ]
├── 08 Hardening and public beta          [ ]
└── 09 Advanced transport: USB            [later; feasibility gate]
```

### Current progress

- [x] Identify Zeron and the zgx project and examine public documentation.
- [x] Confirm that zgx documents Rust/Luau desktop UIs, signals, GPU rendering, blur, native integration, and Windows/macOS/Linux targets.
- [x] Confirm Zeron's public design rules: theme tokens, component variants, spacing scale, nested radii, fine-grained updates, glass, and selection rings.
- [x] Confirm Android's Share intents, share targets, and Storage Access Framework from Android developer documentation.
- [x] Identify the Android foreground-transfer lifetime risk and the USB/MTP limitation.
- [ ] Verify a working zgx desktop app on **each** target OS, with our build toolchain and a pinned commit.
- [ ] Confirm the name and visual identity.
- [ ] Build and test the first complete Android-to-desktop transfer.

---

## 1. Product brief

### 1.1 One-sentence promise

> Send a file from any Android app to your trusted computer in one or two taps, with no cloud account, using a calm and reliable desktop companion.

### 1.2 The problem

- Android's file sharing is easy **inside** the phone. Sending files to a PC is often a different process.
- Some existing tools are powerful but require extra screens, account setup, or repeated destination selection.
- A USB cable is common, but ordinary Android MTP access is designed around a **desktop browsing the phone**. It does not automatically give a third-party Android Share target a direct data channel to the computer.
- A beautiful interface is useful only if transfers complete and errors make sense.

### 1.3 Who this is for

1. A person who frequently sends photos, PDFs, videos, and downloads from Android to a desktop.
2. A developer who shares screenshots, log files, ZIP archives, and code snippets.
3. A student or office user who wants a simple computer destination with no online account.
4. A person who switches between Linux, Windows, and macOS.

### 1.4 Product rules

- **Local first:** no account, cloud storage, or public server for basic sharing.
- **No deceptive automation:** the user chooses a trusted device; nothing is silently sent to an untrusted destination.
- **One obvious primary action** per screen.
- **Fast after first setup:** choose a previously paired computer, then send.
- **One shared protocol** across all desktop operating systems.
- **Readable by default:** glass is a visual treatment, not an excuse for low contrast.
- **Honest connectivity:** say “Not reachable” rather than claiming the phone is connected.
- **Secure by design:** protect against unknown senders, malicious filenames, and untrusted networks.
- **Progress is true progress:** success means the receiver finished and verified the data, not merely that the sender handed bytes to a socket.

### 1.5 First release scope (MVP)

**In scope**

- [ ] Android application appears in the Share menu for supported text and file types.
- [ ] Send **one** or **multiple** shared files Android → desktop over a reachable local network.
- [ ] Discover desktop receivers on the local network; support manual connection when discovery fails.
- [ ] Pair devices once with a confirmation/verification step.
- [ ] Show paired computers and send to one chosen computer.
- [ ] Desktop companion receives in the background when user enables that mode.
- [ ] Save verified files in a user-selected receive folder.
- [ ] Show live progress, completion, and understandable errors.
- [ ] Support Windows, macOS, and Linux release builds.
- [ ] Support dark, light, and reduced-transparency modes.

**Not promised in MVP**

- [ ] Zero-setup direct USB transfer on every Android/PC combination.
- [ ] Internet relay or remote file delivery.
- [ ] iOS client.
- [ ] Multi-computer simultaneous broadcast.
- [ ] Cloud sync, backup, or folder mirroring.
- [ ] Android → Android transfers.
- [ ] Full desktop file explorer for Android storage.

### 1.6 Potential differentiators

- A **direct receiver shortcut** in Android Share, after safe pairing and OS compatibility tests.
- Tiny, persistent desktop receiver with an elegant, optional main window.
- A restrained, Zeron-inspired visual system rather than a busy file manager.
- Strong failure recovery: retry, resume where possible, and clear messages.
- Transport independence that allows future wired and offline approaches.

### 1.7 Competitive research to perform

- [ ] Use LocalSend on Android + Windows and measure share-to-receipt taps.
- [ ] Use KDE Connect on Android + Linux and document device discovery and permissions.
- [ ] Test Quick Share for Windows where available; compare transfer speed and required setup.
- [ ] Record the strongest feature in each alternative without copying its visual assets.
- [ ] Write a one-page differentiation note with specific, measured improvements.

---

## 2. User journeys and acceptance criteria

### Journey A — First pairing

```text
Android first launch
  └── Welcome
      └── Find computers on local network
          ├── Found → Select computer → Verify code on both screens
          │                           └── Save trusted device
          └── Not found → Show network help / Enter IP or scan QR
                                  └── Verify code → Save trusted device
```

- [ ] Give a clear reason before asking for any Android permission.
- [ ] Show actual reachable receivers, not outdated saved devices as though online.
- [ ] Provide a secure pairing method with authenticated identity binding.
- [ ] Save trusted identity locally, not just the display name or IP address.
- [ ] Allow users to rename or forget a paired computer.
- [ ] On a shared computer, incoming files are not accepted until a user enables receiving.

**Done when:** A new phone pairs with a new desktop without creating any online account. The user can explain which machine they trusted.

### Journey B — Daily Android Share

```text
Gallery / Files / browser / chat / document app
  └── Share
      └── SEAM
          ├── One available favorite → Send (or Direct Share to that device)
          ├── Several devices → Tap target → Send
          └── No device → Retry / network help / queue with user approval
              └── Receiving desktop → Verify → Save → Notify
```

- [ ] Handle `ACTION_SEND` and `ACTION_SEND_MULTIPLE`.
- [ ] Handle read-granted `content://` URIs. Do not require real filesystem paths.
- [ ] Support `EXTRA_STREAM` and text payloads such as `EXTRA_TEXT`.
- [ ] Handle multiple incoming MIME types and missing metadata.
- [ ] Preserve original names when safe; sanitize unsafe paths and characters.
- [ ] Show exactly which destination will receive the content.
- [ ] Allow cancel before and during sending.
- [ ] Show an Android notification for a long active transfer when required.
- [ ] Never show “Sent” before the receiver confirms a successful save.

**Done when:** From a third-party Android app, a user shares a 25 MB file to a paired computer without opening an unrelated file browser.

### Journey C — Desktop to Android

```text
Desktop window
  └── Drop a file / Choose files
      └── Choose paired Android device
          └── Android receives consent / save destination as required
              └── Transfer and verify
```

- [ ] Build after the Android → desktop path is stable.
- [ ] Use Android's supported document/media APIs to save received files.
- [ ] Require explicit Android approval or a well-scoped trusted-receiving setting.
- [ ] Test Android background limits and storage behavior.

**Done when:** Desktop → Android works without broad storage permissions.

### Journey D — Failure and recovery

- [ ] Receiver disappears before transfer.
- [ ] Wi-Fi changes during transfer.
- [ ] File source becomes unreadable or its temporary share permission expires.
- [ ] Phone locks or moves app to the background.
- [ ] Desktop storage fills up.
- [ ] Target path exists with the same filename.
- [ ] Transfer is cancelled.
- [ ] Receiver process closes abruptly.
- [ ] Resume is attempted only if source identity and content match the previous attempt.

**Done when:** No case displays false success, overwrites a user file without policy, or leaves a partial file that looks complete.

---

## 3. Research: zgx and Zeron design paradigm

**Important distinction:** **zgx** is the UI framework. **Zeron** is an application whose public source and design rules illustrate a way to use zgx. Their implementation details may change. This plan is an interpretation, **not** a claim that our proposed APIs already exist.

### 3.1 Verified framework features (at research date)

- Retained-mode UI, with fine-grained signals that update dependent nodes instead of rebuilding whole screens.
- Rust and hot-reloading Luau UI paths.
- Desktop targets documented as macOS, Windows, and Linux.
- Flex and grid layout; text editing; virtual lists; accessible widgets.
- `wgpu` graphics, gradients, shadows, rounded borders, SVG, custom shaders, and backdrop blur.
- Native dialogs, notification support, drag-and-drop, appearance preferences, and accessibility integration.
- Zeron's zgx UI structure uses `ui/ds/` for design components, `themes/*.json` for values, and a `gallery.luau` for development.
- zgx is young, its API changes, and Windows/Linux feature parity must be **tested**, not assumed.

### 3.2 Design rules to adapt from Zeron

```text
Design decisions
├── Theme token = one source of truth
├── Component variants = reusable, typed visual recipes
├── Fixed spacing scale = consistent rhythm
├── Concentric corners = nested radius follows parent inset
├── Even interior insets = no visually compressed sides
├── Live state = update only the values that changed
├── Frosted glass = subtle backdrop, never unreadable text
├── Selection = light wash + thin ring, not a heavy shadow
└── Gallery = every state reviewed in light and dark before shipping
```

- [ ] Reproduce these **principles** in our own design system, not Zeron's branding or source assets.
- [ ] Evaluate zgx's Rust-first vs Luau-first app patterns.
- [ ] **Default choice:** Rust host + Luau for the desktop view and hot reload, following Zeron's demonstrated pattern.
- [ ] Confirm release bundling of Luau and fonts, if this path is selected.
- [ ] Keep transfer, security, storage, and OS logic in Rust. UI scripts must never hold trust secrets.
- [ ] Pin a tested zgx commit or release in `Cargo.lock`; change versions deliberately.
- [ ] Build a small compatibility spike on Windows, macOS, and Linux before investing in effects.

### 3.3 Visual direction: “monochrome glass”

The proposed app uses the **feel** of Zeron's native glass surfaces, black-and-white contrast, subtle rings, and careful motion. It must remain visually distinct and use its own logo and assets.

**Visual keywords:** quiet / precise / glass / charcoal / fog / soft white / fine outline / subtle rings / flat depth / clear status.

**Avoid:** giant glowing buttons, neon gradients, decorative 3D scenes, constant motion, fake blur on text, floating cards everywhere, long settings screens.

### 3.4 Design token proposal (custom to SEAM)

These are **starting values to test**, not official Zeron token values.

| Token | Dark proposal | Light proposal | Purpose |
|---|---|---|---|
| `seam.canvas` | `#080808` | `#F5F5F4` | App background |
| `seam.surface` | `#171717` | `#FFFFFF` | Main panel |
| `seam.surface.raised` | `#202020` | `#F0F0EE` | Elevated panel |
| `seam.text` | `#F6F6F6` | `#171717` | Primary text |
| `seam.text.muted` | `#B6B6B6` | `#575757` | Supporting text |
| `seam.stroke` | `#3A3A3A` | `#D9D9D6` | Hairline border |
| `seam.selection` | `#FFFFFF` | `#222222` | Thin selection ring |
| `seam.focus` | `#F4F4F4` | `#202020` | Keyboard focus indicator |
| `seam.success` | theme semantic | theme semantic | Status, text plus icon |
| `seam.warning` | theme semantic | theme semantic | Needs user action |
| `seam.failure` | theme semantic | theme semantic | Error |

- [ ] Set the dark window's underlay/scrim prototype to Zeron-inspired `#080808` at about 90% over native blur; compare with a fully opaque alternative.
- [ ] Provide a **no transparency** mode when system requests it or compositor blur is unsupported.
- [ ] Test text contrast in both themes; tune token values based on actual render captures.
- [ ] Use a monochrome icon set. Use shape and text as well as color to show status.
- [ ] Treat the platform's accent only as an optional theme input, not a required design color.

**Spacing scale:** `4 / 8 / 12 / 16 / 24 / 32 / 48 / 64` px, with tightly controlled `2` and `10` px exceptions for UI controls. Mirror Zeron's *scale discipline*, not its entire theme.  
**Radii:** `8 / 12 / 16 / 24 / pill`; use named tokens and derive nested radii as `outer radius - inset` when surfaces touch.  
**Borders:** usually 1 px device-aware hairline.  
**Ring:** selected card = wash + thin perimeter ring; keyboard focus = clearly stronger, separate outline.  
**Shadows:** rare, soft, and only when needed for layering.  
**Typography:** choose a legally usable modern sans-serif, with fallback. Test fonts on all platforms; do not assume Zeron's bundled fonts can be redistributed.  
**Motion:** 120–220 ms transitions, ease-out for interaction, no idle breathing animations in the first release. Respect reduced motion.

### 3.5 Component library plan

```text
UI COMPONENTS
├── AppShell
├── TitleBar / WindowControls
├── Surface {plain, raised, sunken, overlay}
├── Text {title, body, meta, caption}
├── Button {primary, secondary, ghost, destructive}
├── IconButton
├── Ring {selection, focus, pairing, progress}
├── DeviceTile {available, sleeping, offline, selected, untrusted}
├── DropZone {idle, hovering, sending, blocked}
├── TransferRow {queued, connecting, running, verifying, done, failed}
├── ProgressBar / CircularProgress
├── StatusDot + text label
├── Toast / InlineError
├── Dialog / Confirm / ConflictDialog
├── EmptyState
├── Skeleton
├── SearchField
├── SettingsRow / Toggle / Select
├── PathPickerRow
└── KeyboardShortcutHint
```

- [ ] Build components in `ui/ds/`, then expose from `ds/init.luau`.
- [ ] Create `ui/gallery.luau` with every state, both themes, glass/opaque comparisons.
- [ ] Store spacing, typography, color, radius, and motion values in named tokens.
- [ ] Do not hardcode screen-specific visual values except documented layout geometry.
- [ ] Add a lint or review check for hardcoded colors and sizes.
- [ ] Use keyed item updates for transfer rows; do not remount the list at every progress event.
- [ ] Test 100 simultaneous visual rows, rapidly changing transfer progress, and GPU idle behavior.

### 3.6 Desktop screens and text wireframes

**Main receiver (normal state)**

```text
┌─────────────────────────────────────────────────────────────────┐
│  ◌ SEAM                                    ─   □   ×             │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│          Receive files                                          │
│          Your computer is ready for trusted devices            │
│                                                                 │
│          ┌──────────────────────────────────────────┐           │
│          │             ◎  Ready                    │           │
│          │       Waiting for a shared file...      │           │
│          └──────────────────────────────────────────┘           │
│                                                                 │
│   Devices                                                      │
│   ◌ Pixel Phone                  Trusted · Nearby               │
│                                                                 │
│   Recent transfers                                              │
│   ✓ photo.jpg                    Just now       Open            │
│   ✓ document.pdf                 Earlier        Open            │
│                                                                 │
├─────────────────────────────────────────────────────────────────┤
│  Receiving: ON              Open folder            Settings ⚙    │
└─────────────────────────────────────────────────────────────────┘
```

**Active transfer**

```text
┌──────────────────────────────────────────────────────────┐
│  From Pixel Phone                                        │
│                                                          │
│  ▧  presentation.pdf                                     │
│     68%     13.6 MB / 20 MB       about 4 s left        │
│     ━━━━━━━━━━━━━━━━━━━━━━━░░░░░░░░░░                   │
│                                                          │
│  Verifying after download                  Cancel         │
└──────────────────────────────────────────────────────────┘
```

**Android share sheet follow-on**

```text
┌────────────────────────────┐
│  Send with SEAM            │
│  3 files · 18.4 MB         │
│                            │
│  Trusted computers         │
│  ◉ Workstation   Available │
│  ○ Laptop        Offline   │
│                            │
│  [ Send to Workstation ]   │
└────────────────────────────┘
```

**Pairing**

```text
┌──────────────────────────────────────────┐
│  Pair with Workstation                   │
│  Confirm these codes match on both       │
│                                          │
│       8  2  4  -  1  6  9                │
│                                          │
│  [ Cancel ]             [ Pair ]         │
└──────────────────────────────────────────┘
```

The numeric code above is **only a visual mockup**. The actual comparison method must be cryptographically tied to the secure handshake. A random display code with no authenticated handshake is not a secure pairing method.

### 3.7 Accessibility and interaction

- [ ] Keyboard navigation works on every desktop screen and dialog.
- [ ] Focus ring is distinguishable from the selection ring.
- [ ] Test VoiceOver on macOS, Narrator on Windows, Orca or supported AT on Linux.
- [ ] Label buttons and status indicators for screen readers.
- [ ] Minimum touch target is comfortable on Android, with 48 dp as the baseline.
- [ ] Respect OS text scaling, reduced motion, reduced transparency, and contrast preferences.
- [ ] Provide meaningful progress text (“68%, 13.6 of 20 MB”) alongside graphics.
- [ ] Avoid animation when the app is idle or when the user has reduced motion enabled.
- [ ] Support light and dark mode as first-class states, not invert filters.

---

## 4. Platform architecture

### 4.1 System overview

```text
                          ANDROID DEVICE
                  ┌─────────────────────────────┐
                  │ Third-party app Share menu  │
                  ├─────────────────────────────┤
                  │ Kotlin/Compose Share UI     │
                  │ Intents + ContentResolver   │
                  ├──────────────┬──────────────┤
                  │ Rust FFI     │ Android OS   │
                  ├──────────────┴──────────────┤
                  │ shared-core / protocol      │
                  │ discovery / pairing / send  │
                  └──────────────┬──────────────┘
                                 │
                        LAN, secure protocol
                                 │
                  ┌──────────────┴──────────────┐
                  │ Desktop Rust receiver       │
                  │ daemon / core / local IPC   │
                  ├──────────────┬──────────────┤
                  │ zgx host     │ OS adapters  │
                  │ Luau UI      │ tray, files  │
                  └──────────────┴──────────────┘
                     Windows / macOS / Linux
```

### 4.2 Chosen layers

| Layer | Proposed implementation | Role |
|---|---|---|
| Transfer core | Rust | Queue, streaming, hashing, retries, status |
| Network transport | Rust, QUIC/TLS candidate | Encrypted reliable file stream; benchmark before final selection |
| Discovery | mDNS/DNS-SD candidate, plus manual address/QR | Find reachable receiver on LAN |
| Pairing and trust | Rust, reviewed cryptographic libraries | Verified identity, trust store, session authorization |
| Desktop service | Rust | Background receiver, folder safety, local API |
| Desktop shell | zgx Rust host + Luau UI | Cross-platform windows and presentation |
| Android interface | Kotlin + Compose | Share targets, permissions, picker, notification |
| Android engine | Rust via JNI/UniFFI candidate | Shared transfer and protocol behavior |
| Persistent settings | Local SQLite or small atomic config, decide after spike | Devices, preferences, transfer history |
| Filesystem adapters | Per-platform Rust/OS APIs | Safe paths, save, open folder, notifications |
| USB later | Separate transport adapter | Never leak USB assumptions into core logic |

**Key choice:** There should not be a separate Android-only transfer protocol. Rust code must own protocol correctness on both sides.

### 4.3 Process model

- **Desktop service/agent:** owns listening sockets, trusted identities, queue, transfer state, and filesystem writes.
- **Desktop UI:** communicates with service over authenticated local IPC (prefer native mechanisms; validate platform options). Closing the window need not stop receiving if the user enabled background mode.
- **Android app:** owns system integration and presents the target selection. Rust engine works only under permitted Android lifecycle rules.
- **Security boundary:** UI supplies user intent; core validates authorization and safe file operations.

### 4.4 Constraints

- A public release must not depend on USB debugging or ADB.
- Background Android transfers must follow OS service/job rules. Android 15+ adds a `dataSync` foreground-service time limit; Android 16 also changes job quotas. Evaluate user-initiated data transfer jobs and applicable alternatives.
- Desktop firewall prompts, mDNS broadcast filtering, guest Wi-Fi isolation, VPNs, and missing Bonjour/Avahi can prevent discovery.
- zgx feature support and release packaging on all three desktop platforms are **validation tasks**.
- Android Share grants can be temporary. Stream the content while the grant is valid or stage it with explicit limits if recovery requires it.

---

## 5. Repository and folder structure

This is the proposed **real project tree**. It is not yet created. Some subfolders may be consolidated after a first working spike.

```text
seam/
├── README.md                         # Product summary and setup
├── LICENSE                           # Choose before public release
├── CONTRIBUTING.md
├── SECURITY.md                        # Reporting and threat model link
├── CHANGELOG.md
├── Cargo.toml                        # Workspace membership
├── Cargo.lock                        # Pinned exact dependency versions
├── rust-toolchain.toml               # Pinned stable toolchain
├── .gitignore
├── .editorconfig
├── .cargo/
│   └── config.toml                   # Target-specific build settings
│
├── docs/
│   ├── MASTER_PLAN.md                # THIS document
│   ├── PRODUCT.md                    # Personas, flows, scope
│   ├── ARCHITECTURE.md               # Runtime and data ownership
│   ├── PROTOCOL.md                   # Wire messages and versioning
│   ├── SECURITY.md                   # Threat model and crypto review
│   ├── ANDROID.md                    # Sharesheet, SAF, lifecycle
│   ├── DESKTOP.md                    # zgx, IPC, OS integrations
│   ├── USB_RESEARCH.md               # Separate feasibility report
│   ├── DECISIONS/
│   │   ├── 0001-local-first.md
│   │   ├── 0002-transfer-transport.md
│   │   ├── 0003-zgx-luau-ui.md
│   │   └── 0004-device-pairing.md
│   ├── UX/
│   │   ├── DESIGN_SYSTEM.md
│   │   ├── SCREENS.md
│   │   ├── COPY.md
│   │   └── ACCESSIBILITY.md
│   └── TESTING/
│       ├── DEVICE_MATRIX.md
│       ├── ACCEPTANCE.md
│       └── FAILURE_CASES.md
│
├── crates/
│   ├── seam-core/                     # Transfer state machine and queue
│   │   └── src/{lib.rs,job.rs,queue.rs,progress.rs,error.rs}
│   ├── seam-protocol/                 # Versioned envelopes and wire model
│   │   └── src/{lib.rs,message.rs,version.rs,codec.rs}
│   ├── seam-transport/                # Connection abstraction and LAN impl.
│   │   └── src/{lib.rs,traits.rs,quic.rs}
│   ├── seam-discovery/                # mDNS + manual pairing path
│   │   └── src/{lib.rs,mdns.rs,manual.rs}
│   ├── seam-identity/                 # Key storage, pairing, trust checks
│   │   └── src/{lib.rs,pairing.rs,trust.rs,certs.rs}
│   ├── seam-files/                    # File source/sink traits, path safety
│   │   └── src/{lib.rs,source.rs,sink.rs,safe_name.rs,verify.rs}
│   ├── seam-persistence/              # Settings, history, migrations
│   │   └── src/{lib.rs,config.rs,history.rs,migrations.rs}
│   ├── seam-platform/                 # Shared platform-facing interfaces
│   │   └── src/{lib.rs,notify.rs,open.rs,paths.rs}
│   ├── seam-ipc/                      # Desktop local control API
│   │   └── src/{lib.rs,request.rs,event.rs,auth.rs}
│   └── seam-test-support/             # Mocks, fixture files, fault injection
│       └── src/{lib.rs,fixtures.rs,network.rs}
│
├── apps/
│   ├── desktop/
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── main.rs                # zgx host, app lifecycle
│   │   │   ├── commands.rs            # UI bridge to local service
│   │   │   ├── service_client.rs
│   │   │   └── platform/
│   │   │       ├── windows.rs
│   │   │       ├── macos.rs
│   │   │       └── linux.rs
│   │   ├── ui/
│   │   │   ├── .luaurc
│   │   │   ├── app.luau
│   │   │   ├── gallery.luau            # Design component workbench
│   │   │   ├── ds/
│   │   │   │   ├── init.luau
│   │   │   │   ├── tokens.luau
│   │   │   │   ├── themes/{dark.json,light.json,opaque.json}
│   │   │   │   ├── text.luau
│   │   │   │   ├── button.luau
│   │   │   │   ├── surface.luau
│   │   │   │   ├── ring.luau
│   │   │   │   ├── device_tile.luau
│   │   │   │   ├── transfer_row.luau
│   │   │   │   └── drop_zone.luau
│   │   │   ├── screens/
│   │   │   │   ├── shell/{init.luau,title_bar.luau,sidebar.luau}
│   │   │   │   ├── receive/{init.luau,empty.luau,active.luau}
│   │   │   │   ├── send/{init.luau,select_files.luau}
│   │   │   │   ├── devices/{init.luau,pair.luau}
│   │   │   │   ├── history/init.luau
│   │   │   │   └── settings/{init.luau,appearance.luau,storage.luau}
│   │   │   ├── mocks/{devices.luau,transfers.luau}
│   │   │   └── tests/
│   │   │       ├── design_tokens.test.luau
│   │   │       └── transfer_rows_in_place.test.luau
│   │   ├── assets/{icons,images}
│   │   └── fonts/                     # Only licensed/redistributable fonts
│   │
│   ├── receiver-service/
│   │   ├── Cargo.toml
│   │   └── src/{main.rs,listener.rs,shutdown.rs}
│   │
│   └── android/
│       ├── settings.gradle.kts
│       ├── build.gradle.kts
│       ├── gradle.properties
│       ├── app/
│       │   ├── build.gradle.kts
│       │   └── src/
│       │       ├── main/
│       │       │   ├── AndroidManifest.xml
│       │       │   ├── java/.../seam/
│       │       │   │   ├── MainActivity.kt
│       │       │   │   ├── share/ShareActivity.kt
│       │       │   │   ├── share/ShareIntentParser.kt
│       │       │   │   ├── share/ShareItem.kt
│       │       │   │   ├── receive/ReceiveFlow.kt
│       │       │   │   ├── transfer/TransferCoordinator.kt
│       │       │   │   ├── transfer/TransferNotification.kt
│       │       │   │   ├── storage/ContentUriSource.kt
│       │       │   │   ├── pairing/PairingScreen.kt
│       │       │   │   ├── settings/SettingsScreen.kt
│       │       │   │   ├── bridge/RustBridge.kt
│       │       │   │   └── ui/{theme,components,screens}/
│       │       │   ├── res/xml/shortcuts.xml
│       │       │   ├── res/drawable/
│       │       │   └── jniLibs/       # Generated by build; not hand edited
│       │       ├── test/             # Unit tests
│       │       └── androidTest/      # Instrumentation tests
│       └── rust-bridge/
│           ├── Cargo.toml
│           └── src/{lib.rs,bindings.rs,android_io.rs}
│
├── integration-tests/
│   ├── end_to_end/
│   ├── pair_and_transfer/
│   ├── fault_injection/
│   ├── android_share/
│   └── fixtures/{tiny.txt,unicode-names.json}
│
├── packaging/
│   ├── windows/                      # Installer, signing, firewall notes
│   ├── macos/                        # .app, .dmg, signing/notarization
│   ├── linux/                        # tar.gz / packages / desktop entry
│   └── android/                      # APK/AAB signing and release config
│
├── scripts/
│   ├── bootstrap.sh
│   ├── check.sh
│   ├── build-android-rust.sh
│   ├── run-gallery.sh
│   └── package-all.sh
│
└── .github/workflows/
    ├── rust-ci.yml
    ├── android-ci.yml
    ├── desktop-builds.yml
    ├── security-audit.yml
    └── release.yml
```

> `{...}` in this **diagram** is a compact notation for multiple files/folders. Real directories must be expanded into normal files. This tree is the intended structure, not a claim that all folders already exist.

### Folder ownership rules

- [ ] `crates/` must not depend on zgx, Compose, or Android `Activity` types.
- [ ] `apps/desktop/ui/ds/` owns reusable styling and components. Screens use components and layout, not custom styling copies.
- [ ] `apps/android/` owns permissions and Android lifecycle; Rust does not request Android permissions directly.
- [ ] `seam-files` accepts abstract readable sources and writable sinks, not only POSIX file paths.
- [ ] `seam-identity` alone handles trusted keys and authentication decisions.
- [ ] Platform-specific integration is isolated behind small interfaces.
- [ ] UI test fixtures simulate progress without requiring a running network.
- [ ] Keep packaging scripts separate from production transfer code.

---

## 6. Transfer protocol and connection flow

### 6.1 Candidate wire protocol

A versioned application protocol over a secured LAN transport. **QUIC is a candidate**, not a final commitment until a desktop + Android spike passes. Use maintained, audited cryptography libraries; do not invent a cipher or authentication scheme.

```text
Discover
  └── Connect
      └── Authenticate trusted identity
          └── Request transfer
              ├── Receiver rejects → show reason
              └── Receiver accepts
                  └── Send manifest (filenames, sizes, types, IDs)
                      └── Stream bytes with backpressure
                          └── Receiver writes temp files
                              └── Verify size + strong content hash
                                  └── Commit files atomically where possible
                                      └── Acknowledge success
```

### 6.2 Message model (concept only)

| Message | Required information | Notes |
|---|---|---|
| `Hello` | protocol version range, device identity, capabilities | Reject incompatible versions safely |
| `PairRequest` | temporary session, identity proof | No unauthenticated trust by name alone |
| `PairConfirm` | user approval bound to handshake | Prevent silent trust |
| `TransferOffer` | job ID, item count, sizes where known | No unsanitized paths |
| `TransferAccept` | job ID, approval, capacity/limits | Could require desktop confirmation |
| `TransferReject` | job ID, safe reason code | Clear user message |
| `FileBegin` | item ID, safe name, size if known, hash strategy | Unknown stream lengths allowed if supported |
| `FileChunk` | job/item ID, offset, bytes | Limit chunk and memory sizes |
| `FileEnd` | item ID, final length/hash | Receiver verifies |
| `TransferCommit` | final per-item outcomes | Only then show success |
| `Cancel` | job ID | Idempotent |
| `ResumeRequest` | job ID, source identity, verified offset | Post-MVP if safe |

### 6.3 State machine

```text
NEW
 └── QUEUED
     └── DISCOVERING
         └── CONNECTING
             └── AUTHENTICATING
                 └── AWAITING_ACCEPTANCE
                     └── TRANSFERRING
                         └── VERIFYING
                             └── COMMITTING
                                 └── COMPLETED

Any active state → FAILED / CANCELLED / RETRYABLE
RETRYABLE → QUEUED (only when permitted and source still available)
```

- [ ] Define states and valid transitions in `seam-core`.
- [ ] Define machine-readable error categories and friendly UI messages.
- [ ] Enforce protocol version, item counts, byte limits, timeouts, and memory limits.
- [ ] Verify hash of each completed item, plus the final transfer manifest.
- [ ] Distinguish estimated remaining time from exact elapsed time.
- [ ] Use bounded queues and stream chunks; **do not load large files into RAM**.
- [ ] Handle unknown-length Android content streams.
- [ ] Reject duplicate or spoofed job IDs as appropriate.
- [ ] Preserve backward compatibility by adding fields and capabilities, not silently repurposing wire fields.

### 6.4 Filesystem safety rules

- [ ] Never trust a sender-supplied relative/absolute path.
- [ ] Sanitize filenames for Windows, macOS, and Linux (including reserved names, Unicode, traversal, and separators).
- [ ] Prevent symlink-based writes outside the selected destination.
- [ ] Write into a restricted temporary receive location and commit after validation.
- [ ] Default collision policy: create a safe new name, such as `photo (2).jpg`, not overwrite.
- [ ] Enforce configured maximum size and available disk checks.
- [ ] Clean up partial files after cancel/failure; preserve resumable partials only if encrypted/controlled and permitted.
- [ ] Avoid automatically opening or executing received files.

---

## 7. Security and privacy plan

### Threats

```text
Threat model
├── Untrusted sender on shared Wi-Fi
├── Fake device imitating a trusted computer
├── MITM during first pairing
├── Replay of transfer or pairing requests
├── Malicious file names and metadata
├── Resource exhaustion via huge files or many offers
├── Lost or stolen paired device
├── Local unprivileged process abusing desktop IPC
└── Logs leaking names, paths, or other private content
```

### Controls

- [ ] Document a real authenticated pairing protocol before coding the release handshake.
- [ ] Bind the shown verification code/QR to the underlying cryptographic identities and handshake transcript.
- [ ] Use established TLS/QUIC libraries, identity verification, and secure randomness.
- [ ] Persist trusted identities in OS-appropriate secure storage where practical.
- [ ] Give users a clear **Forget / Revoke device** action.
- [ ] Do not trust an IP address, mDNS name, or device nickname as identity.
- [ ] Require user approval for an unknown device; add user-configurable approvals for known devices.
- [ ] Authenticate desktop local IPC and limit its accessible functions.
- [ ] Limit offer rate, concurrent transfers, item count, size, CPU, memory, and storage use.
- [ ] Keep file names and transfer contents off telemetry by default; logs must not include sensitive paths without a deliberate debug mode.
- [ ] Confirm the exact protocol and crypto choices through security review before public beta.
- [ ] Run fuzz/property tests for protocol decoding, filenames, and manifest processing.

**Privacy promise:** Files go directly between devices on the local network in the first release. Do not claim “fully offline” when the selected transport requires a working LAN. The app should function without an external internet connection if the LAN is available.

---

## 8. Android implementation

### 8.1 Android integration tree

```text
Android app
├── Share entry points
│   ├── ACTION_SEND
│   ├── ACTION_SEND_MULTIPLE
│   └── MIME types: text, image, video, application files
├── Source reader
│   ├── ContentResolver.openInputStream / descriptor
│   ├── Read URI grant and lifecycle
│   ├── Filename / MIME / size metadata with fallbacks
│   └── Bounded, streamed reads
├── Destination picker
│   ├── Trusted devices
│   ├── Reachability / discovery
│   └── Send confirmation
├── Transfer coordinator
│   ├── Rust bridge
│   ├── Android lifecycle / service or job policy
│   ├── Notification and cancel
│   └── Success / failure screen
└── Optional receive mode
    ├── Consent
    ├── SAF / MediaStore output
    └── Verified file result
```

### 8.2 Tasks

- [ ] Create Android Studio project with Kotlin + Compose.
- [ ] Define manifest share intent filters for selected MIME types.
- [ ] Test Share appearance from Gallery, Files, Chrome, a PDF app, and a chat app.
- [ ] Parse ClipData, `EXTRA_STREAM`, and `EXTRA_TEXT` as applicable.
- [ ] Read incoming `content://` resources using grants rather than filesystem path guesses.
- [ ] Build Rust JNI/UniFFI binding spike on `arm64-v8a`; add other ABIs only if justified.
- [ ] Choose how to pass stream chunks or file descriptors between Kotlin and Rust without unnecessary copying.
- [ ] Add Activity recreation, process death, and permission-expiry handling.
- [ ] Test foreground transfer notification and notification permission behavior by OS version.
- [ ] Test Android 15+ `dataSync` service timeout handling and Android 16 job quota behavior.
- [ ] Evaluate user-initiated data transfer jobs where applicable.
- [ ] Implement trusted-computer selection and clear transfer confirmation.
- [ ] Add Android Direct Share shortcuts **only** after validating OS behavior and device-target semantics; users must still see the actual destination and consent as designed.
- [ ] Add optional in-app “Choose files” path using SAF.
- [ ] Add device forget, preferred computer, network help, and receive-folder preference when receiving is supported.

**Android-specific note:** A share target receives temporary access to content. Queueing work for much later is not automatically safe. If the original URI can no longer be read, ask the user to share again; do not claim that the file has been retained.

---

## 9. Desktop implementation and platform work

### 9.1 Common zgx desktop tasks

- [ ] Create minimal desktop Rust + zgx window from pinned framework revision.
- [ ] Confirm dynamic themes, basic blur, drag-and-drop, notification APIs, and native dialogs.
- [ ] Create `ui/ds/` token library and component gallery.
- [ ] Show real receiver state from local service over IPC.
- [ ] Keep transfer progress state reactive; throttle visual updates without dropping transfer data.
- [ ] Add controlled drop zone and file picker for desktop → Android later.
- [ ] Add tray/menu-bar controls when supported; provide graceful fallback.
- [ ] Store the selected receive folder and last-used target securely.
- [ ] Provide clear settings for start at login and run in background. Default must be transparent to user.
- [ ] Provide quit behavior that explains whether receiving stops.

### 9.2 Windows

- [ ] Test Windows 10/11 targets and MSVC Rust toolchain.
- [ ] Package executable and service/agent with a signed installer plan.
- [ ] Add start-at-login only with user consent.
- [ ] Handle Windows Defender Firewall prompts and private/public network choice safely.
- [ ] Verify standard Downloads and user-selected folder permissions.
- [ ] Handle Windows path restrictions and reserved filenames.
- [ ] Test high-DPI scaling and Narrator.

### 9.3 macOS

- [ ] Test Apple Silicon; assess Intel support as a release decision.
- [ ] Build `.app` and `.dmg` with correct bundled resources.
- [ ] Set up Developer ID signing, hardened runtime, and notarization before public distribution.
- [ ] Use a menu-bar receiver model where appropriate.
- [ ] Respect macOS file access prompts and sandbox choices.
- [ ] Test native blur and opaque mode on multiple macOS versions.
- [ ] Test VoiceOver and reduced transparency.

### 9.4 Linux

- [ ] Test at least one GNOME/Wayland and one KDE/Wayland setup; add X11 where feasible.
- [ ] Check mDNS with and without Avahi support.
- [ ] Support XDG Downloads and document directory conventions.
- [ ] Use desktop notifications and XDG portals where needed.
- [ ] Implement a receiver fallback when tray icons are unavailable or unreliable.
- [ ] Ship a tested tarball or AppImage/Flatpak/deb/RPM option only after validation; choose an initial packaging format.
- [ ] Test accessibility with a supported Linux screen reader.

### 9.5 Cross-platform parity gate

- [ ] Same pairing identities and protocol version on all platforms.
- [ ] Same file hashes and results for the same test vectors.
- [ ] Same confirmation/error meanings, even if OS dialogs differ.
- [ ] App works with the desktop window closed when background receiving is enabled.
- [ ] Clean shutdown releases ports and partial files safely.

---

## 10. USB and wired transfer: separate research track

**Product intent:** A cable should feel as simple as a network transfer. **Engineering reality:** Android MTP, USB tethering, ADB, and USB accessory modes are different mechanisms with different OS support and permission requirements.

### Candidate evaluation

| Method | Android Share → desktop directly? | Consumer-friendly? | Plan |
|---|---|---|---|
| MTP/PTP | **Not by itself**; desktop primarily browses phone storage | Common | Separate desktop file-browser feature if justified |
| USB tethering / USB networking | **Possible** if both devices receive a usable network route | Depends on phone/OS setup | Best first wired feasibility spike |
| ADB reverse/forward | Technically useful for development | **No**, requires debugging | Dev-only testing tool |
| Android Open Accessory / USB host/gadget variants | Hardware/OS-dependent | Uncertain | Research only, no universal support claim |
| Wi-Fi LAN | Yes | Usually easiest | **MVP baseline** |

### USB gates

- [ ] Test a representative Android phone + Windows 11 + USB networking, using an actual Share intent.
- [ ] Repeat with macOS and Linux, documenting user interaction and required drivers.
- [ ] Determine whether computer discovery can detect a USB network route without confusing it with Wi-Fi.
- [ ] Compare setup steps and reliability against local Wi-Fi on each OS.
- [ ] Decide if a wired transport can be added behind the same `Transport` trait.
- [ ] Do not advertise “plug in and share” until a **non-developer** can complete the action on supported phones and OS versions.
- [ ] If USB is inconsistent, state supported combinations and provide a clear fallback rather than hiding limitations.

**Decision gate:** Implement a consumer wired mode only when at least one documented route works without debugging, root, or a custom kernel driver on the selected supported platforms.

---

## 11. Engineering quality and tests

### 11.1 Test pyramid

```text
Quality
├── Rust unit tests
│   ├── state transitions
│   ├── safe filenames and path policy
│   ├── message codec and version negotiation
│   ├── queue/cancel/retry
│   └── hashes and file integrity
├── Property and fuzz tests
│   ├── malformed manifests
│   ├── Unicode paths
│   └── unexpected frame sizes
├── Integration tests
│   ├── Rust sender → Rust receiver (localhost)
│   ├── simulated packet loss and interruptions
│   └── partial file and disk-full handling
├── Platform tests
│   ├── Android intent and URI grants
│   ├── Windows installer and firewall
│   ├── macOS sign/notarize and folder access
│   └── Linux portals/mDNS and desktop environment
└── End-to-end manual matrix
    ├── Android → Windows
    ├── Android → macOS
    ├── Android → Linux
    └── Desktop → Android (later)
```

### 11.2 Test files

- [ ] Empty file.
- [ ] 1-byte file.
- [ ] 100 KB, 25 MB, 1 GB, and 10 GB files (when target storage supports them).
- [ ] 1,000 small files in a single share batch, if supported by source app.
- [ ] Unicode filename, emoji, long filename, reserved Windows name.
- [ ] Image, video, PDF, ZIP, text, and unknown MIME type.
- [ ] Two files with the same filename.
- [ ] Source with unknown size.
- [ ] Source that becomes inaccessible during transfer.

### 11.3 Reliability targets (proposed; measure before release)

| Metric | Initial target | Verification |
|---|---|---|
| Data integrity | **100%** of completed files match source hash | Automated hashes |
| Local pairing | Complete in under 1 minute in a normal LAN | Usability test |
| Daily share taps | Prefer ≤2 after OS Share selection | Usability test |
| Discovery | Common home LAN discoverable without manual IP | Device matrix |
| Memory | Bounded during multi-GB streaming | Stress test |
| Cancel | Cancels promptly and removes unsafe partials | Fault test |
| Idle background UI | No continuous animation or unnecessary repaint | Profiler |
| Accessibility | All critical actions reachable and labeled | Platform audits |

The targets are goals, not observed performance claims.

### 11.4 CI/release checks

- [ ] `cargo fmt --check`.
- [ ] `cargo clippy` under a documented warning policy.
- [ ] Rust unit + integration tests.
- [ ] Android lint, Kotlin tests, APK smoke build.
- [ ] zgx/Luau syntax and design tests on pinned framework.
- [ ] Windows/macOS/Linux compile jobs.
- [ ] Dependency audit, license inventory, and secret scan.
- [ ] Release artifact hashes, checksums, and signed artifacts.
- [ ] Smoke-test installer/uninstaller and upgrade path.

---

## 12. Phased implementation plan and checklist

### Phase 0 — Decisions, environment, prototypes

**Goal:** Prove the stack is workable, and reduce risk before building UI polish.

- [ ] Create Git repository and put this plan at `docs/MASTER_PLAN.md`.
- [ ] Choose license after reviewing zgx and dependency licenses.
- [ ] Select min Android version and target SDK based on actual test device access.
- [ ] Choose initial supported desktop OS versions and architectures.
- [ ] Decide Rust desktop + Luau UI approach after building zgx `gallery`.
- [ ] Pin Rust, Android Gradle Plugin, Java, and zgx version/commit.
- [ ] Verify a basic zgx window on Windows, macOS, and Linux.
- [ ] Verify an Android app can load a minimal Rust library.
- [ ] Make a two-device proof of concept: raw content stream over a local encrypted candidate transport.
- [ ] Record findings in short architecture decision records.

**Done when:** All four platforms can build their basic shell, and a minimal Rust sender and receiver exchange verified bytes over LAN.

### Phase 1 — Core engine vertical slice

**Goal:** One safe transfer, without any final UI.

- [ ] Define transport and file source/sink interfaces.
- [ ] Define protocol version and message types.
- [ ] Implement client/server connection and stream.
- [ ] Implement bounded memory and file chunking.
- [ ] Implement hash and receiver verification.
- [ ] Implement cancel and basic error codes.
- [ ] Build `seam-send` and `seam-receive` developer CLI fixtures.
- [ ] Test 1-byte to multi-GB payloads with exact hash checks.

**Done when:** CLI-to-CLI file transfer works safely in both directions on the same local network, before any final GUI exists.

### Phase 2 — Android Share → desktop proof

**Goal:** The core product journey becomes real.

- [ ] Add Android Share entry and MIME/intent parser.
- [ ] Read content URIs with stream-based IO.
- [ ] Bridge Android stream to Rust transfer core.
- [ ] Build temporary desktop receiver CLI.
- [ ] Let Android select a manually configured destination first.
- [ ] Transfer shared photo and PDF with verification.
- [ ] Handle cancellation and one meaningful error state.

**Done when:** A user selects Share in a real Android third-party app, chooses SEAM, and the correct file appears on the PC.

### Phase 3 — Discovery, pairing, and trust

**Goal:** Remove manual IP setup without reducing security.

- [ ] Implement discovery and fallback manual address/QR.
- [ ] Implement verified pairing protocol.
- [ ] Implement trusted-device store and revocation.
- [ ] Add receiver approval policy.
- [ ] Show reachable/offline states accurately.
- [ ] Test fake sender / replay / bad signature cases.

**Done when:** A new user can pair without terminal commands, and an unpaired sender cannot deliver files silently.

### Phase 4 — zgx desktop design system + receiver

**Goal:** A polished, useful desktop experience.

- [ ] Create design tokens and light/dark/opaque themes.
- [ ] Implement Zeron-inspired glass surface, ring, rounded corners, hover wash, button variants.
- [ ] Implement component gallery and screenshot baselines.
- [ ] Create desktop receiver service, safe receive folder, and IPC.
- [ ] Implement app shell, paired devices, transfer list, active progress, settings.
- [ ] Implement native file notifications and “Open folder”.
- [ ] Add system tray/menu-bar model with a tested fallback.
- [ ] Ensure UI stays responsive during big transfers.

**Done when:** Closing the visible UI does not lose active work if background receiving is enabled, and progress/finished states are correct.

### Phase 5 — Android polish and fast target selection

**Goal:** Make the Share menu experience feel effortless.

- [ ] Implement Compose destination selection, paired devices, offline help, and result screen.
- [ ] Save preferred target securely.
- [ ] Explore Android Direct Share/Sharing Shortcuts after compatibility review.
- [ ] Implement safe long-transfer lifecycle/notification behavior.
- [ ] Add progress and cancel action.
- [ ] Test portrait, landscape, large fonts, and accessibility.

**Done when:** A repeat user can send to a preferred trusted desktop in one or two taps after choosing SEAM in the Share menu.

### Phase 6 — Three desktop OS releases

**Goal:** Identical core behavior with correct native integration.

- [ ] Windows installer, firewall guidance, and autostart option.
- [ ] macOS bundle, signing/notarization, and menu-bar behavior.
- [ ] Linux package and portal/notification integration.
- [ ] End-to-end Android → each OS on real devices.
- [ ] Test update and uninstall behavior, including trust data handling.

**Done when:** A non-developer can install, pair, and receive on each advertised OS.

### Phase 7 — Desktop → Android and reliability

**Goal:** Two-way sharing and robust recovery.

- [ ] Desktop drop zone and file picker.
- [ ] Android incoming-file consent and safe save with SAF/MediaStore.
- [ ] Queue multiple files, show per-file outcome.
- [ ] Define and implement safe retry or resume where feasible.
- [ ] Test all failure scenarios in Section 2.
- [ ] Add transfer history that users can clear.

**Done when:** Bidirectional transfers work and common interruptions do not cause silent data loss.

### Phase 8 — Public beta

**Goal:** Real users can trust the product.

- [ ] Run security review and fix critical/high findings.
- [ ] Run accessibility review, including opaque and reduced-motion modes.
- [ ] Publish clear privacy policy and permission explanations.
- [ ] Run competitive tap-count and reliability usability tests.
- [ ] Add issue templates, crash diagnostics with opt-in privacy protection, and reproducible bug reports.
- [ ] Build signed checksummed beta artifacts, write release notes.
- [ ] Collect feedback and prioritize problems over cosmetic features.

**Done when:** Public beta passes release checklist and there are no known silent-corruption, unauthenticated-acceptance, or destructive-overwrite defects.

### Phase 9 — Wired USB investigation

**Goal:** Validate whether USB can be better than Wi-Fi for ordinary users.

- [ ] Complete USB method study from Section 10.
- [ ] Build USB networking transport prototype for each desktop OS.
- [ ] Document Android device support and exact setup taps.
- [ ] Add route preference only when a wired path is confirmed reachable.
- [ ] Add user-facing USB flow only after real-device testing.

**Done when:** Supported cable workflows are repeatable and clearly documented; unsupported paths show a Wi-Fi fallback.

---

## 13. Product copy and interaction rules

### Clear, short system messages

| State | UI text |
|---|---|
| Idle desktop | “Ready to receive” |
| Device not found | “Computer not found. Check that both devices use the same local network.” |
| User pairing | “Confirm this code on both devices.” |
| Sending | “Sending 2 of 3 files” |
| Finishing | “Verifying file” |
| Success | “Sent to Workstation” |
| Rejected | “Workstation did not accept this transfer.” |
| Storage full | “Not enough space on Workstation.” |
| Source lost | “This file is no longer available. Share it again.” |
| Not trusted | “Pair this device before sending.” |

- [ ] One short sentence per error, plus an optional “Details” link.
- [ ] Use exact device name in approval and completion messages.
- [ ] Never imply encryption is absent if it is used; avoid security claims not verified by test.
- [ ] No marketing copy in transfer dialogs.
- [ ] Give user actions clear verbs: **Send**, **Cancel**, **Retry**, **Open folder**, **Forget device**.

---

## 14. Risks and mitigation

| Risk | Impact | Early mitigation |
|---|---|---|
| zgx API churn | Desktop app breaks during updates | Pin revision, run gallery smoke tests, upgrade intentionally |
| zgx feature differences across OS | Inconsistent blur/tray/window behavior | Phase 0 parity spike, opaque and tray-free fallback |
| Android `content://` URI lifetime | File no longer readable | Stream promptly, handle expired grants, stage only with consent |
| Android background restrictions | Long transfer interrupted | Use permitted foreground/job model, test versions |
| mDNS blocked | Devices cannot find each other | Manual IP/QR fallback, network troubleshooting |
| Host firewall | Receiver unreachable | Install-time documentation, explicit user guidance |
| Untrusted network peer | Privacy or security failure | Authenticated pairing and authorization |
| Malicious filename | File overwrite / traversal | Strict safe-name and atomic-save policy |
| Large files | Memory and disk pressure | Streaming with backpressure and limits |
| USB differences | Failed “just plug it in” promise | Keep USB gated and optional |
| Overdesign | Pretty but unusable app | First complete transfer before animation polish |
| Licensing/font reuse | Legal distribution trouble | Create own assets and audit dependencies/fonts |

---

## 15. Release definition and metrics

### Minimum product quality gate

- [ ] Verified end-to-end Android → Windows transfer.
- [ ] Verified end-to-end Android → macOS transfer.
- [ ] Verified end-to-end Android → Linux transfer.
- [ ] No account required for LAN transfer.
- [ ] Pairing is secure and explicit.
- [ ] No confirmed file becomes silently corrupted.
- [ ] No incoming file silently overwrites an existing file.
- [ ] Transfer errors remain visible and actionable.
- [ ] Idle desktop app has no unnecessary constant UI animation.
- [ ] UI works with blur disabled.
- [ ] Keyboard, screen reader, and text scale checks passed.
- [ ] Install and uninstall tested on advertised platforms.
- [ ] Permissions, privacy, and source licenses documented.

### Metrics to collect (with privacy protections)

- Time from tapping **Share → SEAM** to receiver save confirmation.
- Number of taps required after initial pairing.
- Pairing success rate during controlled testing.
- Transfer completion rate under defined Wi-Fi conditions.
- Mean time to recover from temporary network failure.
- Idle memory/CPU/GPU usage per platform.
- Crash rate (from opt-in reports or local QA tests).

Do not collect filenames, file contents, or personal device identifiers as anonymous telemetry.

---

## 16. Decision log

Use this table for quick state, then create a detailed ADR in `docs/DECISIONS/` for each major choice.

| ID | Decision | Status | Why |
|---|---|---|---|
| D-001 | Local LAN is the MVP data path | **Proposed** | Best common denominator across all OS platforms |
| D-002 | Rust shared core for protocol | **Proposed** | One implementation of integrity and security |
| D-003 | zgx + Luau desktop interface | **Proposed** | Fits Zeron's pattern; needs three-OS spike |
| D-004 | Kotlin + Compose for Android system UI | **Proposed** | Native Share, permission, and lifecycle APIs |
| D-005 | QUIC/TLS secure transport | **Candidate** | Evaluate library maturity, binary size, Android behavior |
| D-006 | mDNS discovery + manual/QR fallback | **Candidate** | Common local discovery plus robust fallback |
| D-007 | Unknown devices require approval | **Proposed** | Explicit user trust |
| D-008 | Never overwrite incoming files by default | **Proposed** | Prevent data loss |
| D-009 | USB is a separate later-phase track | **Proposed** | Consumer setup differs by device and OS |
| D-010 | “SEAM” is a temporary name | **Open** | Branding and trademark review required |
| D-011 | Desktop receiver process lifetime | **Open** | Decide agent, service, IPC, and autostart policy by OS |
| D-012 | License, release channels, architectures | **Open** | Must be selected before beta |

### First decisions to resolve

- [ ] Which Android minimum API and ABI targets will we support?
- [ ] Which desktop OS version baselines can we actually test?
- [ ] Does QUIC pass the Android performance and packaging spike, or should we choose a simpler secure transport?
- [ ] Which validated pairing protocol and credential storage do we use?
- [ ] Which OS-neutral receive-location and history storage APIs do we need?
- [ ] Which font license and icon library fit our distribution?
- [ ] Should incoming transfers from trusted devices require an approval prompt by default, or allow a clearly opted-in auto-receive mode?

---

## 17. Sources and research notes

These are the external references used for the **framework and platform facts**. The colors, architecture, screen sketches, and milestones above are **our proposed design**, not excerpts from these sources.

### zgx and Zeron

1. **zgx repository and README** — [github.com/zeronsh/zgx](https://github.com/zeronsh/zgx). Official overview of retained-mode signals, Rust/Luau, desktop platforms, paint, layout, accessibility, OS hooks, and its young API.
2. **Zeron repository** — [github.com/zeronsh/zeron](https://github.com/zeronsh/zeron). Official product/source reference.
3. **Zeron website** — [zeron.sh](https://zeron.sh/). Public description of dark/light themes and frosted native windows.
4. **Zeron-on-zgx implementation notes** — [Zeron zgx README mirror](https://upd.dev/zeronsh/comet/src/commit/2d63bdb2abb34ac949f41afbc7b11715280693a7/apps/zgx/README.md). Details design tokens, `ui.variants`, 4–64 spacing scale, concentric corners, even insets, selected ring, live updates, `ui/ds/`, gallery, and Zeron's `#080808` dark glass scrim. This is a point-in-time source; verify against the current repository before implementing.
5. **Zeron contribution guidelines** — [CONTRIBUTORS.md](https://github.com/zeronsh/zeron/blob/main/CONTRIBUTORS.md). Guidance on native UI consistency and checking light/dark and frost/opaque states. It is Zeron's project policy, not our project's obligation.

### Android

6. **Receive data from other apps** — [Android Developers](https://developer.android.com/develop/ui/compose/sharing/receive). Share targets, `ACTION_SEND`, MIME types, data extraction, and permission-aware handling.
7. **Storage Access Framework** — [Android Developers](https://developer.android.com/guide/topics/providers/document-provider). Standard user-mediated document access, local/cloud providers, and file picking.
8. **Direct Share targets** — [Android Developers](https://developer.android.com/develop/ui/compose/sharing/direct-share-targets). Sharing Shortcuts, OS restrictions, and shortcut publishing requirements.
9. **Foreground service changes** — [Android Developers](https://developer.android.com/develop/background-work/services/fgs/changes). Background data-transfer policy changes across Android versions.
10. **Foreground service timeouts** — [Android Developers](https://developer.android.com/develop/background-work/services/fgs/timeout). Android 15 data synchronization service limits.

### Research safeguards

- [ ] Check every API against the **pinned** zgx revision before copying a code example.
- [ ] Check Android's latest target-SDK and background-transfer policies immediately before release.
- [ ] Verify platform-specific native behavior on real machines, not only in a README.
- [ ] Inspect licenses for any code, fonts, imagery, or icons reused from other projects.
- [ ] Keep a dated entry here when major assumptions change.

---

## 18. First work session: exact order

If starting implementation today, do only the items below before adding visual polish:

1. [ ] Create repository with `docs/MASTER_PLAN.md`, Cargo workspace, and Android Studio project.
2. [ ] Pin one zgx revision and make a basic window appear on your main desktop OS.
3. [ ] Make a test Android APK appear as a Share target for a PDF or image.
4. [ ] Make Kotlin open the shared `content://` stream without copying the whole file into memory.
5. [ ] Make a Rust receiver CLI accept an encrypted stream and save it safely.
6. [ ] Connect Android sender to the receiver using a manually entered address.
7. [ ] Verify the transferred file's content hash and show success only after confirmation.
8. [ ] Record known failures and only then proceed to discovery, secure pairing, and polished zgx UI.

### Definition of the first visible success

> Share a photo on an Android phone → choose SEAM → enter or select a test receiver → receive the exact photo on a desktop → verify matching file hash.  
> No cloud, no account, no fake progress. The desktop UI can still be plain at this point.

---

**Project principle:** The interface should feel almost invisible. The transfer must be secure, predictable, and real. We can make glass and rings beautiful after the bytes arrive safely.
