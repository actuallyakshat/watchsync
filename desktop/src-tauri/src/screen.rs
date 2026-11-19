//! Screen sharing: which macOS picker `getDisplayMedia` opens.
//!
//! Left alone, WebKit in an app hands over the whole main screen without
//! asking. WebKit asks its UI delegate first, though, through a private
//! method wry doesn't implement. We add it to wry's delegate. The call window
//! first says what you picked in its menu (your entire screen, or a window),
//! and then the delegate opens the matching macOS picker. Any request that
//! didn't come from that menu is denied, so nothing is captured unasked.

use std::sync::atomic::{AtomicIsize, Ordering};

/// WebKit's `WKDisplayCapturePermissionDecision`.
const DENY: isize = 0;
const SCREEN: isize = 1;
const WINDOW: isize = 2;

/// What the next `getDisplayMedia` may open. Read once, then back to deny.
static NEXT: AtomicIsize = AtomicIsize::new(DENY);

/// The call window's share menu: "screen" or "window". Its next
/// `getDisplayMedia` opens that macOS picker.
#[tauri::command]
pub fn choose_share(kind: String) -> Result<(), String> {
    let decision = match kind.as_str() {
        "screen" => SCREEN,
        "window" => WINDOW,
        _ => return Err(format!("unknown share kind {kind:?}")),
    };
    NEXT.store(decision, Ordering::SeqCst);
    Ok(())
}

fn take() -> isize {
    NEXT.swap(DENY, Ordering::SeqCst)
}

/// Teach this window's web view to ask us before sharing the screen.
pub fn install(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    if let Err(e) = window.with_webview(|wv| unsafe { mac::install(wv.inner()) }) {
        eprintln!("screen sharing unavailable: {e}");
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

#[cfg(target_os = "macos")]
mod mac {
    use block2::Block;
    use objc2::ffi;
    use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
    use objc2::{msg_send, sel};
    use std::ffi::c_void;

    /// `_webView:requestDisplayCapturePermissionForOrigin:initiatedByFrame:
    /// withSystemAudio:decisionHandler:`
    unsafe extern "C-unwind" fn decide(
        _this: *mut AnyObject,
        _cmd: Sel,
        _webview: *mut AnyObject,
        _origin: *mut AnyObject,
        _frame: *mut AnyObject,
        _system_audio: Bool,
        handler: &Block<dyn Fn(isize)>,
    ) {
        handler.call((super::take(),));
    }

    pub unsafe fn install(webview: *mut c_void) {
        unsafe {
            let webview = webview.cast::<AnyObject>();
            if webview.is_null() {
                return;
            }
            let delegate: *mut AnyObject = msg_send![webview, UIDelegate];
            if delegate.is_null() {
                return;
            }
            let class: *const AnyClass = (*delegate).class();
            let selector = sel!(_webView:requestDisplayCapturePermissionForOrigin:initiatedByFrame:withSystemAudio:decisionHandler:);
            let imp: Imp = std::mem::transmute::<
                unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject, *mut AnyObject, Bool, &Block<dyn Fn(isize)>),
                Imp,
            >(decide);
            // Fails (harmlessly) if it's already there.
            ffi::class_addMethod(class as *mut AnyClass, selector, imp, c"v@:@@@B@?".as_ptr());
            // WebKit may cache what the delegate can do when it's set.
            let _: () = msg_send![webview, setUIDelegate: delegate];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_is_used_once_then_requests_are_denied() {
        assert_eq!(take(), DENY);
        choose_share("window".into()).unwrap();
        assert_eq!(take(), WINDOW);
        assert_eq!(take(), DENY);
        choose_share("screen".into()).unwrap();
        assert_eq!(take(), SCREEN);
        assert!(choose_share("tab".into()).is_err());
        assert_eq!(take(), DENY);
    }
}
