use scarlet::{hosted, task};

#[test]
fn private_stack_and_heap_keep_the_virtual_host_page_offset() {
    const Q: usize = 4096;
    let _platform = hosted::prepare(4, 32 * 1024 * 1024).unwrap();
    let granule = scarlet_hosted_runtime::ram::host_page_size().unwrap();
    let task = task::new_user_task("congruent-frames".into(), 0);
    task.init();
    for quarter in 0..granule / Q {
        let start = 0x1000_0000 + quarter * Q;
        for map in [
            task.allocate_data_pages(start, 9).unwrap(),
            task.allocate_stack_pages(start + 0x1000_0000, 7).unwrap(),
        ] {
            assert_eq!(map.pmarea.start as usize % granule, map.vmarea.start % granule);
            assert_eq!(map.pmarea.byte_len(), Some(map.vmarea.size() as u64));
        }
        task.free_data_pages(start, 9);
        task.free_stack_pages(start + 0x1000_0000, 7);
    }
}
