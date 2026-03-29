#!/usr/bin/env python3
import struct, sys

def le16(v): return struct.pack('<H', v)
def le32(v): return struct.pack('<I', v)
def le64(v): return struct.pack('<Q', v)

IMAGE_FILE_MACHINE_ARM64 = 0xAA64
PE32PLUS_MAGIC = 0x20B
IMAGE_SUBSYSTEM_NATIVE = 1
IMAGE_SCN_CNT_CODE = 0x20
IMAGE_SCN_MEM_EXECUTE = 0x20000000
IMAGE_SCN_MEM_READ = 0x40000000
IMAGE_FILE_EXECUTABLE_IMAGE = 0x0002

FILE_ALIGN = 0x200
SECT_ALIGN = 0x1000

# ARM64: mov x0, #-1; mov x1, #0; mov x8, #0x2C; svc #0
# NtTerminateProcess(CurrentProcess=-1, ExitStatus=0)
code = struct.pack('<IIII', 0x92800000, 0xD2800001, 0xD2800588, 0xD4000001)
code_size = len(code)
code_raw_size = ((code_size + FILE_ALIGN - 1) // FILE_ALIGN) * FILE_ALIGN

num_sections = 1
opt_hdr_size = 32 + 88  # PE32+ standard (32) + windows-specific (88), 0 data dirs
headers_raw = 0x40 + 4 + 20 + opt_hdr_size + 40 * num_sections
size_of_headers = ((headers_raw + FILE_ALIGN - 1) // FILE_ALIGN) * FILE_ALIGN

entry_rva = SECT_ALIGN
size_of_image = SECT_ALIGN * 2

pe = bytearray(size_of_headers + code_raw_size)

# DOS Header
struct.pack_into('<2s', pe, 0, b'MZ')
struct.pack_into('<I', pe, 0x3C, 0x40)

# PE Signature
struct.pack_into('<4s', pe, 0x40, b'PE\0\0')

# COFF Header (20 bytes at 0x44)
off = 0x44
struct.pack_into('<H', pe, off, IMAGE_FILE_MACHINE_ARM64); off += 2
struct.pack_into('<H', pe, off, num_sections); off += 2
struct.pack_into('<I', pe, off, 0); off += 4  # TimeDateStamp
struct.pack_into('<I', pe, off, 0); off += 4  # PointerToSymbolTable
struct.pack_into('<I', pe, off, 0); off += 4  # NumberOfSymbols
struct.pack_into('<H', pe, off, opt_hdr_size); off += 2  # SizeOfOptionalHeader
struct.pack_into('<H', pe, off, IMAGE_FILE_EXECUTABLE_IMAGE); off += 2  # Characteristics

# Optional Header PE32+ (at 0x58)
off = 0x58
struct.pack_into('<H', pe, off, PE32PLUS_MAGIC); off += 2
pe[off] = 0; off += 1  # MajorLinkerVersion
pe[off] = 0; off += 1  # MinorLinkerVersion
struct.pack_into('<I', pe, off, code_size); off += 4  # SizeOfCode
struct.pack_into('<I', pe, off, 0); off += 4  # SizeOfInitializedData
struct.pack_into('<I', pe, off, 0); off += 4  # SizeOfUninitializedData
struct.pack_into('<I', pe, off, entry_rva); off += 4  # AddressOfEntryPoint
struct.pack_into('<I', pe, off, SECT_ALIGN); off += 4  # BaseOfCode
struct.pack_into('<Q', pe, off, 0x140000000); off += 8  # ImageBase (PE32+: 8 bytes)

# Windows-Specific Fields
struct.pack_into('<I', pe, off, SECT_ALIGN); off += 4  # SectionAlignment
struct.pack_into('<I', pe, off, FILE_ALIGN); off += 4  # FileAlignment
struct.pack_into('<H', pe, off, 6); off += 2  # MajorOperatingSystemVersion
struct.pack_into('<H', pe, off, 0); off += 2  # MinorOperatingSystemVersion
struct.pack_into('<H', pe, off, 0); off += 2  # MajorImageVersion
struct.pack_into('<H', pe, off, 0); off += 2  # MinorImageVersion
struct.pack_into('<H', pe, off, 6); off += 2  # MajorSubsystemVersion
struct.pack_into('<H', pe, off, 0); off += 2  # MinorSubsystemVersion
struct.pack_into('<I', pe, off, 0); off += 4  # Win32VersionValue
struct.pack_into('<I', pe, off, size_of_image); off += 4  # SizeOfImage
struct.pack_into('<I', pe, off, size_of_headers); off += 4  # SizeOfHeaders
struct.pack_into('<I', pe, off, 0); off += 4  # CheckSum
struct.pack_into('<H', pe, off, IMAGE_SUBSYSTEM_NATIVE); off += 2  # Subsystem
struct.pack_into('<H', pe, off, 0); off += 2  # DllCharacteristics
struct.pack_into('<Q', pe, off, 0x100000); off += 8  # SizeOfStackReserve
struct.pack_into('<Q', pe, off, 0x1000); off += 8  # SizeOfStackCommit
struct.pack_into('<Q', pe, off, 0x100000); off += 8  # SizeOfHeapReserve
struct.pack_into('<Q', pe, off, 0x1000); off += 8  # SizeOfHeapCommit
struct.pack_into('<I', pe, off, 0); off += 4  # LoaderFlags
struct.pack_into('<I', pe, off, 0); off += 4  # NumberOfRvaAndSizes

# Section Header (.text, 40 bytes)
off = 0x58 + opt_hdr_size
pe[off:off+8] = b'.text\x00\x00\x00'  # Name
off += 8
struct.pack_into('<I', pe, off, code_size); off += 4  # VirtualSize
struct.pack_into('<I', pe, off, SECT_ALIGN); off += 4  # VirtualAddress
struct.pack_into('<I', pe, off, code_raw_size); off += 4  # SizeOfRawData
struct.pack_into('<I', pe, off, size_of_headers); off += 4  # PointerToRawData
struct.pack_into('<I', pe, off, 0); off += 4  # PointerToRelocations
struct.pack_into('<I', pe, off, 0); off += 4  # PointerToLinenumbers
struct.pack_into('<H', pe, off, 0); off += 2  # NumberOfRelocations
struct.pack_into('<H', pe, off, 0); off += 2  # NumberOfLinenumbers
struct.pack_into('<I', pe, off, IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ); off += 4

# Code
pe[size_of_headers:size_of_headers+code_size] = code

out = sys.argv[1] if len(sys.argv) > 1 else 'test_exit.exe'
with open(out, 'wb') as f:
    f.write(pe)
print(f"Wrote {len(pe)} bytes to {out}")
