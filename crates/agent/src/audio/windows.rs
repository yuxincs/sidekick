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
            unsafe {
                CoUninitialize();
            }
        }
    }
}
fn endpoint<T>(operation: impl FnOnce(IAudioEndpointVolume) -> Result<T>) -> Result<T> {
    // Initialize the calling worker's COM apartment and release interfaces before uninitializing.
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
    endpoint(|e| Ok(unsafe { e.GetMute()? }.as_bool()))
}
pub fn set_muted(muted: bool) -> Result<()> {
    endpoint(|e| {
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
        // The system owns this buffer for the duration of OnNotify. Only enqueue a signal here.
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
    super::native_subscription(|source| unsafe {
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
    })
}
