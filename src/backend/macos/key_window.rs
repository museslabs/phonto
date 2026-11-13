//! A wallpaper window that can take the keyboard, for procedural wallpapers
//! with a keyboard channel.
//!
//! It is a non-activating panel. It becomes the key window when the user
//! clicks the desktop, and then gets the key events, while the app in front
//! stays active. A click on a window or a switch to another app makes that
//! window the key window, and the wallpaper loses the keyboard. Reading keys
//! this way needs no permission, because macOS sends them to our own window.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, extern_methods, msg_send};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSEvent, NSEventModifierFlags, NSEventType, NSPanel,
    NSResponder, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSRect};

use super::keyboard::Keyboard;

define_class!(
    /// The application. AppKit gives the key window no key-up while Command
    /// is down. The key would stay down in the wallpaper. This sends it on,
    /// as SDL and winit do.
    #[unsafe(super(NSApplication, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    pub struct Application;

    impl Application {
        #[unsafe(method(sendEvent:))]
        fn send_event(&self, event: &NSEvent) {
            if event.r#type() == NSEventType::KeyUp
                && event.modifierFlags().contains(NSEventModifierFlags::Command)
                && let Some(window) = self.keyWindow()
            {
                window.sendEvent(event);
            } else {
                // SAFETY: `sendEvent:` takes an event and returns nothing.
                let _: () = unsafe { msg_send![super(self), sendEvent: event] };
            }
        }
    }
);

impl Application {
    extern_methods!(
        /// The application, an `Application` if no other call made it first.
        #[unsafe(method(sharedApplication))]
        pub fn shared(mtm: MainThreadMarker) -> Retained<NSApplication>;
    );
}

pub struct KeyWindowIvars {
    keyboard: Rc<RefCell<Keyboard>>,
}

define_class!(
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = KeyWindowIvars]
    pub struct KeyWindow;

    impl KeyWindow {
        /// A borderless window can't become key unless it says so.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        #[unsafe(method(becomeKeyWindow))]
        fn become_key_window(&self) {
            let _: () = unsafe { msg_send![super(self), becomeKeyWindow] };
            log::info!("the wallpaper has the keyboard");
        }

        #[unsafe(method(resignKeyWindow))]
        fn resign_key_window(&self) {
            let _: () = unsafe { msg_send![super(self), resignKeyWindow] };
            self.ivars().keyboard.borrow_mut().release_all();
            log::info!("the wallpaper gave back the keyboard");
        }

        // These don't call super. NSWindow would beep for a key that nothing
        // handles.
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            self.ivars().keyboard.borrow_mut().key(event.keyCode(), true);
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            self.ivars().keyboard.borrow_mut().key(event.keyCode(), false);
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            self.ivars()
                .keyboard
                .borrow_mut()
                .modifier(event.keyCode(), event.modifierFlags());
        }
    }
);

impl KeyWindow {
    /// Makes a borderless window over `frame` that feeds `keyboard`.
    pub fn create(
        mtm: MainThreadMarker,
        frame: NSRect,
        keyboard: Rc<RefCell<Keyboard>>,
    ) -> Retained<NSWindow> {
        let this = Self::alloc(mtm).set_ivars(KeyWindowIvars { keyboard });
        let this: Retained<Self> = unsafe {
            msg_send![
                super(this),
                initWithContentRect: frame,
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered,
                defer: false,
            ]
        };
        // A panel hides when its app deactivates. The wallpaper must stay.
        this.setHidesOnDeactivate(false);
        // AppKit makes a window that can become key the key window by itself,
        // as `[NSApp run]` does at launch. This one waits for a click.
        this.setBecomesKeyOnlyIfNeeded(true);
        Retained::into_super(Retained::into_super(this))
    }
}
