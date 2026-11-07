use anyhow::{Result, ensure};
use std::{ffi::c_void, ptr};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}
// SAFETY: Signatures use CoreAudio C ABI types; Address matches AudioObjectPropertyAddress.
#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn AudioObjectSetPropertyData(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> i32;
}
const GLOBAL: u32 = u32::from_be_bytes(*b"glob");
fn read(object: u32, address: &Address) -> Result<u32> {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: All callers request UInt32 properties. The repr(C) address and aligned
    // UInt32 output/size buffers remain valid for this synchronous call; no qualifier is used.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            address,
            0,
            ptr::null(),
            &mut size,
            (&mut value as *mut u32).cast(),
        )
    };
    ensure!(
        status == 0 && size == 4,
        "Could not read audio mute state (CoreAudio {status}); the output may not support mute"
    );
    Ok(value)
}
fn device() -> Result<u32> {
    let id = read(
        1,
        &Address {
            selector: u32::from_be_bytes(*b"dOut"),
            scope: GLOBAL,
            element: 0,
        },
    )?;
    ensure!(id != 0, "No default audio output");
    Ok(id)
}
fn address() -> Address {
    Address {
        selector: u32::from_be_bytes(*b"mute"),
        scope: u32::from_be_bytes(*b"outp"),
        element: 0,
    }
}
pub fn muted() -> Result<bool> {
    Ok(read(device()?, &address())? != 0)
}
pub fn set_muted(muted: bool) -> Result<()> {
    let id = device()?;
    let value = u32::from(muted);
    // SAFETY: The mute property takes a UInt32. Its repr(C) address and initialized
    // value remain valid for the synchronous call; zero qualifier size permits null.
    let status = unsafe {
        AudioObjectSetPropertyData(
            id,
            &address(),
            0,
            ptr::null(),
            4,
            (&value as *const u32).cast(),
        )
    };
    ensure!(
        status == 0,
        "Could not change audio mute state (CoreAudio {status}); the output may not support mute"
    );
    Ok(())
}

use block2::{Block, RcBlock};
use host_agent_core::audio::{AudioNotifier, AudioSubscription};
type Callback = dyn Fn(u32, *const c_void);
// SAFETY: Signatures use CoreAudio C ABI types; Address matches AudioObjectPropertyAddress.
#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectAddPropertyListenerBlock(
        object: u32,
        address: *const Address,
        queue: *mut c_void,
        callback: *const Block<Callback>,
    ) -> i32;
    fn AudioObjectRemovePropertyListenerBlock(
        object: u32,
        address: *const Address,
        queue: *mut c_void,
        callback: *const Block<Callback>,
    ) -> i32;
}
fn property_matches(expected: Address, actual: Address) -> bool {
    let wildcard = u32::from_be_bytes(*b"****");
    (actual.selector == expected.selector || actual.selector == wildcard)
        && (actual.scope == expected.scope || actual.scope == wildcard)
        && (actual.element == expected.element || actual.element == u32::MAX)
}

struct Listener {
    object: u32,
    address: Address,
    callback: RcBlock<Callback>,
}
impl Listener {
    fn new(object: u32, address: Address, source: AudioNotifier, rebind: bool) -> Result<Self> {
        let expected = address;
        let callback: RcBlock<Callback> = RcBlock::new(move |count: u32, data: *const c_void| {
            if count == 0 || data.is_null() {
                return;
            }
            // CoreAudio can batch unrelated property addresses into one callback.
            // SAFETY: CoreAudio supplies count aligned, initialized AudioObjectPropertyAddress
            // values in a single buffer, valid for this invocation. Zero/null were handled
            // above; the borrowed slice never escapes the callback.
            let addresses =
                unsafe { std::slice::from_raw_parts(data.cast::<Address>(), count as usize) };
            if addresses
                .iter()
                .any(|actual| property_matches(expected, *actual))
            {
                source.changed(rebind);
            }
        });
        // SAFETY: The address has the CoreAudio C layout. CoreAudio copies/retains
        // the correctly typed block on success; Listener also owns it until removal.
        // A null queue requests CoreAudio's internal dispatch queue. Captures are
        // thread-safe and the callback only reads its borrowed array and signals.
        let status = unsafe {
            AudioObjectAddPropertyListenerBlock(object, &address, ptr::null_mut(), &*callback)
        };
        ensure!(
            status == 0,
            "Could not subscribe to CoreAudio (status {status})"
        );
        Ok(Self {
            object,
            address,
            callback,
        })
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        // SAFETY: This is the same object, address, null queue, and block used for
        // registration. All remain alive through removal. CoreAudio retains its block
        // copies, including captures, for any invocation still in flight.
        unsafe {
            AudioObjectRemovePropertyListenerBlock(
                self.object,
                &self.address,
                ptr::null_mut(),
                &*self.callback,
            );
        }
    }
}
pub fn subscribe() -> Result<AudioSubscription> {
    super::native_subscription(|source| {
        let mut listeners = vec![
            Listener::new(
                1,
                Address {
                    selector: u32::from_be_bytes(*b"dOut"),
                    scope: GLOBAL,
                    element: 0,
                },
                source.clone(),
                true,
            )?,
            Listener::new(
                1,
                Address {
                    selector: u32::from_be_bytes(*b"srst"),
                    scope: GLOBAL,
                    element: 0,
                },
                source.clone(),
                true,
            )?,
        ];
        if let Ok(id) = device() {
            if read(id, &address()).is_ok() {
                listeners.push(Listener::new(id, address(), source.clone(), false)?);
            }
            listeners.push(Listener::new(
                id,
                Address {
                    selector: u32::from_be_bytes(*b"livn"),
                    scope: GLOBAL,
                    element: 0,
                },
                source,
                true,
            )?);
        }
        Ok(listeners)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn property_batches_only_notify_the_registered_property() {
        let mute = address();
        let alive = Address {
            selector: u32::from_be_bytes(*b"livn"),
            scope: GLOBAL,
            element: 0,
        };
        assert!(property_matches(mute, mute));
        assert!(!property_matches(alive, mute));
        assert!(!property_matches(mute, alive));
        assert!(property_matches(
            mute,
            Address {
                selector: u32::from_be_bytes(*b"****"),
                scope: mute.scope,
                element: u32::MAX
            }
        ));
    }
}
