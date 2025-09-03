#!/usr/bin/env python3
"""
FAT32 Directory Detailed Analysis Tool
Provides comprehensive analysis of a single FAT32 image file
"""

import sys
import struct

def get_fat32_info(image_path):
    """Extract FAT32 filesystem information from boot sector"""
    with open(image_path, 'rb') as f:
        # Read boot sector
        boot_sector = f.read(512)
        
        # Parse BPB (BIOS Parameter Block)
        bytes_per_sector = struct.unpack('<H', boot_sector[11:13])[0]
        sectors_per_cluster = boot_sector[13]
        reserved_sectors = struct.unpack('<H', boot_sector[14:16])[0]
        fat_count = boot_sector[16]
        total_sectors = struct.unpack('<L', boot_sector[32:36])[0]
        sectors_per_fat = struct.unpack('<L', boot_sector[36:40])[0]
        root_cluster = struct.unpack('<L', boot_sector[44:48])[0]
        
        # Calculate important offsets
        fat_start = reserved_sectors
        data_start = fat_start + (fat_count * sectors_per_fat)
        cluster_size = bytes_per_sector * sectors_per_cluster
        
        return {
            'bytes_per_sector': bytes_per_sector,
            'sectors_per_cluster': sectors_per_cluster,
            'cluster_size': cluster_size,
            'fat_start': fat_start,
            'data_start': data_start,
            'root_cluster': root_cluster,
            'sectors_per_fat': sectors_per_fat
        }

def cluster_to_offset(cluster_num, fs_info):
    """Convert cluster number to byte offset in the image"""
    return (fs_info['data_start'] + (cluster_num - 2) * fs_info['sectors_per_cluster']) * fs_info['bytes_per_sector']

def read_fat_entry(image_path, cluster_num, fs_info):
    """Read FAT entry for given cluster number"""
    fat_offset = fs_info['fat_start'] * fs_info['bytes_per_sector']
    entry_offset = fat_offset + (cluster_num * 4)
    
    with open(image_path, 'rb') as f:
        f.seek(entry_offset)
        entry = struct.unpack('<L', f.read(4))[0]
        return entry & 0x0FFFFFFF  # Mask off upper 4 bits

def decode_lfn_entry(entry_data):
    """Decode Long File Name entry"""
    sequence = entry_data[0] & 0x1F
    is_last = (entry_data[0] & 0x40) != 0
    
    # Extract name parts (UTF-16LE)
    name_part1 = entry_data[1:11].decode('utf-16le', errors='replace').rstrip('\x00\xff')
    name_part2 = entry_data[14:26].decode('utf-16le', errors='replace').rstrip('\x00\xff')
    name_part3 = entry_data[28:32].decode('utf-16le', errors='replace').rstrip('\x00\xff')
    
    name_fragment = name_part1 + name_part2 + name_part3
    return sequence, is_last, name_fragment

def decode_sfn_entry(entry_data):
    """Decode Short File Name entry"""
    name = entry_data[:8].decode('ascii', errors='replace').rstrip()
    ext = entry_data[8:11].decode('ascii', errors='replace').rstrip()
    attributes = entry_data[11]
    
    # Combine name and extension
    if ext:
        full_name = f"{name}.{ext}"
    else:
        full_name = name
    
    # Determine entry type
    if attributes & 0x10:  # Directory
        entry_type = "DIR"
    elif attributes & 0x08:  # Volume label
        entry_type = "VOL"
        full_name = name + ext  # Volume labels don't use dots
    else:
        entry_type = "FILE"
    
    # Get cluster and size info
    first_cluster_low = struct.unpack('<H', entry_data[26:28])[0]
    first_cluster_high = struct.unpack('<H', entry_data[20:22])[0]
    first_cluster = (first_cluster_high << 16) | first_cluster_low
    file_size = struct.unpack('<L', entry_data[28:32])[0]
    
    return {
        'type': entry_type,
        'name': full_name,
        'attributes': attributes,
        'first_cluster': first_cluster,
        'file_size': file_size
    }

