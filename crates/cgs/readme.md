# cgs

> cgs aka core graphics

### Layout illustrations

`LayoutPreview` owns a bezel-free monochrome canvas, reusable native window views,
and their animation transactions. Callers only supply layout geometry in canvas
points, measured from the top-left corner. Stable window IDs preserve views when
windows rearrange; slice order controls stacking for overlapping layouts.

```rust
let preview = LayoutPreview::new(&ui, CGSize::new(420.0, 220.0));
preview.set_windows(&[
    PreviewWindow::new(0, CGRect::new(CGPoint::new(10.0, 10.0), CGSize::new(195.0, 200.0))),
    PreviewWindow::new(1, CGRect::new(CGPoint::new(215.0, 10.0), CGSize::new(195.0, 95.0))),
    PreviewWindow::new(2, CGRect::new(CGPoint::new(215.0, 115.0), CGSize::new(195.0, 95.0))),
], PreviewAnimation::default());
```

Pass a new layout to move, resize, add, remove, or reorder windows. Use
`PreviewAnimation::Immediate` for direct manipulation or
`PreviewAnimation::Animated(Duration::from_millis(160))` for a transition.
The first layout and updates with Reduce Motion enabled are always immediate.
Identical geometry does no animation work. `canvas_size()` exposes the drawing
area; `set_panes()` is shorthand using slice indices as identities.

System fill and separator colors provide the appearance. There is no live capture,
wallpaper decoding, timer, shadow, or idle render loop. Share the component with
callbacks through `Rc<LayoutPreview>`.
