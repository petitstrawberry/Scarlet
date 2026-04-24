#!/usr/bin/env python3
"""Generate a minimal Windows ARM64 PE executable that calls NtTerminateProcess.

Produces a valid PE32+ binary with:
- ARM64 machine type
- Single .text section
- 16 data directories (all zero)
- Proper optional header fields for Windows loader compatibility

Usage:
    python3 gen_min_pe.py [output_path]

Dependencies:
    pip3 install lief (for verification only)
"""
import struct
import sys


def make_pe(code_bytes: bytes) -> bytes:
    FILE_ALIGN = 0x200
    SECT_ALIGN = 0x1000

    num_sections = 1
    opt_hdr_size = 240  # PE32+ standard(32) + windows-specific(88) + 16*8 data dirs
    headers_raw = 0x40 + 4 + 20 + opt_hdr_size + 40 * num_sections
    size_of_headers = ((headers_raw + FILE_ALIGN - 1) // FILE_ALIGN) * FILE_ALIGN
    code_raw_size = ((len(code_bytes) + FILE_ALIGN - 1) // FILE_ALIGN) * FILE_ALIGN
    size_of_image = SECT_ALIGN * 2

    pe = bytearray(size_of_headers + code_raw_size)

    # DOS Header
    pe[0:2] = b'MZ'
    struct.pack_into('<I', pe, 0x3C, 0x40)

    # PE Signature
    pe[0x40:0x44] = b'PE\0\0'

    # COFF Header (20 bytes at 0x44)
    struct.pack_into('<H', pe, 0x44, 0xAA64)   # Machine: ARM64
    struct.pack_into('<H', pe, 0x46, num_sections)
    struct.pack_into('<I', pe, 0x48, 0)        # TimeDateStamp
    struct.pack_into('<I', pe, 0x4C, 0)        # PointerToSymbolTable
    struct.pack_into('<I', pe, 0x50, 0)        # NumberOfSymbols
    struct.pack_into('<H', pe, 0x54, opt_hdr_size)
    struct.pack_into('<H', pe, 0x56, 0x0022)   # EXECUTABLE_IMAGE | LARGE_ADDRESS_AWARE

    # Optional Header PE32+ (at 0x58)
    struct.pack_into('<H', pe, 0x58, 0x20B)    # PE32+ magic
    pe[0x5A] = 14                               # MajorLinkerVersion
    struct.pack_into('<I', pe, 0x5C, len(code_bytes))  # SizeOfCode
    struct.pack_into('<I', pe, 0x60, 0)        # SizeOfInitializedData
    struct.pack_into('<I', pe, 0x64, 0)        # SizeOfUninitializedData
    struct.pack_into('<I', pe, 0x68, SECT_ALIGN)       # AddressOfEntryPoint
    struct.pack_into('<I', pe, 0x6C, SECT_ALIGN)       # BaseOfCode
    struct.pack_into('<Q', pe, 0x70, 0x140000000)      # ImageBase
    struct.pack_into('<I', pe, 0x78, SECT_ALIGN)       # SectionAlignment
    struct.pack_into('<I', pe, 0x7C, FILE_ALIGN)       # FileAlignment
    struct.pack_into('<H', pe, 0x80, 10)       # MajorOperatingSystemVersion
    struct.pack_into('<H', pe, 0x84, 10)       # MajorImageVersion
    struct.pack_into('<H', pe, 0x88, 10)       # MajorSubsystemVersion
    struct.pack_into('<I', pe, 0x90, size_of_image)
    struct.pack_into('<I', pe, 0x94, size_of_headers)
    struct.pack_into('<H', pe, 0x9C, 1)        # Subsystem: NATIVE
    # DllCharacteristics: HIGH_ENTROPY_VA|DYNAMIC_BASE|NX_COMPAT|GUARD_CF|TERMINAL_SERVER_AWARE
    struct.pack_into('<H', pe, 0x9E, 0x8160)
    struct.pack_into('<Q', pe, 0xA0, 0x100000) # SizeOfStackReserve
    struct.pack_into('<Q', pe, 0xA8, 0x1000)   # SizeOfStackCommit
    struct.pack_into('<Q', pe, 0xB0, 0x100000) # SizeOfHeapReserve
    struct.pack_into('<Q', pe, 0xB8, 0x1000)   # SizeOfHeapCommit
    struct.pack_into('<I', pe, 0xC0, 0)        # LoaderFlags
    struct.pack_into('<I', pe, 0xC4, 16)       # NumberOfRvaAndSizes

    # 16 data directories = 128 bytes of zeros (0xC8..0x147)

    # Section Header (.text at 0x148)
    sec_off = 0x148
    pe[sec_off:sec_off+8] = b'.text\x00\x00\x00'
    struct.pack_into('<I', pe, sec_off + 8, len(code_bytes))   # VirtualSize
    struct.pack_into('<I', pe, sec_off + 12, SECT_ALIGN)       # VirtualAddress
    struct.pack_into('<I', pe, sec_off + 16, code_raw_size)    # SizeOfRawData
    struct.pack_into('<I', pe, sec_off + 20, size_of_headers)  # PointerToRawData
    # Chars: CNT_CODE | MEM_EXECUTE | MEM_READ
    struct.pack_into('<I', pe, sec_off + 36, 0x60000020)

    # Code
    pe[size_of_headers:size_of_headers+len(code_bytes)] = code_bytes

    return bytes(pe)


def main() -> None:
    output = sys.argv[1] if len(sys.argv) > 1 else "test_exit.exe"

    # ARM64: NtTerminateProcess(CurrentProcess=-1, ExitStatus=0)
    # SVC #0x2C = syscall #44
    code = struct.pack('<IIII',
        0x92800000,  # MOVN X0, #0 → X0 = -1 (CurrentProcess handle)
        0xD2800001,  # MOV X1, #0 (ExitStatus)
        0xD4000581,  # SVC #0x2C (NtTerminateProcess)
        0xD65F03C0,  # RET
    )

    pe_data = make_pe(code)

    with open(output, 'wb') as f:
        f.write(pe_data)
    print(f"Wrote {len(pe_data)} bytes to {output}")

    try:
        import lief
        b = lief.parse(output)
        if b is None:
            print("ERROR: LIEF failed to parse generated PE")
            sys.exit(1)
        print(f"Verified: {b.header.machine} entry={hex(b.optional_header.addressof_entrypoint)} "
              f"datadirs={b.optional_header.numberof_rva_and_size}")
    except ImportError:
        pass


if __name__ == "__main__":
    main()