def analyze_fat32_directory_cluster(image_path, cluster_start_offset, cluster_size=512, fs_info=None):
    """
    Analyze a FAT32 directory cluster to find free space and detailed entry information
    
    Args:
        image_path: Path to the FAT32 image file
        cluster_start_offset: Byte offset where the cluster starts
        cluster_size: Size of the cluster in bytes (default 512)
        fs_info: Filesystem information dictionary
    """
    print(f"Analyzing cluster at offset 0x{cluster_start_offset:x}")
    print("=" * 60)
    
    with open(image_path, 'rb') as f:
        f.seek(cluster_start_offset)
        cluster_data = f.read(cluster_size)
    
    # Directory entries are 32 bytes each
    entry_size = 32
    entries_per_cluster = cluster_size // entry_size
    
    print(f"Cluster size: {cluster_size} bytes")
    print(f"Entries per cluster: {entries_per_cluster}")
    print()
    
    free_entries = []
    used_entries = []
    lfn_fragments = []
    current_lfn_name = ""
    
    for i in range(entries_per_cluster):
        entry_offset = i * entry_size
        entry_data = cluster_data[entry_offset:entry_offset + entry_size]
        
        # Check if entry is free
        first_byte = entry_data[0]
        
        if first_byte == 0x00:
            # Entry is free (and all subsequent entries are free)
            free_entries.append((i, "FREE (end of directory)"))
            break
        elif first_byte == 0xE5:
            # Entry is deleted/free
            free_entries.append((i, "FREE (deleted)"))
        else:
            # Entry is used
            if entry_data[11] & 0x0F == 0x0F:  # LFN entry
                seq, is_last, name_frag = decode_lfn_entry(entry_data)
                if is_last:
                    current_lfn_name = name_frag
                    lfn_fragments = [name_frag]
                else:
                    lfn_fragments.insert(0, name_frag)
                used_entries.append((i, f"LFN[{seq}]: '{name_frag}'" + (" (LAST)" if is_last else "")))
            else:
                # Regular directory entry
                entry_info = decode_sfn_entry(entry_data)
                
                # If we have LFN fragments, combine them
                if lfn_fragments:
                    long_name = "".join(lfn_fragments)
                    description = f"{entry_info['type']}: '{long_name}' (SFN: {entry_info['name']})"
                else:
                    description = f"{entry_info['type']}: {entry_info['name']}"
                
                # Add cluster and size info for files/directories
                if entry_info['first_cluster'] > 0:
                    description += f" [cluster={entry_info['first_cluster']}"
                    if entry_info['type'] == 'FILE':
                        description += f", size={entry_info['file_size']}"
                    description += "]"
                
                used_entries.append((i, description))
                
                # Reset LFN state
                lfn_fragments = []
                current_lfn_name = ""
    
    # Print detailed analysis
    print("DETAILED ENTRY ANALYSIS:")
    for entry_num, description in used_entries:
        print(f"  Entry {entry_num:2d} (offset 0x{entry_num*32:03x}): {description}")
    
    print()
    print("FREE ENTRIES:")
    for entry_num, description in free_entries:
        print(f"  Entry {entry_num:2d} (offset 0x{entry_num*32:03x}): {description}")
    
    print()
    print(f"Total used entries: {len(used_entries)}")
    print(f"Total free entries: {len(free_entries)}")
    print(f"Space utilization: {len(used_entries)}/{entries_per_cluster} ({len(used_entries)/entries_per_cluster*100:.1f}%)")
    
    # Check if there's enough space for a new entry (LFN + SFN = 2 entries minimum)
    consecutive_free = 0
    max_consecutive_free = 0
    
    for i in range(entries_per_cluster):
        entry_offset = i * entry_size
        first_byte = cluster_data[entry_offset]
        
        if first_byte == 0x00 or first_byte == 0xE5:
            consecutive_free += 1
            max_consecutive_free = max(max_consecutive_free, consecutive_free)
        else:
            consecutive_free = 0
    
    print(f"Maximum consecutive free entries: {max_consecutive_free}")
    print(f"Can fit new entry (needs 2+ slots): {'YES' if max_consecutive_free >= 2 else 'NO'}")
    
    return len(used_entries), len(free_entries), max_consecutive_free

