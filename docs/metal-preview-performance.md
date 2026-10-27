# Metal preview work — 2026-10-27

The screenshot fallback was removed at the user's explicit request. The current
Mission Control layout and its configuration/CLI/IPC remain unchanged. This work
adds source/rendering primitives, not the proposed new Mission Control design.

## Bottlenecks and the scrolling fix

Source inspection found duplicate `request_previews`/`sync_metal` submissions,
full scene allocation/style/projection work during input, repeated capture inventory
bookkeeping, two CGS presentation windows per display, and an owner presentation
handoff after GPU completion.

The visible scroll mismatch had a structural cause: captions moved in CA while a
display-wide preview canvas needed a new GPU frame to move one workspace. Metal
work itself was small. A native diagnostic measured encoding/GPU/owner handoff at
roughly 0.14/0.12/0.013 ms p50, versus 33 ms submission-to-hardware-presentation.

Each workspace now has a canvas inside its existing clipped CA layer. The canvases
share one capture inventory and the overview's single CGS surface. Vertical motion
moves the parent; horizontal motion translates only the workspace's cached canvas.
Both happen in the input CA transaction. A GPU replacement rebases without moving
the image. Stable scrolling updates retained geometry without another projection
allocation. Dragging likewise moves its cached canvas and can reuse a source frame
across displays without a pixel copy.

Native WindowServer screenshots taken with the owner held between input and capture
verify independent horizontal movement, shared vertical movement, rebase and immediate
reversal. The movement is visible with zero intervening GPU submissions. An attached
coordinate flip and a shared-wake stall were caught and fixed by these probes.

## Changes by subsystem

| Subsystem | Result |
| --- | --- |
| Capture | CGS is the sole owner. Separate source policies; incremental updates; independent scene/camera state; one shared mailbox drain across canvases. |
| Quality/activity | Projected physical size retained; 60 fps selected/dragged, 30 fps other live sources; snapshots for hidden sources; asymmetric resolution hysteresis and asynchronous paused-snapshot upgrades. |
| Scheduling | Foreground fresh work precedes prefetch. Eight admitted sources per owner, sixteen jobs and 512 MiB active reservations process-wide; two jobs reserved against speculative snapshots. |
| Memory | Retained samples pin GPU reads. Open frame cache capped at 128 MiB per owner; closed thumbnail cache 32 MiB; paused presentation contexts/cache reserved within 256 MiB. |
| Hosting | One CGS window and CAContext per display; reusable attached canvases. Existing standalone CGS API preserved. |
| Presentation | Display-link drawables, two in-flight commands per canvas, scheduled owner handoff; no command-retention list, no production synchronous GPU/drawable waits. |
| Input | Cached CA movement; dirty scene reconstruction only at boundaries/display updates; affected hover/selection entries updated directly; duplicate submissions/framework palette lookups/nested UI transactions removed. |
| Fallback removal | Deleted Rift's SCK session/discovery/result channel/scheduling, CGImage pixel copying/resizing, screenshot cache and direct SCK dependency. Icons/captions remain. |
| Reuse | Shared camera, existing per-item matrices/transitions/clipping/effects, stable frame handles and texture access. No workspace policy in CGS. |

## Measurement

The full CGS report and raw records are in `cgs/docs/metal-preview.md` and
`cgs/benchmarks/metal-preview-2026-10-27.json` in CGS validation commit `e527d89`. The
native renderer workload used three alternating trials per side with matching
standalone release profiles/locks. Initial profile-mismatched numbers were rejected.
The results do not support a general cold-start or CPU-encoding speedup: cold workloads
remained variable (the five-window attached workload regressed, other sampled
medians improved). Ten/twenty-window capture-to-present p95 improved about 9–10%.
The main demonstrated latency improvement is synchronous cached geometry movement.

Medians across three matched trials per side (before → after):

| Windows | First hardware presentation, ms | Capture-to-present p95, ms | Measurement RSS, MiB |
| --- | --- | --- | --- |
| 1 | 369.0 → 336.8 | 42.74 → 44.21 | 68.0 → 68.1 |
| 5 | 458.0 → 591.6 | 43.43 → 44.08 | 83.8 → 83.4 |
| 10 | 577.4 → 534.0 | 53.18 → 48.62 | 103.3 → 103.3 |
| 20 | 625.6 → 613.9 | 53.76 → 48.28 | 141.5 → 141.4 |

