use core::arch::asm;

#[repr(align(8))]
#[derive(Clone, Copy)]
pub struct PageTableEntry {
    pub entry: u64,
}

impl PageTableEntry {
    pub const fn new() -> Self {
        PageTableEntry { entry: 0 }
    }

    pub fn get_vpn(&self) -> usize {
        (self.entry >> 10) as usize
    }

    pub fn get_ppn(&self) -> usize {
        (self.entry >> 10) as usize
    }

    pub fn get_flags(&self) -> u64 {
        self.entry & 0xfff
    }

    pub fn is_valid(&self) -> bool {
        self.entry & 1 == 1
    }

    pub fn is_leaf(&self) -> bool {
        self.entry & 0x1ff == 0
    }

    pub fn validate(&mut self) {
        self.entry |= 1;
    }

    pub fn invalidate(&mut self) {
        self.entry &= !1;
    }

    pub fn set_vpn(&mut self, vpn: usize) -> &mut Self {
        self.entry |= (vpn as u64) << 10;
        self
    }

    pub fn set_ppn(&mut self, ppn: usize) -> &mut Self {
        self.entry |= (ppn as u64) << 10;
        self
    }

    pub fn set_flags(&mut self, flags: u64) -> &mut Self {
        self.entry |= flags;
        self
    }

    pub fn clear_flags(&mut self) -> &mut Self {
        self.entry &= !0xfff;
        self
    }
}

#[repr(align(4096))]
pub struct PageTable {
    pub entries: [PageTableEntry; 512],
}

impl PageTable {
    pub const fn new() -> Self {
        PageTable {
            entries: [PageTableEntry::new(); 512],
        }
    }

    pub fn switch(&self, asid: usize) {
        let mode = 9;
        let ppn = self as *const _ as usize >> 12;
        let satp = mode << 60 | asid << 44 | ppn;
        unsafe {
            asm!(
                "
                csrw satp, {0}
                sfence.vma
                ",

                in(reg) satp,
            );
        }
    }

    fn get_next_level_table(&self, index: usize) -> *mut PageTable {
        let addr = self.entries[index].get_ppn() << 12;
        addr as *mut PageTable
    }

    /* Only for root page table */
    pub fn map(&mut self, vaddr: usize, paddr: usize, size: usize) {
        
    }

    fn walk(&mut self, vaddr: usize, level: usize) -> Option<&mut PageTable> {
        if level == 0 {
            return Some(self);
        }
        let vpn = vaddr >> 12;
        let index = vpn & 0x1ff;
        let entry = &self.entries[index];
        if !entry.is_valid() {
            return None;
        }
        if entry.is_leaf() {
            return Some(self);
        }
        let next_level_table = self.get_next_level_table(index);
        unsafe { &mut *next_level_table }.walk(vaddr, level - 1)
    }

    pub fn get_entry_addr(&self, vaddr: usize, level: usize) -> usize {
        let vpn = vaddr >> 12;
        let index = vpn & 0x1ff;
        let entry = self.entries[index];
        let ppn = entry.entry & 0x0000_ffff_ffff_f000;
        ppn as usize
    }

    pub fn get_entry_flags(&self, vaddr: usize, level: usize) -> u64 {
        let vpn = vaddr >> 12;
        let index = vpn & 0x1ff;
        let entry = self.entries[index].entry;
        entry & 0xfff
    }

    pub fn get_entry_type(&self, vaddr: usize, level: usize) -> u64 {
        let vpn = vaddr >> 12;
        let index = vpn & 0x1ff;
        let entry = self.entries[index].entry;
        entry & 0x1ff
    }
}


pub fn get_page(vaddr: usize, root_page_table: &PageTable) -> usize {
    let mut page_table = root_page_table;
    for level in (0..3).rev() {
        let index = (vaddr >> (12 + 9 * level)) & 0x1ff;
        let entry = page_table.entries[index].entry;
        if entry & 1 == 0 {
            return 0;
        }
        if entry & 0x1ff == 0 {
            return 0;
        }
        page_table = unsafe { &*page_table.get_next_level_table(index) };
    }
    page_table.get_entry_addr(vaddr, 0)
}
