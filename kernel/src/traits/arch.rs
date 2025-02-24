pub trait Arch {
    fn init(&mut self, cpu_id: usize);
}