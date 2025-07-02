#!/usr/bin/env python3
"""
Script to set OSABI for ELF files in Scarlet
This script safely sets the OSABI field in ELF files used by Scarlet.
"""

import sys
import struct
import os
import shutil

def set_osabi(elf_file, osabi_value):
    """Set OSABI for ELF file"""
    
    # Check if file exists
    if not os.path.exists(elf_file):
        print(f"Error: ELF file '{elf_file}' not found")
        return False
    
    # Create backup file
    backup_file = f"{elf_file}.backup"
    shutil.copy2(elf_file, backup_file)
    
    try:
        with open(elf_file, 'r+b') as f:
            # Check ELF magic number
            f.seek(0)
            magic = f.read(4)
            if magic != b'\x7fELF':
                print(f"Error: '{elf_file}' is not a valid ELF file")
                return False
            
            # Set value to OSABI field (offset 7)
            f.seek(7)
            f.write(struct.pack('B', osabi_value))
            
        print(f"Successfully set OSABI to {osabi_value} in {elf_file}")
        
        # Remove backup file
        os.remove(backup_file)
        return True
        
    except Exception as e:
        print(f"Error: Failed to set OSABI in {elf_file}: {e}")
        # Restore from backup on error
        shutil.move(backup_file, elf_file)
        return False

def main():
    if len(sys.argv) != 3:
        print("Usage: set_osabi.py <elf_file> <osabi_value>")
        sys.exit(1)
    
    elf_file = sys.argv[1]
    try:
        osabi_value = int(sys.argv[2])
        if not 0 <= osabi_value <= 255:
            print("Error: OSABI value must be between 0 and 255")
            sys.exit(1)
    except ValueError:
        print("Error: OSABI value must be a valid integer")
        sys.exit(1)
    
    if set_osabi(elf_file, osabi_value):
        sys.exit(0)
    else:
        sys.exit(1)

if __name__ == "__main__":
    main()
