//! Is sound coming out of speakers? Then the mic picks up the movie and
//! friends hear it twice, a moment late. Asked of CoreAudio directly.

/// Some(true): speakers (the Mac's own, a TV or display over HDMI or
/// DisplayPort, AirPlay). Some(false): headphones, Bluetooth, or USB (often
/// a headset). None: couldn't tell.
#[tauri::command]
pub fn output_is_speakers() -> Option<bool> {
    imp::output_is_speakers()
}

const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

/// What a transport type and (for built-in outputs) data source mean.
pub fn is_speakers(transport: u32, data_source: Option<u32>) -> Option<bool> {
    match transport {
        t if t == fourcc(b"bltn") => Some(data_source != Some(fourcc(b"hdpn"))),
        t if t == fourcc(b"hdmi") || t == fourcc(b"dprt") || t == fourcc(b"airp") => Some(true),
        t if t == fourcc(b"blue") || t == fourcc(b"blea") || t == fourcc(b"usb ") => Some(false),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::fourcc;
    use std::ffi::c_void;

    #[repr(C)]
    struct Address {
        selector: u32,
        scope: u32,
        element: u32,
    }

    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        fn AudioObjectGetPropertyData(
            object: u32,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const c_void,
            data_size: *mut u32,
            data: *mut c_void,
        ) -> i32;
    }

    const SYSTEM_OBJECT: u32 = 1;
    const MAIN_ELEMENT: u32 = 0;

    fn get_u32(object: u32, selector: &[u8; 4], scope: &[u8; 4]) -> Option<u32> {
        let address = Address {
            selector: fourcc(selector),
            scope: fourcc(scope),
            element: MAIN_ELEMENT,
        };
        let mut value: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: a u32 out-parameter of the size we pass, for properties
        // whose value is a UInt32 (device ids, four-char codes).
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                (&mut value as *mut u32).cast(),
            )
        };
        (status == 0).then_some(value)
    }

    pub fn output_is_speakers() -> Option<bool> {
        let device = get_u32(SYSTEM_OBJECT, b"dOut", b"glob").filter(|d| *d != 0)?;
        let transport = get_u32(device, b"tran", b"glob")?;
        let source = get_u32(device, b"ssrc", b"outp");
        super::is_speakers(transport, source)
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn output_is_speakers() -> Option<bool> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speakers_are_told_from_headphones() {
        let builtin = fourcc(b"bltn");
        assert_eq!(is_speakers(builtin, Some(fourcc(b"ispk"))), Some(true));
        assert_eq!(is_speakers(builtin, Some(fourcc(b"hdpn"))), Some(false));
        assert_eq!(is_speakers(builtin, None), Some(true));
        assert_eq!(is_speakers(fourcc(b"hdmi"), None), Some(true));
        assert_eq!(is_speakers(fourcc(b"blue"), None), Some(false));
        assert_eq!(is_speakers(fourcc(b"virt"), None), None);
    }

    #[test]
    fn this_mac_answers() {
        // Whatever it's plugged into, CoreAudio should know the output.
        #[cfg(target_os = "macos")]
        println!("speakers: {:?}", output_is_speakers());
    }
}
