pub trait UsbHostController: Send + Sync {
    fn poll_events(&self);
}
