use objc2::{AnyThread, DefinedClass, define_class, msg_send, rc::Retained, runtime::NSObject};
use objc2_av_foundation::AVPlayer;
use objc2_core_media::kCMTimeZero;
use objc2_foundation::NSNotification;

pub struct LoopObserverIvars {
    player: Retained<AVPlayer>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = LoopObserverIvars]
    pub struct LoopObserver;

    impl LoopObserver {
        /// Only the screen observer plays and pauses the player.
        #[unsafe(method(itemEnded:))]
        fn _item_ended(&self, _notif: &NSNotification) {
            unsafe { self.ivars().player.seekToTime(kCMTimeZero) };
        }
    }
);

impl LoopObserver {
    pub fn new(player: Retained<AVPlayer>) -> Retained<Self> {
        let ivars = LoopObserverIvars { player };
        let this = Self::alloc().set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }
}
