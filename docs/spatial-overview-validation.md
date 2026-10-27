# Spatial overview implementation — 2026-10-27

This supersedes the ribbon presentation described in `metal-preview-performance.md`.
The screenshot fallback remains removed. CGS is pinned to an immutable Git revision;
a sibling checkout is no longer required to build Rift.

## Reference and deliberate changes

The 23.6167-second reference was inspected as a 10 fps sequence and at its focus
transition. It shows real window geometry over stationary wallpaper, continuous
camera scaling (approximately 14–100%), small glass application badges and a compact
bottom-left zoom control. A fixed wallpaper patch differed by only 0.158/255 mean
absolute pixel value across sampled states. Zoom settles monotonically in roughly
0.55 seconds; focus return takes roughly 0.3 seconds. No bouncy animation was added.
The recording does not establish keyboard, cross-workspace drop or multi-monitor
semantics.

At the user's direction, workspace regions are stacked vertically rather than
reproducing the reference's original region placement. Scrolling workspaces align
their remembered focused windows. Shared mode shows the entire shared catalog on
the display where the overview opens, retaining actual workspace/display ownership.
Independent mode retains each display's own workspace context. Native Niri workspace
and overview documentation/source informed the distinction between monitor context
and shared overview progress; the shared catalog behavior follows the user's explicit
choice rather than imposing Niri's per-monitor workspace inventory.

## Implementation

- One hosted surface and shared Metal camera per overview display. Persistent window
  identities/world geometry; camera movement does not move real windows or rebuild
  capture inventory. Inverse-camera picking and pointer-pivot zoom use the same math.
- Stationary native wallpaper asset, or asynchronous desktop-only SCK snapshot if its
  file is missing. Real previews, restrained shadows, application/group badges,
  percentage/control strip. Application icons are retained rather than rediscovered
  during drag. Desktop snapshot is not a window-preview fallback.
- Existing semantic reactor drop operations handle rearrangement and workspace moves.
  A cached-frame drag ghost remains until acknowledgment, with bounded edge scrolling.
- Window activation commits workspace and window focus together, resolves the final
  scrolling frame before native focus acknowledgment, and runs one bounded camera
  transition. The covered native layout is installed without its competing animation.
  Handoff waits for matching PID, ordered-in state, native bounds and presentation.
  A two-second deadline bounds failures. Remote-owner activation returns the host to
  its desktop and follows existing reactor display/focus policy.
- Canvas panning uses absolute pointer displacement from the press camera, including
  right drag over windows. Empty-canvas release cannot activate a window underneath.
  Reversal does not accumulate device-delta error. Input outside the hosted canvas
  passes through; captured drag/release remains coherent across its boundary.
- Arrow/tab navigation, enter/escape, smooth wheel/pinch camera control and zoom keys
  share the existing input infrastructure. Configured zoom-key bindings take priority.
- Capture policy is independent of presentation: selected visible sources request up
  to 60 fps, other visible sources 15 fps, distant sources snapshots. Entry bootstraps
  at overview resolution rather than reserving full Retina sources before zoom-out.
  Producer lifetime is separate from the bounded paused renderer/frame cache.
- CGS caches immutable source identity and imported texture dimensions, eliminating
  repeated Objective-C metadata getters from reconciliation. Cached drawable camera
  transforms move immediately and atomically rebase when new GPU pixels arrive.

## Validation and measurements

877 Rift library tests pass, including a new regression that failed before final
scrolling target resolution was fixed. Existing native-input tests now distinguish
absolute pointer coordinates from unrelated raw deltas and cover crossing the host
boundary and releasing capture. The real shipped Metal shader passes GPU readback
for transforms, clip, corners and premultiplied stacking. All-target checks and
optimized builds pass. After switching from the local dependency to the immutable
Git dependency, one parallel test run aborted inside Apple's
`SLSWindowManagementFallbackBridge` weak-reference handling; all 877 tests passed
when rerun serially. Earlier parallel runs passed too, so this intermittent native
framework/test concurrency failure remains unresolved. Exact logs/artifacts remain
local under `/tmp` because native screenshots contain private application content.

A representative native main-thread sample before capture metadata caching had
898/1019 samples in capture pumping/reconciliation (about 88%). After the combined
shared-host and metadata changes, 1316/1396 samples were waiting in the event loop
(about 94%). These are different host/workload snapshots, not an isolated causal
speedup or an end-to-end latency benchmark. In the latter snapshot, renderer CPU p50
was about 0.241 ms and GPU p50 about 0.181 ms. Static presentation interval is not an
animation refresh-rate result.

The native cached-camera probe verifies movement, rebase and reversal with zero
intervening GPU submissions. Native no-source testing verifies the camera clock
continues with zero captured frames/GPU submissions/jobs. One matched one-window
before/after trial measured camera setter p50 0.001/0.082 ms: the new setter performs
CA work to move pixels immediately. It does not establish a p50/p95/p99 visual-latency
improvement. The larger alternating benchmark was interrupted by session locking;
those failed trials were discarded.

On two native displays, a prior candidate successfully rearranged two fixture windows
and dropped one into a workspace owned by the other display without changing owners
or switching active workspaces. Shared-catalog screenshots verify one overview host
and the other display left visible. These preceded the latest focus/pan fixes. The
latest visual run stopped before sending gestures because the session was locked
and Rift reported zero visible displays. The new click transition, pan reversal and
release behavior still require visual confirmation on an unlocked desktop.

## Limits and remaining work

No complete 60/120 Hz HID-to-hardware-present comparison, 1/5/10/20/40-window sweep,
repeated-session memory-growth result or full sleep/lock/reconnect/fullscreen/native
Spaces matrix is claimed. No VM guest is provisioned. Physical pinch behavior is not
yet visually certified. The demo recorded before the latest fixes is not acceptance
proof for the current build. Hover traffic-light chrome from the reference remains
unimplemented. Cross-display focus cannot be an identical single-screen zoom handoff.
Very large zoomed-in sources can still hit capture admission limits; cached frames
remain usable, but adaptive budget allocation needs further native evaluation.

The implementation does not meet the desired net LOC reduction: the initial rewrite
adds roughly 800 net Rift lines, plus approximately 324 CGS lines including benchmark
and documentation changes. Persistent geometry and lifecycle policy replaced ribbon
projection/card logic, but retained window drag, caching and native handoff still need
simplification after native correctness is established. No render graph, texture
atlas, compute pass, separate capture engine or new user configuration mode was added.
