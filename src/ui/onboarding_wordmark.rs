//! Original Rift lettering, drawn for this project. No font or traced reference assets.
use std::ffi::c_void;

use cgs::Ui;
use cgs::view::{NativeView, View};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, class, define_class, msg_send};
use objc2_app_kit::{NSColor, NSView, NSWorkspace};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSNumber, NSString};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPathCreateMutable() -> *mut c_void;
    fn CGPathMoveToPoint(path: *mut c_void, transform: *const c_void, x: f64, y: f64);
    fn CGPathAddCurveToPoint(
        path: *mut c_void,
        transform: *const c_void,
        a: f64,
        b: f64,
        c: f64,
        d: f64,
        x: f64,
        y: f64,
    );
    fn CGPathRelease(path: *mut c_void);
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "RiftSetupWordmark"]
    #[ivars = f64]
    struct WordmarkView;
    impl WordmarkView {
        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance(&self) {
            unsafe { let _: () = msg_send![super(self), viewDidChangeEffectiveAppearance]; }
            if let Some(layer) = self.layer() {
                if let Some(layers) = unsafe { layer.sublayers() } {
                    for stroke in layers.iter() {
                        unsafe { let _: () = msg_send![&*stroke, setStrokeColor: Retained::as_ptr(&NSColor::labelColor().CGColor())]; }
                    }
                }
            }
        }
        #[unsafe(method(layout))]
        fn arrange(&self) {
            unsafe { let _: () = msg_send![super(self), layout]; }
            if let Some(layer) = self.layer() {
                if let Some(layers) = unsafe { layer.sublayers() } {
                    for stroke in layers.iter() {
                        let scale = *self.ivars();
                        stroke.setFrame(CGRect::new(CGPoint::new((self.bounds().size.width - 410. * scale) / 2., 0.), CGSize::new(410. * scale, 200. * scale)));
                        stroke.setContentsScale(self.window().map_or(2., |w| w.backingScaleFactor()));
                    }
                }
            }
        }
    }
);
struct Wordmark(Retained<WordmarkView>);
impl NativeView for Wordmark {
    fn ns_view(&self) -> &NSView { &self.0 }
}

pub(super) fn create_scaled(ui: &Ui, animate: bool, scale: f64) -> View {
    let native: Retained<WordmarkView> =
        unsafe { msg_send![super(WordmarkView::alloc(ui.mtm()).set_ivars(scale)), init] };
    native.heightAnchor().constraintEqualToConstant(200. * scale).setActive(true);
    native.setWantsLayer(true);
    let reduce = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
    // A connected r-i-f-t body, followed by the crossbar and the lifted i dot.
    // Coordinates are in points. Core Animation rasterizes at the display's backing scale.
    let strokes: &[(f64, f64, &[[f64; 6]], f64, f64)] = &[
        (
            100.,
            68.,
            &[
                [115., 74., 128., 100., 132., 114.],
                [137., 135., 121., 125., 124., 110.],
                [127., 100., 145., 131., 157., 119.],
                [166., 110., 147., 72., 151., 65.],
                [156., 54., 180., 88., 191., 109.],
                [200., 127., 182., 80., 181., 68.],
                [180., 49., 207., 67., 223., 97.],
                [249., 145., 270., 190., 251., 183.],
                [232., 176., 222., 113., 215., 72.],
                [203., 24., 184., 13., 190., 37.],
                [197., 65., 246., 87., 265., 104.],
                [283., 122., 291., 153., 281., 146.],
                [269., 135., 253., 78., 256., 67.],
                [261., 47., 290., 71., 305., 91.],
            ],
            0.,
            1.42,
        ),
        (205., 111., &[[228., 114., 261., 119., 289., 116.]], 1.44, 0.19),
        (196., 140., &[[197., 142., 198., 143., 199., 144.]], 1.65, 0.09),
    ];
    unsafe {
        let root = native.layer().unwrap();
        for &(x, y, curves, start, duration) in strokes {
            let path = CGPathCreateMutable();
            CGPathMoveToPoint(path, std::ptr::null(), x * scale, y * scale);
            for p in curves {
                CGPathAddCurveToPoint(
                    path,
                    std::ptr::null(),
                    p[0] * scale,
                    p[1] * scale,
                    p[2] * scale,
                    p[3] * scale,
                    p[4] * scale,
                    p[5] * scale,
                );
            }
            let layer: Retained<AnyObject> = msg_send![class!(CAShapeLayer), new];
            let _: () = msg_send![&*layer, setFrame: CGRect::new(CGPoint::new(55., 0.), CGSize::new(410.,200.))];
            let _: () = msg_send![&*layer, setPath: path];
            CGPathRelease(path);
            let _: () = msg_send![&*layer, setFillColor: std::ptr::null::<c_void>()];
            let _: () = msg_send![&*layer, setStrokeColor: Retained::as_ptr(&NSColor::labelColor().CGColor())];
            let _: () = msg_send![&*layer, setLineWidth: 5.5f64 * scale];
            let _: () = msg_send![&*layer, setLineCap: &*NSString::from_str("round")];
            let _: () = msg_send![&*layer, setLineJoin: &*NSString::from_str("round")];
            let _: () = msg_send![&*layer, setStrokeEnd: 1f64];
            if animate && !reduce {
                let animation: Retained<AnyObject> = msg_send![class!(CABasicAnimation), animationWithKeyPath: &*NSString::from_str("strokeEnd")];
                let _: () = msg_send![&*animation, setFromValue: &*NSNumber::new_f64(0.)];
                let _: () = msg_send![&*animation, setToValue: &*NSNumber::new_f64(1.)];
                let _: () = msg_send![&*animation, setDuration: duration];
                let now = objc2_quartz_core::CACurrentMediaTime();
                let _: () = msg_send![&*animation, setBeginTime: now + start];
                let _: () = msg_send![&*animation, setFillMode: &*NSString::from_str("backwards")];
                let timing: Retained<AnyObject> = msg_send![class!(CAMediaTimingFunction), functionWithName: &*NSString::from_str("easeInEaseOut")];
                let _: () = msg_send![&*animation, setTimingFunction: &*timing];
                let _: () = msg_send![&*layer, addAnimation: &*animation, forKey: &*NSString::from_str("handwriting")];
            }
            let _: () = msg_send![&*root, addSublayer: &*layer];
        }
    }
    let view = View::new(ui).content(Wordmark(native), cgs::layout::uniform_insets(0.));
    view.accessibility_label("rift");
    view
}
