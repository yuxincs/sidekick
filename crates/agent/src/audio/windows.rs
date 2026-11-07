use anyhow::Result;
use windows::Win32::{
    Foundation::RPC_E_CHANGED_MODE,
    Media::Audio::{
        Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
    },
    System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    },
};
struct Com(bool);
impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: Com is created only after successful CoInitializeEx and stays
            // on that thread. Each successful initialization is balanced exactly once.
            unsafe {
                CoUninitialize();
            }
        }
    }
}
fn endpoint<T>(operation: impl FnOnce(IAudioEndpointVolume) -> Result<T>) -> Result<T> {
    // SAFETY: COM is initialized on this calling thread (or its existing apartment
    // is used for RPC_E_CHANGED_MODE). All interface values are owned and dropped
    // before the matching Com guard. Calls use valid class IDs and no aggregation.
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED);
        if initialized != RPC_E_CHANGED_MODE {
            initialized.ok()?;
        }
        let _com = Com(initialized.is_ok());
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        operation(device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)?)
    }
}
pub fn muted() -> Result<bool> {
    endpoint(|e| {
        // SAFETY: endpoint supplies a live IAudioEndpointVolume in this thread's
        // initialized COM apartment; GetMute has no caller-provided pointer inputs.
        Ok(unsafe { e.GetMute()? }.as_bool())
    })
}
pub fn set_muted(muted: bool) -> Result<()> {
    endpoint(|e| {
        // SAFETY: The endpoint is live in this COM apartment. The BOOL is valid
        // and the API permits a null event-context GUID pointer.
        unsafe {
            e.SetMute(muted, std::ptr::null())?;
        }
        Ok(())
    })
}

use host_agent_core::audio::{AudioNotifier, AudioSubscription};
use std::sync::atomic::{AtomicI8, Ordering};
use windows::{
    Win32::{
        Foundation::PROPERTYKEY,
        Media::Audio::{
            AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, EDataFlow, ERole,
            Endpoints::{IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl},
            IMMNotificationClient, IMMNotificationClient_Impl,
        },
    },
    core::{PCWSTR, implement},
};

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback {
    source: AudioNotifier,
    last: AtomicI8,
}
impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        // SAFETY: Windows invokes this private COM callback with a properly aligned,
        // initialized notification buffer valid until OnNotify returns. as_ref handles
        // null, and the borrow never escapes. Only the fixed-size mute field is read.
        if let Some(data) = unsafe { data.as_ref() } {
            let muted = i8::from(data.bMuted.as_bool());
            if self.last.swap(muted, Ordering::Relaxed) != muted {
                self.source.changed(false);
            }
        }
        Ok(())
    }
}
#[implement(IMMNotificationClient)]
struct DeviceCallback(AudioNotifier);
impl IMMNotificationClient_Impl for DeviceCallback_Impl {
    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        if flow == eRender && role == eConsole {
            self.0.changed(true);
        }
        Ok(())
    }
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> windows::core::Result<()> {
        self.0.changed(true);
        Ok(())
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> windows::core::Result<()> {
        self.0.changed(true);
        Ok(())
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> windows::core::Result<()> {
        self.0.changed(true);
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> windows::core::Result<()> {
        Ok(())
    }
}
struct Registration {
    volume: Option<(IAudioEndpointVolume, IAudioEndpointVolumeCallback)>,
    enumerator: IMMDeviceEnumerator,
    device_callback: IMMNotificationClient,
    _com: Com,
}
impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: These owned interfaces and callback identities remain alive on
        // the registration thread, with COM still initialized by _com. Unregister
        // before releasing our references; Windows owns references during callbacks.
        unsafe {
            if let Some((volume, callback)) = &self.volume {
                let _ = volume.UnregisterControlChangeNotify(callback);
            }
            let _ = self
                .enumerator
                .UnregisterEndpointNotificationCallback(&self.device_callback);
        }
    }
}
pub fn subscribe() -> Result<AudioSubscription> {
    super::native_subscription(|source| {
        // SAFETY: The dedicated worker initializes its own COM apartment. Owned
        // interfaces and callbacks are retained by Registration, which unregisters
        // them and releases them on this same worker before uninitializing COM.
        // All interface arguments are live, and activation uses no aggregation.
        unsafe {
            let initialized = CoInitializeEx(None, COINIT_MULTITHREADED);
            initialized.ok()?;
            let com = Com(true);
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device_callback: IMMNotificationClient = DeviceCallback(source.clone()).into();
            enumerator.RegisterEndpointNotificationCallback(&device_callback)?;
            // Construct the RAII owner before any further fallible operations.
            let mut registration = Registration {
                volume: None,
                enumerator,
                device_callback,
                _com: com,
            };
            if let Ok(device) = registration
                .enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
            {
                let volume = device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)?;
                let callback: IAudioEndpointVolumeCallback = VolumeCallback {
                    source,
                    last: AtomicI8::new(-1),
                }
                .into();
                volume.RegisterControlChangeNotify(&callback)?;
                registration.volume = Some((volume, callback));
            }
            Ok(registration)
        }
    })
}
