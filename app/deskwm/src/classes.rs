//! The AppKit side of a desk window: its NSWindow, the view the renderer draws
//! into (a CAMetalLayer) and the window's delegate. Each forwards to `desk`
//! and `input`; nothing here holds state beyond the window's hwnd.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSEvent, NSResponder, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow, NSWindowDelegate,
    NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSRect, NSSize};
use objc2_quartz_core::{CALayer, CAMetalLayer};

use crate::ffi::guard;
use crate::input::{self, Ev};

pub struct ViewIvars {
    hwnd: Cell<u64>,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; DeskView has no Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DeskwmView"]
    #[ivars = ViewIvars]
    pub struct DeskView;

    impl DeskView {
        #[unsafe(method_id(makeBackingLayer))]
        fn make_backing_layer(&self) -> Retained<CALayer> {
            let layer = CAMetalLayer::new();
            layer.setOpaque(false);
            layer.setFramebufferOnly(true);
            Retained::into_super(layer)
        }

        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            false
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            unsafe {
                let _: () = msg_send![super(self), setFrameSize: size];
            }
            self.update_drawable_size();
        }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn backing_changed(&self) {
            unsafe {
                let _: () = msg_send![super(self), viewDidChangeBackingProperties];
            }
            self.update_drawable_size();
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            unsafe {
                let _: () = msg_send![super(self), updateTrackingAreas];
            }
            if let Some(old) = self.ivars().tracking.borrow_mut().take() {
                self.removeTrackingArea(&old);
            }
            let options = NSTrackingAreaOptions::MouseMoved | NSTrackingAreaOptions::ActiveAlways | NSTrackingAreaOptions::InVisibleRect;
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    self.bounds(),
                    options,
                    Some(self),
                    None,
                )
            };
            self.addTrackingArea(&area);
            *self.ivars().tracking.borrow_mut() = Some(area);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, e: &NSEvent) {
            self.forward(Ev::Down(1), e);
        }
        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, e: &NSEvent) {
            self.forward(Ev::Up(1), e);
        }
        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, e: &NSEvent) {
            self.forward(Ev::Down(3), e);
        }
        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, e: &NSEvent) {
            self.forward(Ev::Up(3), e);
        }
        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, e: &NSEvent) {
            let button = match e.buttonNumber() {
                3 => 4,
                4 => 5,
                _ => 2,
            };
            self.forward(Ev::Down(button), e);
        }
        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, e: &NSEvent) {
            let button = match e.buttonNumber() {
                3 => 4,
                4 => 5,
                _ => 2,
            };
            self.forward(Ev::Up(button), e);
        }
        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, e: &NSEvent) {
            self.forward(Ev::Move, e);
        }
        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, e: &NSEvent) {
            self.forward(Ev::Drag, e);
        }
        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, e: &NSEvent) {
            self.forward(Ev::Drag, e);
        }
        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, e: &NSEvent) {
            self.forward(Ev::Drag, e);
        }
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, e: &NSEvent) {
            // A scroll the trackpad's fingers already make on the host
            if input::trackpad_scroll(e) {
                return;
            }
            self.forward(Ev::Scroll, e);
        }
        #[unsafe(method(touchesBeganWithEvent:))]
        fn touches_began(&self, e: &NSEvent) {
            self.touches(e);
        }
        #[unsafe(method(touchesMovedWithEvent:))]
        fn touches_moved(&self, e: &NSEvent) {
            self.touches(e);
        }
        #[unsafe(method(touchesEndedWithEvent:))]
        fn touches_ended(&self, e: &NSEvent) {
            self.touches(e);
        }
        #[unsafe(method(touchesCancelledWithEvent:))]
        fn touches_cancelled(&self, e: &NSEvent) {
            self.touches(e);
        }
    }
);

