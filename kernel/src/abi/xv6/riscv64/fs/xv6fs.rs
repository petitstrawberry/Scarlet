use core::mem;

/// xv6-style directory entry
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Dirent {
    pub inum: u16,      // inode number
    pub name: [u8; 14], // file name (null-terminated)
}

impl Dirent {
    pub const DIRENT_SIZE: usize = mem::size_of::<Dirent>();

    pub fn new(inum: u16, name: &str) -> Self {
        let mut dirent = Dirent {
            inum,
            name: [0; 14],
        };
        
        // Copy name, ensuring null termination
        let name_bytes = name.as_bytes();
        let copy_len = name_bytes.len().min(13); // Leave space for null terminator
        dirent.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
        dirent.name[copy_len] = 0; // Null terminate
        
        dirent
    }
    
    pub fn name_str(&self) -> &str {
        // Find null terminator
        let mut end = 0;
        while end < self.name.len() && self.name[end] != 0 {
            end += 1;
        }
        
        // Convert to string
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
    
    /// Convert Dirent to byte array for reading
    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(
                self as *const Dirent as *const u8,
                mem::size_of::<Dirent>()
            )
        }
    }
}

/// xv6-style file stat structure
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Stat {
    pub dev: i32,     // File system's disk device
    pub ino: u32,     // Inode number
    pub file_type: u16, // Type of file (T_DIR, T_FILE, T_DEVICE)
    pub nlink: u16,     // Number of links to file
    pub size: u64,    // Size of file in bytes
}

// xv6 file type constants
pub const T_DIR: u16 = 1;    // Directory
pub const T_FILE: u16 = 2;   // File
pub const T_DEVICE: u16 = 3; // Device
