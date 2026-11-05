//! Transitional fixture wiring for the normal native android-runtime app.
//! Only the receiving input endpoints enter the Android View.
use std::{
    environment::{Environment, FilePublication, HandleMapping, VfsView},
    handle::Handle,
    socket::Socket,
    string::String,
    vec::Vec,
};
const DISPLAY_WIDTH: u32 = 480;
const DISPLAY_HEIGHT: u32 = 640;
pub struct Setup {
    listener: Socket,
    keyboard: Handle,
    touch: Handle,
    mouse: Handle,
    mouse_reader: Handle,
    display_control: Handle,
}
pub struct Display {
    pid: u32,
    _bridge: super::input_bridge::Bridge,
}
pub fn prepare(android: &VfsView) -> Result<Option<Setup>, &'static str> {
    if option_env!("SCARLET_ANDROID_SWS_NATIVE") != Some("1") {
        return Ok(None);
    }
    let (keyboard, key_events) =
        std::input::create_virtual_keyboard().map_err(|_| "keyboard channel")?;
    let (touch, touch_events) =
        std::input::create_virtual_touchscreen(DISPLAY_WIDTH, DISPLAY_HEIGHT)
            .map_err(|_| "touch channel")?;
    let (mouse_reader, mouse) = std::task::pipe().map_err(|_| "absolute mouse channel")?;
    android
        .create_directory("/dev/input")
        .or_else(|_| android.open("/dev/input", 0).map(|_| ()))
        .map_err(|_| "Android input directory")?;
    android
        .publish_files(
            "/dev/input",
            &[
                FilePublication {
                    name: "event0",
                    file: &key_events,
                    access: 1,
                },
                FilePublication {
                    name: "event1",
                    file: &touch_events,
                    access: 1,
                },
            ],
        )
        .map_err(|_| "Android input publication")?;
    let mut random = [0u8; 16];
    let mut filled = 0;
    while filled < random.len() {
        // SAFETY: the remaining stack buffer is writable for the synchronous call.
        let count = unsafe {
            std::syscall::syscall2(
                std::syscall::Syscall::GetRandom,
                random[filled..].as_mut_ptr() as usize,
                random.len() - filled,
            )
        };
        if count == 0 || count > random.len() - filled {
            return Err("display endpoint identity");
        }
        filled += count;
    }
    let mut name = String::from("/tmp/scarlet-display-");
    for byte in random {
        name.push_str(&std::format!("{byte:02x}"));
    }
    name.push_str(".sock");
    let listener = Socket::new().map_err(|_| "display socket")?;
    listener.bind(&name).map_err(|_| "display bind")?;
    listener.listen(1).map_err(|_| "display listen")?;
    let storage =
        VfsView::create("tmpfs", "size=64K").map_err(|_| "display configuration storage")?;
    let config = storage
        .open("/endpoint", 0xc2)
        .map_err(|_| "display configuration file")?;
    let control_storage =
        VfsView::create("tmpfs", "size=4K").map_err(|_| "display control storage")?;
    let display_control = control_storage
        .open("/request", 0xc2)
        .map_err(|_| "display control file")?;
    let text = std::format!("SAD1\n{name}\n{DISPLAY_WIDTH} {DISPLAY_HEIGHT}\n");
    config
        .as_stream()
        .map_err(|_| "display configuration stream")?
        .write_all(text.as_bytes())
        .map_err(|_| "display configuration write")?;
    android
        .create_directory("/scarlet/runtime")
        .or_else(|_| android.open("/scarlet/runtime", 0).map(|_| ()))
        .map_err(|_| "display configuration directory")?;
    android
        .publish_files(
            "/scarlet/runtime",
            &[
                FilePublication {
                    name: "display-endpoint",
                    file: &config,
                    access: 1,
                },
                FilePublication {
                    name: "display-request",
                    file: &display_control,
                    access: 1,
                },
            ],
        )
        .map_err(|_| "display configuration publication")?;
    Ok(Some(Setup {
        listener,
        keyboard,
        touch,
        mouse,
        mouse_reader,
        display_control,
    }))
}
impl Setup {
    pub fn start(
        self,
        environment: &Environment,
        stdio: &[HandleMapping<'_>],
    ) -> Result<Display, &'static str> {
        let native = environment
            .root("scarlet")
            .map_err(|_| "display native View")?;
        let executable = native
            .open("/bin/android-runtime", 0)
            .map_err(|_| "native Android display executable")?;
        let mut handles: Vec<_> = stdio
            .iter()
            .map(|h| HandleMapping {
                source: h.source,
                target: h.target,
            })
            .collect();
        handles.extend([
            HandleMapping {
                source: &self.keyboard,
                target: 3,
            },
            HandleMapping {
                source: &self.touch,
                target: 4,
            },
            HandleMapping {
                source: self.listener.as_handle(),
                target: 5,
            },
            HandleMapping {
                source: &self.mouse,
                target: 6,
            },
            HandleMapping {
                source: &self.display_control,
                target: 7,
            },
        ]);
        let bridge = super::input_bridge::Bridge::start(environment, self.mouse_reader, stdio)?;
        let width = std::format!("{DISPLAY_WIDTH}");
        let height = std::format!("{DISPLAY_HEIGHT}");
        let pid = environment
            .spawn(
                &executable,
                &[
                    "/bin/android-runtime",
                    "5",
                    "3",
                    "4",
                    "6",
                    "7",
                    width.as_str(),
                    height.as_str(),
                ],
                &["PATH=/bin"],
                "/",
                &handles,
            )
            .map_err(|_| "native display spawn")?;
        std::println!("SCARLET_ANDROID_NATIVE_DISPLAY_STARTED pid={}", pid);
        Ok(Display {pid, _bridge: bridge})
    }
}
impl Display {
    pub fn set_session(&self, pid: u32) { self._bridge.set_session(pid); }
}
impl Drop for Display {
    fn drop(&mut self) {
        // SAFETY: only the renderer spawned and owned by this fixture is targeted.
        let _ = unsafe { std::syscall::syscall2(std::syscall::Syscall::Kill, self.pid as usize, 9) };
        let _ = super::wait_for_program(self.pid as i32);
    }
}