def analyze_cluster_chain(image_path, start_cluster, fs_info):
    """Follow a cluster chain and return all cluster numbers"""
    clusters = []
    current_cluster = start_cluster
    
    while current_cluster >= 2 and current_cluster < 0x0FFFFFF8:
        clusters.append(current_cluster)
        current_cluster = read_fat_entry(image_path, current_cluster, fs_info)
        
        # Prevent infinite loops
        if len(clusters) > 100:
            print("Warning: Cluster chain seems too long, stopping")
            break
    
    return clusters

def main():
    # Get image path from command line or use default
    if len(sys.argv) > 1:
        image_path = sys.argv[1]
    else:
        image_path = "fat32-test.img"
    
    # Check if help is requested
    if image_path in ['-h', '--help', 'help']:
        print("FAT32 Detailed Analysis Tool")
        print("Usage:")
        print(f"  {sys.argv[0]} [image_path]")
        print(f"  {sys.argv[0]} --help")
        print()
        print("Examples:")
        print(f"  {sys.argv[0]}                    # Analyze fat32-test.img")
        print(f"  {sys.argv[0]} my_image.img       # Analyze my_image.img")
        print(f"  {sys.argv[0]} /path/to/disk.img  # Analyze disk.img with full path")
        return
    
    print(f"Detailed FAT32 Analysis: {image_path}")
    print("=" * 80)
    
    try:
        # Get filesystem information
        fs_info = get_fat32_info(image_path)
        
        print("FILESYSTEM INFORMATION:")
        print(f"  Bytes per sector: {fs_info['bytes_per_sector']}")
        print(f"  Sectors per cluster: {fs_info['sectors_per_cluster']}")
        print(f"  Cluster size: {fs_info['cluster_size']} bytes")
        print(f"  FAT start sector: {fs_info['fat_start']}")
        print(f"  Data start sector: {fs_info['data_start']}")
        print(f"  Root directory cluster: {fs_info['root_cluster']}")
        print()
        
        # Analyze root directory
        root_offset = cluster_to_offset(fs_info['root_cluster'], fs_info)
        print("ROOT DIRECTORY ANALYSIS:")
        print("-" * 40)
        
        used, free, max_free = analyze_fat32_directory_cluster(
            image_path, root_offset, fs_info['cluster_size'], fs_info
        )
        
        # Check if root directory has been extended
        root_clusters = analyze_cluster_chain(image_path, fs_info['root_cluster'], fs_info)
        print(f"\nROOT DIRECTORY CLUSTER CHAIN: {root_clusters}")
        
        if len(root_clusters) > 1:
            print(f"\nROOT DIRECTORY HAS BEEN EXTENDED!")
            for i, cluster_num in enumerate(root_clusters[1:], 1):
                print(f"\nEXTENDED CLUSTER {i} (Cluster #{cluster_num}):")
                print("-" * 50)
                ext_offset = cluster_to_offset(cluster_num, fs_info)
                analyze_fat32_directory_cluster(
                    image_path, ext_offset, fs_info['cluster_size'], fs_info
                )
        
        print(f"\nSUMMARY:")
        print(f"  Root directory uses {len(root_clusters)} cluster(s)")
        print(f"  Total entries used: {used}")
        print(f"  Space available for new entries: {'YES' if max_free >= 2 else 'NO'}")
        
    except FileNotFoundError:
        print(f"Error: File '{image_path}' not found!")
        print()
        print("Usage:")
        print(f"  {sys.argv[0]} [image_path]")
        print()
        print("Examples:")
        print(f"  {sys.argv[0]}                    # Analyze fat32-test.img (default)")
        print(f"  {sys.argv[0]} my_image.img       # Analyze my_image.img")
        print(f"  {sys.argv[0]} /path/to/disk.img  # Analyze disk.img with full path")
        sys.exit(1)
    except Exception as e:
        print(f"Error analyzing {image_path}: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)

if __name__ == "__main__":
    main()