These renderer fixtures do not establish end-to-end HID input latency or whole-Rift memory use.

The existing real-window capture harness captured all eleven managed windows in
all trials and returned active frame bytes to zero after hide. Cached availability,
hardware presentation and fresh foreground capture are recorded separately. Its
initial before/after integration runs used different build feature/profile contexts,
so their CPU/RSS are not claimed as a controlled comparison.

No HID-input-to-photon p50/p95/p99, complete 60/120 Hz certification, exact power or
WindowServer-private allocation claim is made. Callback-to-owner time is not input
latency. Presentation interval samples include intentional idle gaps.

## Validation and lifecycle

- Rift: all 875 library tests passed with one test thread. An initial parallel run
  had two input binding-mode failures; the isolated 26-test input suite and full
  serial suite passed. Those tests were not changed.
- CGS: shipped-metallib pixel readback covers camera, transforms, clipping, corners,
  stacking and premultiplied alpha.
- Native lifecycle tests cover parked/offscreen sources, transitions/reversal,
  snapshot idle, priority changes without GPU submission, retained pixels during
  snapshot resolution upgrades, resize/destruction, pending-work teardown and reopen.
- Shared-canvas native probe: two canvases, five shared streams; shutdown reached
  zero process capture jobs/reservations. It also works when only canvases render.
- No producer, capture stream or display clock is intentionally retained to preserve
  pixels. The bounded thumbnail/drawable caches do retain memory. Caller-held frame
  handles and in-flight GPU samples can outlive active-source accounting.
- No retain cycle was introduced: capture callbacks use closed mailboxes/weak
  discovery waiters, scheduled presentation uses weak state and epochs, and retained
  source samples are released on command completion. CA mutations stay on the owner.

## Rejected/deferred work and limits

- Command-buffer `presentDrawable` is incompatible with the retained transactional
  presentation contract. No blind vsync/drawable-count/frame-latency changes were kept.
- No render graphs, compute hashing, indirect commands, atlases or extra dependencies
  were introduced. Source pixel copies were removed rather than optimized.
- Independently movable canvases add drawable pools, potentially about 1.5 display
  areas for three visible workspace rows, plus an on-demand drag canvas. Capture
  streams are shared. This is the memory tradeoff for immediate independent motion;
  cache reservations account for it. A future shared camera can use one canvas when
  content moves together.
- No VM is provisioned. Native fullscreen/Spaces, multiple displays/reconnects,
  locked sessions, sleep/wake and permission revocation still need their real native
  matrix. The host's permission settings and display topology were not changed.
- Some static CA source fixtures continue emitting SCK Complete buffers; zero work
  for that live fixture is not claimed. Snapshot idle and close are asserted to stop.
- Future arbitrary per-window CA captions must share their rendered geometry/parent
  transform or be coordinated with presentation. This is not the new UI implementation.

## Builds and code size

Rift must use the exact committed CGS revision exposing the new source/canvas API,
not an untracked local path or the old crates.io release. Cargo.lock records that
revision. The local nested CGS checkout is development material, not a build input.

This removes the duplicate Rift capture infrastructure and its ownership/channel
state. CGS gains hosting, shared sources, camera, frame handles, budgets and diagnostics
while retaining the old standalone API. Production LOC increased overall; this does
not meet a net-LOC-reduction claim. Exact added/removed counts are recorded with the
final commits, separately from native test/benchmark/report support.

The tested renderer is CGS [`caaf18aa`](https://github.com/acsandmann/cgs/commit/caaf18aa459eef99b3a8a8d8b861f1795c52c393), pinned by full Git revision. Final native reruns passed the 20-source lifecycle suite and the two-canvas screenshot/reversal probe. CGS’s shipped-metallib pixel test passed; Rift’s all-target check passed with the Git dependency. A fresh archived Rift checkout with no nested CGS directory also passed `cargo check --locked --workspace --all-targets`.

Relative to the starting commits, production Rust/shader changes are CGS +847/-201 and Rift +543/-885 (net +304 lines across both). These counts exclude manifests, lockfiles, generated metallib, native fixtures, scripts and reports. The source/capture separation reduces Rift ownership complexity, but the shared-canvas/API/resource work increases total source size.
