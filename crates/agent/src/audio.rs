//! Default playback-output mute control; microphone input is never changed.
use anyhow::Result;
use host_agent_core::service::AudioControl;
pub struct HostAudio;
impl AudioControl for HostAudio {
    fn muted(&self) -> Result<bool> {
        platform::muted()
    }
    fn set_muted(&self, muted: bool) -> Result<()> {
        platform::set_muted(muted)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use anyhow::{Result, ensure};
    use std::{ffi::c_void, ptr};
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
        // All properties here are UInt32 and the address/data buffers remain valid for this call.
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
}

#[cfg(target_os = "windows")]
mod platform {
    use anyhow::Result;
    use windows::Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        Media::Audio::{
            Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole,
            eRender,
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
}

#[cfg(target_os = "linux")]
mod platform {
    use anyhow::{Context, Result, bail, ensure};
    use std::{
        io::{Read, Seek},
        path::Path,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    enum Backend {
        PipeWire,
        Pulse,
    }
    impl Backend {
        fn detect() -> Result<Self> {
            if Path::new("/usr/bin/wpctl").is_file() {
                Ok(Self::PipeWire)
            } else if Path::new("/usr/bin/pactl").is_file() {
                Ok(Self::Pulse)
            } else {
                bail!("Audio control needs wpctl (WirePlumber) or pactl (PulseAudio)")
            }
        }
        fn run(&self, args: &[&str]) -> Result<String> {
            let executable = match self {
                Self::PipeWire => "/usr/bin/wpctl",
                Self::Pulse => "/usr/bin/pactl",
            };
            let mut output = tempfile::tempfile()?;
            let errors = tempfile::tempfile()?;
            let mut child = Command::new(executable)
                .args(args)
                .env("LC_ALL", "C")
                .stdin(Stdio::null())
                .stdout(output.try_clone()?)
                .stderr(errors)
                .spawn()
                .context("Could not start audio control")?;
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(status) = child.try_wait()? {
                    ensure!(
                        status.success(),
                        "Audio control failed ({status}); check your desktop audio session"
                    );
                    break;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    bail!("Audio request timed out; outcome unknown and not retried");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            output.rewind()?;
            let mut text = String::new();
            output.take(4096).read_to_string(&mut text)?;
            Ok(text)
        }
    }
    fn parse(text: &str, pipewire: bool) -> Result<bool> {
        if pipewire {
            let words: Vec<_> = text.split_whitespace().collect();
            ensure!(
                words.len() == 2 || words.len() == 3,
                "Invalid audio response"
            );
            ensure!(
                words[0] == "Volume:"
                    && words[1]
                        .parse::<f32>()
                        .is_ok_and(|v| v.is_finite() && v >= 0.0),
                "Invalid audio response"
            );
            if words.len() == 3 {
                ensure!(words[2] == "[MUTED]", "Invalid audio response");
            }
            Ok(words.len() == 3)
        } else {
            match text.trim() {
                "Mute: yes" => Ok(true),
                "Mute: no" => Ok(false),
                _ => bail!("Invalid audio response"),
            }
        }
    }
    pub fn muted() -> Result<bool> {
        let backend = Backend::detect()?;
        let args = match backend {
            Backend::PipeWire => ["get-volume", "@DEFAULT_AUDIO_SINK@"],
            Backend::Pulse => ["get-sink-mute", "@DEFAULT_SINK@"],
        };
        parse(&backend.run(&args)?, matches!(backend, Backend::PipeWire))
    }
    pub fn set_muted(muted: bool) -> Result<()> {
        let backend = Backend::detect()?;
        let args = match backend {
            Backend::PipeWire => [
                "set-mute",
                "@DEFAULT_AUDIO_SINK@",
                if muted { "1" } else { "0" },
            ],
            Backend::Pulse => [
                "set-sink-mute",
                "@DEFAULT_SINK@",
                if muted { "1" } else { "0" },
            ],
        };
        backend.run(&args)?;
        Ok(())
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn parses_only_known_audio_states() {
            assert!(parse("Volume: 0.50 [MUTED]\n", true).unwrap());
            assert!(!parse("Volume: 1.00", true).unwrap());
            assert!(parse("Mute: yes\n", false).unwrap());
            assert!(!parse("Mute: no", false).unwrap());
            for text in [
                "",
                "Volume: NaN",
                "Volume: 1.0 error",
                "error",
                "Volume: -1",
            ] {
                assert!(parse(text, true).is_err());
            }
            assert!(parse("Mute: unknown", false).is_err());
        }
    }
}
