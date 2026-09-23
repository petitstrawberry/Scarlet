use scarlet_probe_macros::{Answer, answer, passthrough};

#[derive(Answer)]
struct Example;

#[derive(scarlet_probe_macros_other::Answer)]
struct OtherExample;

#[passthrough]
fn from_attribute() -> u32 {
    answer!()
}

fn main() {
    assert_eq!(Example::answer(), 42);
    assert_eq!(from_attribute(), 42);
    assert_eq!(
        OtherExample::answer(),
        scarlet_probe_macros_other::answer!()
    );
    println!("SCARLET_NATIVE_PROC_MACRO_OK={}", answer!());
}
