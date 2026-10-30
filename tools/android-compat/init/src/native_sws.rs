//! Optional compositor owned by the private graphics execution fixture.
use std::{
    environment::{Environment, HandleMapping},
    println,
};
pub struct Compositor(u32);
impl Compositor {
    pub fn start(
        environment: &Environment,
        handles: &[HandleMapping<'_>],
    ) -> Result<Option<Self>, &'static str> {
        if option_env!("SCARLET_ANDROID_SWS_NATIVE") != Some("1") {
            return Ok(None);
        }
        let native = environment.root("scarlet").map_err(|_| "SWS native view")?;
        let executable = native.open("/bin/sws", 0).map_err(|_| "SWS executable")?;
        let pid = environment
            .spawn(
                &executable,
                &["/bin/sws"],
                &["PATH=/bin", "SWS_BACKEND=cpu"],
                "/",
                handles,
            )
            .map_err(|_| "SWS spawn")?;
        println!("SCARLET_ANDROID_SWS_STARTED pid={}", pid);
        Ok(Some(Self(pid)))
    }
}
impl Drop for Compositor {
    fn drop(&mut self) {
        // SAFETY: terminate only the compositor spawned by this private fixture.
        let result =
            unsafe { std::syscall::syscall2(std::syscall::Syscall::Kill, self.0 as usize, 9) };
        let (waited, status) = std::task::waitpid(self.0 as i32, 0);
        println!(
            "SCARLET_ANDROID_SWS_STOPPED pid={} waited={} status={} stop_result={}",
            self.0, waited, status, result
        );
    }
}