impl DeskView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect, hwnd: u64) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars { hwnd: Cell::new(hwnd), tracking: RefCell::new(None) });
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setWantsLayer(true);
        view.setLayerContentsRedrawPolicy(objc2_app_kit::NSViewLayerContentsRedrawPolicy::Never);
        // The trackpad's fingers, for the host's own touchpad gestures
        view.setAllowedTouchTypes(objc2_app_kit::NSTouchTypeMask::Indirect);
        view.update_drawable_size();
        view
    }

    pub fn hwnd(&self) -> u64 {
        self.ivars().hwnd.get()
    }

    pub fn metal_layer(&self) -> Option<Retained<CALayer>> {
        self.layer()
    }

    pub fn update_drawable_size(&self) {
        if let Some(layer) = self.layer() {
            if let Ok(metal) = layer.downcast::<CAMetalLayer>() {
                let scale = self.window().map(|w| w.backingScaleFactor()).unwrap_or(2.0);
                let b = self.bounds();
                metal.setContentsScale(scale);
                metal.setDrawableSize(objc2_core_foundation::CGSize::new(b.size.width * scale, b.size.height * scale));
            }
        }
    }

    fn touches(&self, e: &NSEvent) {
        let hwnd = self.hwnd();
        guard("view touches", || input::touches(hwnd, e, self));
    }

    fn forward(&self, ev: Ev, e: &NSEvent) {
        let hwnd = self.hwnd();
        guard("view input", || {
            // The host holds the pointer (a game): motion, not positions
            if matches!(ev, Ev::Move | Ev::Drag) && crate::desk::pointer_locked() {
                crate::desk::relative_motion(e.deltaX(), e.deltaY());
                return;
            }
            let p = self.convertPoint_fromView(e.locationInWindow(), None);
            let clicks = match ev {
                // clickCount only exists on button events (it throws on others)
                Ev::Down(_) | Ev::Up(_) => e.clickCount() as i32,
                _ => 0,
            };
            let (sx, sy) = match ev {
                Ev::Scroll => {
                    let mut dx = e.scrollingDeltaX();
                    let mut dy = e.scrollingDeltaY();
                    if !e.hasPreciseScrollingDeltas() {
                        dx *= 10.0;
                        dy *= 10.0;
                    }
                    (dx, dy)
                }
                _ => (0.0, 0.0),
            };
            let cmd = e.modifierFlags().contains(objc2_app_kit::NSEventModifierFlags::Command);
            if matches!(ev, Ev::Down(_) | Ev::Up(_)) && std::env::var_os("DESKWM_DEBUG_INPUT").is_some() {
                crate::ffi::log(0, &format!("deskwm: {ev:?} on 0x{hwnd:x} at {:.0},{:.0} clicks {clicks} cmd {cmd}", p.x, p.y));
            }
            input::on_event(hwnd, ev, p, clicks, sx, sy, cmd);
        });
    }
}

pub struct WindowIvars {
    pane: Cell<bool>,
}

define_class!(
    // SAFETY: NSWindow has no subclassing requirements; DeskWindow has no Drop.
    #[unsafe(super(NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DeskwmWindow"]
    #[ivars = WindowIvars]
    pub struct DeskWindow;

    impl DeskWindow {
        // The keyboard stays with the hidden key window: the host routes keys
        // to whichever of its windows has the focus
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            false
        }

        // Windows and dialogs can become main: window switchers, Mission
        // Control and the Dock pick a window by making it main
        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            self.ivars().pane.get()
        }
    }
);

impl DeskWindow {
    pub fn new(mtm: MainThreadMarker, frame: NSRect, style: NSWindowStyleMask, pane: bool) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WindowIvars { pane: Cell::new(pane) });
        unsafe {
            msg_send![super(this), initWithContentRect: frame, styleMask: style, backing: NSBackingStoreType::Buffered, defer: false]
        }
    }

    pub fn as_window(&self) -> &NSWindow {
        self
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; Delegate has no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DeskwmDelegate"]
    #[ivars = Cell<u64>]
    pub struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowDidMove:))]
        fn window_did_move(&self, _n: &NSNotification) {
            let hwnd = self.ivars().get();
            guard("windowDidMove", || crate::desk::did_move(hwnd));
        }

        #[unsafe(method(windowWillStartLiveResize:))]
        fn window_will_start_live_resize(&self, _n: &NSNotification) {
            let hwnd = self.ivars().get();
            guard("windowWillStartLiveResize", || crate::desk::will_start_live_resize(hwnd));
        }

        #[unsafe(method(windowWillResize:toSize:))]
        fn window_will_resize(&self, _w: &NSWindow, size: NSSize) -> NSSize {
            let hwnd = self.ivars().get();
            guard("windowWillResize", || crate::desk::will_resize(hwnd, size)).unwrap_or(size)
        }

        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _n: &NSNotification) {
            let hwnd = self.ivars().get();
            guard("windowDidResize", || crate::desk::did_resize(hwnd));
        }

        #[unsafe(method(windowDidEndLiveResize:))]
        fn window_did_end_live_resize(&self, _n: &NSNotification) {
            let hwnd = self.ivars().get();
            guard("windowDidEndLiveResize", || crate::desk::did_end_live_resize(hwnd));
        }

        #[unsafe(method(windowDidChangeOcclusionState:))]
        fn window_did_change_occlusion_state(&self, _n: &NSNotification) {
            let hwnd = self.ivars().get();
            guard("occlusion", || crate::desk::update_visible(hwnd));
        }

        #[unsafe(method(windowDidMiniaturize:))]
        fn window_did_miniaturize(&self, _n: &NSNotification) {
            let hwnd = self.ivars().get();
            guard("windowDidMiniaturize", || crate::desk::did_miniaturize(hwnd, true));
        }

        #[unsafe(method(windowDidDeminiaturize:))]
        fn window_did_deminiaturize(&self, _n: &NSNotification) {
            let hwnd = self.ivars().get();
            guard("windowDidDeminiaturize", || crate::desk::did_miniaturize(hwnd, false));
        }
    }
);

impl Delegate {
    pub fn new(mtm: MainThreadMarker, hwnd: u64) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Cell::new(hwnd));
        unsafe { msg_send![super(this), init] }
    }

    pub fn as_protocol(&self) -> &ProtocolObject<dyn NSWindowDelegate> {
        ProtocolObject::from_ref(self)
    }
}
