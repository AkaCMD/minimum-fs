// minixfs.rs
// Minix 3 Filesystem Implementation

use crate::{
    cpu::Registers,
    process::{add_kernel_process_args, get_by_pid, set_running, set_waiting},
    syscall::{syscall_block_read, syscall_block_write},
};

use crate::{buffer::Buffer, cpu::memcpy};
use alloc::{
    boxed::Box,
    collections::BTreeMap,
    string::{String, ToString},
    vec,
};
use core::mem::{self, size_of};

pub const MAGIC: u16 = 0x4d5a;
pub const BLOCK_SIZE: u32 = 1024;
pub const NUM_IPTRS: usize = BLOCK_SIZE as usize / 4;
pub const S_IFDIR: u16 = 0o040_000;
pub const S_IFREG: u16 = 0o100_000;
/// The superblock describes the file system on the disk. It gives
/// us all the information we need to read the file system and navigate
/// the file system, including where to find the inodes and zones (blocks).
#[repr(C)]
#[derive(Debug)]
pub struct SuperBlock {
    pub ninodes: u32,
    pub pad0: u16,
    pub imap_blocks: u16,
    pub zmap_blocks: u16,
    pub first_data_zone: u16,
    pub log_zone_size: u16,
    pub pad1: u16,
    pub max_size: u32,
    pub zones: u32,
    pub magic: u16,
    pub pad2: u16,
    pub block_size: u16,
    pub disk_version: u8,
}

/// An inode stores the "meta-data" to a file. The mode stores the permissions
/// AND type of file. This is how we differentiate a directory from a file. A file
/// size is in here too, which tells us how many blocks we need to read. Finally, the
/// zones array points to where we can find the blocks, which is where the data
/// is contained for the file.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct Inode {
    pub mode: u16,
    pub nlinks: u16,
    pub uid: u16,
    pub gid: u16,
    pub size: u32,
    pub atime: u32,
    pub mtime: u32,
    pub ctime: u32,
    pub zones: [u32; 10],
}

/// Notice that an inode does not contain the name of a file. This is because
/// more than one file name may refer to the same inode. These are called "hard links"
/// Instead, a DirEntry essentially associates a file name with an inode as shown in
/// the structure below.
#[repr(C)]
pub struct DirEntry {
    pub inode: u32,
    pub name: [u8; 60],
}

/// The MinixFileSystem implements the FileSystem trait for the VFS.
pub struct MinixFileSystem;
// The plan for this in the future is to have a single inode cache. What we
// will do is have a cache of Node structures which will combine the Inode
// with the block drive.
static mut MFS_INODE_CACHE: [Option<BTreeMap<String, Inode>>; 8] =
    [None, None, None, None, None, None, None, None];

impl MinixFileSystem {
    /// Inodes are the meta-data of a file, including the mode (permissions and type) and
    /// the file's size. They are stored above the data zones, but to figure out where we
    /// need to go to get the inode, we first need the superblock, which is where we can
    /// find all of the information about the filesystem itself.
    pub fn get_inode(bdev: usize, inode_num: u32) -> Option<Inode> {
        // When we read, everything needs to be a multiple of a sector (512 bytes)
        // So, we need to have memory available that's at least 512 bytes, even if
        // we only want 10 bytes or 32 bytes (size of an Inode).
        let mut buffer = Buffer::new(1024);

        // Here is a little memory trick. We have a reference and it will refer to the
        // top portion of our buffer. Since we won't be using the super block and inode
        // simultaneously, we can overlap the memory regions.

        // For Rust-ers, I'm showing two ways here. The first way is to get a reference
        // from a pointer. You will see the &* a lot in Rust for references. Rust
        // makes dereferencing a pointer cumbersome, which lends to not using them.
        let super_block = unsafe { &*(buffer.get_mut() as *mut SuperBlock) };
        // I opted for a pointer here instead of a reference because we will be offsetting the inode by a certain amount.
        let inode = buffer.get_mut() as *mut Inode;
        // Read from the block device. The size is 1 sector (512 bytes) and our offset is past
        // the boot block (first 1024 bytes). This is where the superblock sits.
        syc_read(bdev, buffer.get_mut(), 512, 1024);
        if super_block.magic == MAGIC {
            // If we get here, we successfully read what we think is the super block.
            // The math here is 2 - one for the boot block, one for the super block. Then we
            // have to skip the bitmaps blocks. We have a certain number of inode map blocks (imap)
            // and zone map blocks (zmap).
            // The inode comes to us as a NUMBER, not an index. So, we need to subtract 1.
            let inode_offset = (2 + super_block.imap_blocks + super_block.zmap_blocks) as usize
                * BLOCK_SIZE as usize
                + ((inode_num as usize - 1) / (BLOCK_SIZE as usize / size_of::<Inode>()))
                    * BLOCK_SIZE as usize;

            // Now, we read the inode itself.
            // The block driver requires that our offset be a multiple of 512. We do that with the
            // inode_offset. However, we're going to be reading a group of inodes.
            syc_read(bdev, buffer.get_mut(), 1024, inode_offset as u32);

            // There are 1024 / size_of<Inode>() inodes in each read that we can do. However, we need to figure out which inode in that group we need to read. We just take the % of this to find out.
            let read_this_node =
                (inode_num as usize - 1) % (BLOCK_SIZE as usize / size_of::<Inode>());

            // We copy the inode over. This might not be the best thing since the Inode will
            // eventually have to change after writing.
            return unsafe { Some(*(inode.add(read_this_node))) };
        }
        // If we get here, some result wasn't OK. Either the super block
        // or the inode itself.
        None
    }
}

impl MinixFileSystem {
    /// Init is where we would cache the superblock and inode to avoid having to read
    /// it over and over again, like we do for read right now.
    fn cache_at(btm: &mut BTreeMap<String, Inode>, cwd: &String, inode_num: u32, bdev: usize) {
        let ino = Self::get_inode(bdev, inode_num).unwrap();
        let mut buf = Buffer::new(((ino.size + BLOCK_SIZE - 1) & !BLOCK_SIZE) as usize);
        let dirents = buf.get() as *const DirEntry;
        let sz = Self::read(bdev, &ino, buf.get_mut(), BLOCK_SIZE, 0);
        let num_dirents = sz as usize / size_of::<DirEntry>();

        // We start at 2 because the first two entries are . and ..
        for i in 2..num_dirents {
            unsafe {
                if (*dirents.add(i)).inode == 0 {
                    continue;
                }
                let ref d = *dirents.add(i);
                let d_ino = Self::get_inode(bdev, d.inode).unwrap();
                let mut new_cwd = String::with_capacity(120);
                for i in cwd.bytes() {
                    new_cwd.push(i as char);
                }
                // Add a directory separator between this inode and the next.
                // If we're the root (inode 1), we don't want to double up the
                // frontslash, so only do it for non-roots.
                if inode_num != 1 {
                    new_cwd.push('/');
                }
                for i in 0..60 {
                    if d.name[i] == 0 {
                        break;
                    }
                    new_cwd.push(d.name[i] as char);
                }
                new_cwd.shrink_to_fit();
                if d_ino.mode & S_IFDIR != 0 {
                    // This is a directory, cache these. This is a recursive call,
                    // which I don't really like.
                    Self::cache_at(btm, &new_cwd, d.inode, bdev);
                } else {
                    btm.insert(new_cwd, d_ino);
                }
            }
        }
    }

    // Run this ONLY in a process!
    pub fn init(bdev: usize) {
        if unsafe { MFS_INODE_CACHE[bdev - 1].is_none() } {
            let mut btm = BTreeMap::new();
            let cwd = String::from("/");

            // Let's look at the root (inode #1)
            Self::cache_at(&mut btm, &cwd, 1, bdev);
            unsafe {
                MFS_INODE_CACHE[bdev - 1] = Some(btm);
            }
        } else {
            println!(
                "KERNEL: Initialized an already initialized filesystem {}",
                bdev
            );
        }
    }

    pub fn refresh(bdev: usize) {
        let mut btm = BTreeMap::new();
        let cwd = String::from("/");

        // Let's look at the root (inode #1)
        Self::cache_at(&mut btm, &cwd, 1, bdev);
        unsafe {
            MFS_INODE_CACHE[bdev - 1] = Some(btm);
        }
    }

    /// Find a free inode in the filesystem
    pub fn find_free_inode(dev: usize) -> Option<u32> {
        // Read the superblock to get information about the filesystem
        let mut buffer = Buffer::new(1024);
        let super_block = unsafe { &mut *(buffer.get_mut() as *mut SuperBlock) };
        syc_read(dev, buffer.get_mut(), 1024, 1024);

        // Calculate the number of blocks used for inode map
        let imap_blocks = super_block.imap_blocks as usize;

        // Iterate through each inode map block
        for i in 0..imap_blocks {
            let inode_map_offset = (2 + i) * BLOCK_SIZE as usize;
            syc_read(dev, buffer.get_mut(), BLOCK_SIZE, inode_map_offset as u32);

            // Iterate through each byte in the inode map block
            for i in 0..buffer.len() {
                let byte = buffer[i];
                // Check each bit in the byte to find a free inode
                for j in 0..8 {
                    if byte & (1 << j) == 0 {
                        // Calculate the inode number based on the current byte and bit position
                        let inode_num = (i * BLOCK_SIZE as usize + j) as u32;
                        return Some(inode_num);
                    }
                }
            }
        }

        None // No free inode found
    }

    /// The goal of open is to traverse the path given by path. If we cache the inodes
    /// in RAM, it might make this much quicker. For now, this doesn't do anything since
    /// we're just testing read based on if we know the Inode we're looking for.
    pub fn open(bdev: usize, path: &str) -> Result<Inode, FsError> {
        if let Some(cache) = unsafe { MFS_INODE_CACHE[bdev - 1].take() } {
            let ret;
            if let Some(inode) = cache.get(path) {
                ret = Ok(*inode);
            } else {
                ret = Err(FsError::FileNotFound);
            }
            unsafe {
                MFS_INODE_CACHE[bdev - 1].replace(cache);
            }
            ret
        } else {
            Err(FsError::FileNotFound)
        }
    }

    pub fn read(bdev: usize, inode: &Inode, buffer: *mut u8, size: u32, offset: u32) -> u32 {
        // Our strategy here is to use blocks to see when we need to start reading
        // based on the offset. That's offset_block. Then, the actual byte within
        // that block that we need is offset_byte.
        let mut blocks_seen = 0u32;
        let offset_block = offset / BLOCK_SIZE;
        let mut offset_byte = offset % BLOCK_SIZE;
        // First, the _size parameter (now in bytes_left) is the size of the buffer, not
        // necessarily the size of the file. If our buffer is bigger than the file, we're OK.
        // If our buffer is smaller than the file, then we can only read up to the buffer size.
        let mut bytes_left = if size > inode.size { inode.size } else { size };
        let mut bytes_read = 0u32;
        // The block buffer automatically drops when we quit early due to an error or we've read enough. This will be the holding port when we go out and read a block. Recall that even if we want 10 bytes, we have to read the entire block (really only 512 bytes of the block) first. So, we use the block_buffer as the middle man, which is then copied into the buffer.
        let mut block_buffer = Buffer::new(BLOCK_SIZE as usize);
        // Triply indirect zones point to a block of pointers (BLOCK_SIZE / 4). Each one of those pointers points to another block of pointers (BLOCK_SIZE / 4). Each one of those pointers yet again points to another block of pointers (BLOCK_SIZE / 4). This is why we have indirect, iindirect (doubly), and iiindirect (triply).
        let mut indirect_buffer = Buffer::new(BLOCK_SIZE as usize);
        let mut iindirect_buffer = Buffer::new(BLOCK_SIZE as usize);
        let mut iiindirect_buffer = Buffer::new(BLOCK_SIZE as usize);
        // I put the pointers *const u32 here. That means we will allocate the indirect, doubly indirect, and triply indirect even for small files. I initially had these in their respective scopes, but that required us to recreate the indirect buffer for doubly indirect and both the indirect and doubly indirect buffers for the triply indirect. Not sure which is better, but I probably wasted brain cells on this.
        let izones = indirect_buffer.get() as *const u32;
        let iizones = iindirect_buffer.get() as *const u32;
        let iiizones = iiindirect_buffer.get() as *const u32;

        // ////////////////////////////////////////////
        // // DIRECT ZONES
        // ////////////////////////////////////////////
        // In Rust, our for loop automatically "declares" i from 0 to < 7. The syntax
        // 0..7 means 0 through to 7 but not including 7. If we want to include 7, we
        // would use the syntax 0..=7.
        for i in 0..7 {
            // There are 7 direct zones in the Minix 3 file system. So, we can just read them one by one. Any zone that has the value 0 is skipped and we check the next zones. This might happen as we start writing and truncating.
            if inode.zones[i] == 0 {
                continue;
            }
            // We really use this to keep track of when we need to actually start reading
            // But an if statement probably takes more time than just incrementing it.
            if offset_block <= blocks_seen {
                // If we get here, then our offset is within our window that we want to see.
                // We need to go to the direct pointer's index. That'll give us a block INDEX.
                // That makes it easy since all we have to do is multiply the block size
                // by whatever we get. If it's 0, we skip it and move on.
                let zone_offset = inode.zones[i] * BLOCK_SIZE;
                // We read the zone, which is where the data is located. The zone offset is simply the block
                // size times the zone number. This makes it really easy to read!
                syc_read(bdev, block_buffer.get_mut(), BLOCK_SIZE, zone_offset);

                // There's a little bit of math to see how much we need to read. We don't want to read
                // more than the buffer passed in can handle, and we don't want to read if we haven't
                // taken care of the offset. For example, an offset of 10000 with a size of 2 means we
                // can only read bytes 10,000 and 10,001.
                let read_this_many = if BLOCK_SIZE - offset_byte > bytes_left {
                    bytes_left
                } else {
                    BLOCK_SIZE - offset_byte
                };
                // Once again, here we actually copy the bytes into the final destination, the buffer. This memcpy
                // is written in cpu.rs.
                unsafe {
                    memcpy(
                        buffer.add(bytes_read as usize),
                        block_buffer.get().add(offset_byte as usize),
                        read_this_many as usize,
                    );
                }
                // Regardless of whether we have an offset or not, we reset the offset byte back to 0. This
                // probably will get set to 0 many times, but who cares?
                offset_byte = 0;
                // Reset the statistics to see how many bytes we've read versus how many are left.
                bytes_read += read_this_many;
                bytes_left -= read_this_many;
                // If no more bytes are left, then we're done.
                if bytes_left == 0 {
                    return bytes_read;
                }
            }
            // The blocks_seen is for the offset. We need to skip a certain number of blocks FIRST before getting
            // to the offset. The reason we need to read the zones is because we need to skip zones of 0, and they
            // do not contribute as a "seen" block.
            blocks_seen += 1;
        }
        // ////////////////////////////////////////////
        // // SINGLY INDIRECT ZONES
        // ////////////////////////////////////////////
        // Each indirect zone is a list of pointers, each 4 bytes. These then
        // point to zones where the data can be found. Just like with the direct zones,
        // we need to make sure the zone isn't 0. A zone of 0 means skip it.
        if inode.zones[7] != 0 {
            syc_read(
                bdev,
                indirect_buffer.get_mut(),
                BLOCK_SIZE,
                BLOCK_SIZE * inode.zones[7],
            );
            let izones = indirect_buffer.get() as *const u32;
            for i in 0..NUM_IPTRS {
                // Where do I put unsafe? Dereferencing the pointers and memcpy are the unsafe functions.
                unsafe {
                    if izones.add(i).read() != 0 {
                        if offset_block <= blocks_seen {
                            syc_read(
                                bdev,
                                block_buffer.get_mut(),
                                BLOCK_SIZE,
                                BLOCK_SIZE * izones.add(i).read(),
                            );
                            let read_this_many = if BLOCK_SIZE - offset_byte > bytes_left {
                                bytes_left
                            } else {
                                BLOCK_SIZE - offset_byte
                            };
                            memcpy(
                                buffer.add(bytes_read as usize),
                                block_buffer.get().add(offset_byte as usize),
                                read_this_many as usize,
                            );
                            bytes_read += read_this_many;
                            bytes_left -= read_this_many;
                            offset_byte = 0;
                            if bytes_left == 0 {
                                return bytes_read;
                            }
                        }
                        blocks_seen += 1;
                    }
                }
            }
        }
        // ////////////////////////////////////////////
        // // DOUBLY INDIRECT ZONES
        // ////////////////////////////////////////////
        if inode.zones[8] != 0 {
            syc_read(
                bdev,
                indirect_buffer.get_mut(),
                BLOCK_SIZE,
                BLOCK_SIZE * inode.zones[8],
            );
            unsafe {
                for i in 0..NUM_IPTRS {
                    if izones.add(i).read() != 0 {
                        syc_read(
                            bdev,
                            iindirect_buffer.get_mut(),
                            BLOCK_SIZE,
                            BLOCK_SIZE * izones.add(i).read(),
                        );
                        for j in 0..NUM_IPTRS {
                            if iizones.add(j).read() != 0 {
                                // Notice that this inner code is the same for all end-zone pointers. I'm thinking about
                                // moving this out of here into a function of its own, but that might make it harder
                                // to follow.
                                if offset_block <= blocks_seen {
                                    syc_read(
                                        bdev,
                                        block_buffer.get_mut(),
                                        BLOCK_SIZE,
                                        BLOCK_SIZE * iizones.add(j).read(),
                                    );
                                    let read_this_many = if BLOCK_SIZE - offset_byte > bytes_left {
                                        bytes_left
                                    } else {
                                        BLOCK_SIZE - offset_byte
                                    };
                                    memcpy(
                                        buffer.add(bytes_read as usize),
                                        block_buffer.get().add(offset_byte as usize),
                                        read_this_many as usize,
                                    );
                                    bytes_read += read_this_many;
                                    bytes_left -= read_this_many;
                                    offset_byte = 0;
                                    if bytes_left == 0 {
                                        return bytes_read;
                                    }
                                }
                                blocks_seen += 1;
                            }
                        }
                    }
                }
            }
        }
        // ////////////////////////////////////////////
        // // TRIPLY INDIRECT ZONES
        // ////////////////////////////////////////////
        if inode.zones[9] != 0 {
            syc_read(
                bdev,
                indirect_buffer.get_mut(),
                BLOCK_SIZE,
                BLOCK_SIZE * inode.zones[9],
            );
            unsafe {
                for i in 0..NUM_IPTRS {
                    if izones.add(i).read() != 0 {
                        syc_read(
                            bdev,
                            iindirect_buffer.get_mut(),
                            BLOCK_SIZE,
                            BLOCK_SIZE * izones.add(i).read(),
                        );
                        for j in 0..NUM_IPTRS {
                            if iizones.add(j).read() != 0 {
                                syc_read(
                                    bdev,
                                    iiindirect_buffer.get_mut(),
                                    BLOCK_SIZE,
                                    BLOCK_SIZE * iizones.add(j).read(),
                                );
                                for k in 0..NUM_IPTRS {
                                    if iiizones.add(k).read() != 0 {
                                        // Hey look! This again.
                                        if offset_block <= blocks_seen {
                                            syc_read(
                                                bdev,
                                                block_buffer.get_mut(),
                                                BLOCK_SIZE,
                                                BLOCK_SIZE * iiizones.add(k).read(),
                                            );
                                            let read_this_many =
                                                if BLOCK_SIZE - offset_byte > bytes_left {
                                                    bytes_left
                                                } else {
                                                    BLOCK_SIZE - offset_byte
                                                };
                                            memcpy(
                                                buffer.add(bytes_read as usize),
                                                block_buffer.get().add(offset_byte as usize),
                                                read_this_many as usize,
                                            );
                                            bytes_read += read_this_many;
                                            bytes_left -= read_this_many;
                                            offset_byte = 0;
                                            if bytes_left == 0 {
                                                return bytes_read;
                                            }
                                        }
                                        blocks_seen += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        // Anyone else love this stairstep style? I probably should put the pointers in a function by themselves,
        // but I think that'll make it more difficult to see what's actually happening.

        bytes_read
    }

    pub fn write(bdev: usize, inode: &mut Inode, buffer: *mut u8, size: u32, offset: u32) -> u32 {
        let mut blocks_seen = 0u32;
        let offset_block = offset / BLOCK_SIZE;
        let mut offset_byte = offset % BLOCK_SIZE;

        let mut bytes_left = size;
        let mut bytes_write = 0u32;

        let mut indirect_buffer = Buffer::new(BLOCK_SIZE as usize);
        let mut iindirect_buffer = Buffer::new(BLOCK_SIZE as usize);
        let mut iiindirect_buffer = Buffer::new(BLOCK_SIZE as usize);

        let izones = indirect_buffer.get() as *const u32;
        let iizones = iiindirect_buffer.get() as *const u32;
        let iiizones = iiindirect_buffer.get() as *const u32;

        // ////////////////////////////////////////////
        // // DIRECT ZONES
        // ////////////////////////////////////////////
        // In Rust, our for loop automatically "declares" i from 0 to < 7. The syntax
        // 0..7 means 0 through to 7 but not including 7. If we want to include 7, we
        // would use the syntax 0..=7.
        for i in 0..7 {
            if inode.zones[i] == 0 {
                continue;
            }
            if offset_block <= blocks_seen {
                let zone_offset = inode.zones[i] * BLOCK_SIZE;

                syc_write(bdev, buffer, size, zone_offset);

                let write_this_many = if BLOCK_SIZE - offset_byte > bytes_left {
                    bytes_left
                } else {
                    BLOCK_SIZE - offset_byte
                };
                unsafe {
                    let _ = buffer.add(bytes_write as usize);
                };
                offset_byte = 0;
                bytes_write += write_this_many;
                bytes_left -= write_this_many;
                if bytes_left == 0 {
                    return bytes_write;
                }
            }
            blocks_seen += 1;
        }

        // ////////////////////////////////////////////
        // // SINGLY INDIRECT ZONES
        // ////////////////////////////////////////////
        // Each indirect zone is a list of pointers, each 4 bytes. These then
        // point to zones where the data can be found. Just like with the direct zones,
        // we need to make sure the zone isn't 0. A zone of 0 means skip it.
        if inode.zones[7] != 0 {
            syc_read(
                bdev,
                indirect_buffer.get_mut(),
                BLOCK_SIZE,
                BLOCK_SIZE * inode.zones[7],
            );
            let izones = indirect_buffer.get() as *const u32;
            for i in 0..NUM_IPTRS {
                unsafe {
                    if izones.add(i).read() != 0 {
                        if offset_block <= blocks_seen {
                            syc_write(bdev, buffer, size, BLOCK_SIZE * izones.add(i).read());
                            let write_this_many = if BLOCK_SIZE - offset_byte > bytes_left {
                                bytes_left
                            } else {
                                BLOCK_SIZE - offset_byte
                            };
                            let _ = buffer.add(bytes_write as usize);
                            offset_byte = 0;
                            bytes_write += write_this_many;
                            bytes_left -= write_this_many;
                            if bytes_left == 0 {
                                return bytes_write;
                            }
                        }
                        blocks_seen += 1;
                    }
                }
            }
        }
        // ////////////////////////////////////////////
        // // DOUBLY INDIRECT ZONES
        // ////////////////////////////////////////////
        if inode.zones[8] != 0 {
            syc_read(
                bdev,
                indirect_buffer.get_mut(),
                BLOCK_SIZE,
                BLOCK_SIZE * inode.zones[8],
            );
            unsafe {
                for i in 0..NUM_IPTRS {
                    if izones.add(i).read() != 0 {
                        syc_read(
                            bdev,
                            iindirect_buffer.get_mut(),
                            BLOCK_SIZE,
                            BLOCK_SIZE * izones.add(i).read(),
                        );
                        for j in 0..NUM_IPTRS {
                            if iizones.add(j).read() != 0 {
                                if offset_block <= blocks_seen {
                                    syc_write(
                                        bdev,
                                        buffer,
                                        size,
                                        BLOCK_SIZE * iizones.add(j).read(),
                                    );
                                    let write_this_many = if BLOCK_SIZE - offset_byte > bytes_left {
                                        bytes_left
                                    } else {
                                        BLOCK_SIZE - offset_byte
                                    };
                                    let _ = buffer.add(bytes_write as usize);
                                    bytes_write += write_this_many;
                                    bytes_left -= write_this_many;
                                    offset_byte = 0;
                                    if bytes_left == 0 {
                                        return bytes_write;
                                    }
                                }
                                blocks_seen += 1;
                            }
                        }
                    }
                }
            }
        }
        // ////////////////////////////////////////////
        // // TRIPLY INDIRECT ZONES
        // ////////////////////////////////////////////
        if inode.zones[9] != 0 {
            syc_read(
                bdev,
                indirect_buffer.get_mut(),
                BLOCK_SIZE,
                BLOCK_SIZE * inode.zones[9],
            );
            unsafe {
                for i in 0..NUM_IPTRS {
                    if izones.add(i).read() != 0 {
                        syc_read(
                            bdev,
                            iindirect_buffer.get_mut(),
                            BLOCK_SIZE,
                            BLOCK_SIZE * izones.add(i).read(),
                        );
                        for j in 0..NUM_IPTRS {
                            if iizones.add(j).read() != 0 {
                                syc_read(
                                    bdev,
                                    iiindirect_buffer.get_mut(),
                                    BLOCK_SIZE,
                                    BLOCK_SIZE * iizones.add(j).read(),
                                );
                                for k in 0..NUM_IPTRS {
                                    if iiizones.add(k).read() != 0 {
                                        if offset_block <= blocks_seen {
                                            syc_write(
                                                bdev,
                                                buffer,
                                                size,
                                                BLOCK_SIZE * iiizones.add(k).read(),
                                            );
                                            let write_this_many =
                                                if BLOCK_SIZE - offset_byte > bytes_left {
                                                    bytes_left
                                                } else {
                                                    BLOCK_SIZE - offset_byte
                                                };
                                            let _ = buffer.add(bytes_write as usize);
                                            bytes_write += write_this_many;
                                            bytes_left -= write_this_many;
                                            offset_byte = 0;
                                            if bytes_left == 0 {
                                                return bytes_write;
                                            }
                                        }
                                        blocks_seen += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        inode.size = bytes_write;

        bytes_write
    }

    pub fn delete(bdev: usize, path: &str, inode_num: usize) {
        if let Some(mut cache) = unsafe { MFS_INODE_CACHE[bdev - 1].take() } {
            Self::delete_inode_and_direntry(&mut cache, &path.to_string(), inode_num as u32, bdev);
            unsafe {
                MFS_INODE_CACHE[bdev - 1].replace(cache);
            }
        }
        MinixFileSystem::refresh(bdev);
    }

    fn delete_inode_and_direntry(
        btm: &mut BTreeMap<String, Inode>,
        cwd: &String,
        inode_num: u32,
        bdev: usize,
    ) {
        // Step 1: Get the inode
        let mut ino = match Self::get_inode(bdev, 1) {
            Some(inode) => inode,
            None => return,
        };

        // Step 2: Read the directory entries
        let mut buf = Buffer::new(((ino.size + BLOCK_SIZE - 1) & !BLOCK_SIZE) as usize);
        let dirents = buf.get() as *const DirEntry;
        let sz = Self::read(bdev, &ino, buf.get_mut(), BLOCK_SIZE, 0);
        let num_dirents = sz as usize / size_of::<DirEntry>();
        println!("num_dirents: {}", num_dirents);

        // Step 3: Find and remove the DirEntry
        for i in 2..num_dirents {
            unsafe {
                let ref d = *dirents.add(i);
                if d.inode == inode_num {
                    // Mark this directory entry as deleted
                    let dirent_buffer = buf.get_mut() as *mut DirEntry;
                    (*dirent_buffer.add(i)).inode = 0;

                    // Write the updated directory entries back to the disk
                    Self::write(bdev, &mut ino, buf.get_mut(), sz, 0);

                    // Remove the entry from the BTreeMap
                    let mut path_to_remove = String::with_capacity(cwd.len() + 60);
                    path_to_remove.push_str(cwd);
                    if !cwd.ends_with('/') {
                        path_to_remove.push('/');
                    }
                    for j in 0..60 {
                        if d.name[j] == 0 {
                            break;
                        }
                        path_to_remove.push(d.name[j] as char);
                    }
                    btm.remove(&path_to_remove);
                    break;
                }
            }
        }

        // Step 4: Update the imap to mark the inode as free
        let imap_offset = Self::get_imap_offset(inode_num as usize);
        let nth = inode_num % 8;
        let mut imap_buffer = Buffer::new(512);
        syc_read(
            bdev,
            imap_buffer.get_mut(),
            imap_buffer.len() as u32,
            imap_offset as u32,
        );

        // Clear the nth bit in imap
        imap_buffer[0] &= !(1 << nth);

        // Write back the updated imap
        syc_write(
            bdev,
            imap_buffer.get_mut(),
            imap_buffer.len() as u32,
            imap_offset as u32,
        );
    }

    pub fn create(bdev: usize, cwd: &str, filename: &str) {
        if let Some(mut cache) = unsafe { MFS_INODE_CACHE[bdev - 1].take() } {
            Self::create_new_file(&mut cache, &cwd.to_string(), filename, bdev);
            unsafe {
                MFS_INODE_CACHE[bdev - 1].replace(cache);
            }
        }
        MinixFileSystem::refresh(bdev);
    }

    fn create_new_file(
        btm: &mut BTreeMap<String, Inode>,
        cwd: &String,
        filename: &str,
        bdev: usize,
    ) {
        // Step 1: Allocate a new inode
        let mut new_inode = Inode {
            mode: 0o644,
            nlinks: 1,
            uid: 0,
            gid: 0,
            size: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
            zones: [0; 10],
        };

        // Find a free inode
        let free_inode_num = MinixFileSystem::find_free_inode(bdev).unwrap();

        // Step 2: Update the parent directory with the new directory entry
        let parent_inode = match btm.get(cwd) {
            Some(inode) => inode.clone(),
            None => return,
        };

        // Create a new directory entry
        let mut new_direntry = DirEntry {
            inode: free_inode_num,
            name: [0; 60],
        };

        // Copy the filename to the new directory entry's name
        for (i, c) in filename.bytes().enumerate() {
            if i >= 60 {
                break;
            }
            new_direntry.name[i] = c;
        }

        // Step 3: Update the parent directory's content
        let mut buf = Buffer::new(((parent_inode.size + BLOCK_SIZE - 1) & !BLOCK_SIZE) as usize);
        let dirents = buf.get() as *mut DirEntry;
        let sz = MinixFileSystem::read(bdev, &parent_inode, buf.get_mut(), BLOCK_SIZE, 0);

        // Append the new directory entry to the buffer
        let _dirent_offset = sz;
        unsafe {
            let new_direntry_ptr = dirents.add((sz / mem::size_of::<DirEntry>() as u32) as usize);
            core::ptr::copy_nonoverlapping(&new_direntry as *const DirEntry, new_direntry_ptr, 1);
        }

        // Step 4: Update the imap to mark the new inode as allocated
        let imap_offset = MinixFileSystem::get_imap_offset(free_inode_num as usize);
        let nth = free_inode_num % 8;
        let mut imap_buffer = Buffer::new(512);
        syc_read(
            bdev,
            imap_buffer.get_mut(),
            imap_buffer.len() as u32,
            imap_offset as u32,
        );
        // Set the nth bit in imap
        imap_buffer[0] |= 1 << nth;

        // Write back the updated imap
        syc_write(
            bdev,
            imap_buffer.get_mut(),
            imap_buffer.len() as u32,
            imap_offset as u32,
        );

        // Step 5: Write the new inode to the block device
        let new_inode_offset = MinixFileSystem::get_inode_offset(free_inode_num as usize);
        let mut new_inode_buffer = Buffer::new(size_of::<Inode>());
        unsafe {
            let new_inode_ptr = new_inode_buffer.get_mut() as *mut Inode;
            core::ptr::copy_nonoverlapping(&new_inode, new_inode_ptr, 1);
        }
        MinixFileSystem::write(
            bdev,
            &mut new_inode,
            new_inode_buffer.get_mut(),
            size_of::<Inode>() as u32,
            new_inode_offset as u32,
        );

        // Add the new inode to the BTreeMap
        let mut new_file_path = cwd.clone();
        if !cwd.ends_with('/') {
            new_file_path.push('/');
        }
        new_file_path.push_str(filename);
        btm.insert(new_file_path, new_inode);
    }

    pub fn stat(&self, inode: &Inode) -> Stat {
        Stat {
            mode: inode.mode,
            size: inode.size,
            uid: inode.uid,
            gid: inode.gid,
        }
    }

    pub fn get_imap_offset(inode_num: usize) -> usize {
        // then take the inode_num % 8 bit
        2 * BLOCK_SIZE as usize + (inode_num - 1) / 8
    }

    pub fn get_zmap_offset(zone_num: usize) -> usize {
        // inode.zones[i] * BLOCK_SIZE
        // then take the zone_num % 8 bit
        (2 + 2/* imap blocks */) * BLOCK_SIZE as usize + zone_num / 8
    }

    pub fn get_inode_offset(inode_num: usize) -> usize {
        // (2 + 2/* imap blocks */ + 4/* zmap blocks */) as usize * BLOCK_SIZE as usize
        //     + ((inode_num as usize - 1) / (BLOCK_SIZE as usize / size_of::<Inode>()))
        //         * BLOCK_SIZE as usize
        0x2048 + (inode_num - 2) * 0x40
    }

    pub fn get_zone_offset(zone_num: usize) -> usize {
        // zone_num: inode.zones[i]
        zone_num * BLOCK_SIZE as usize
    }
    pub fn show_fs_info(bdev: usize) {
        let mut buffer = Buffer::new(1024);
        let super_block = unsafe { &*(buffer.get_mut() as *mut SuperBlock) };
        // Read superblock
        syc_read(bdev, buffer.get_mut(), 512, 1024);
        if super_block.magic == MAGIC {
            println!("\nFilesystem Superblock Info: ");
            println!("{:#?}", super_block);
        }
    }

    pub fn show_all_file_paths(bdev: usize) {
        println!("\nNow list all existed files: ");
        if let Some(cache) = unsafe { MFS_INODE_CACHE[bdev - 1].take() } {
            for (path, _) in cache.iter() {
                println!("{}", path);
            }
            unsafe {
                MFS_INODE_CACHE[bdev - 1].replace(cache);
            }
        }
    }
}

/// This is a wrapper function around the syscall_block_read. This allows me to do
/// other things before I call the system call (or after).
fn syc_read(bdev: usize, buffer: *mut u8, size: u32, offset: u32) -> u8 {
    const BLOCK_SIZE: u32 = 512;

    // Calculate the block boundaries
    let block_start = offset / BLOCK_SIZE;
    let block_end = (offset + size + BLOCK_SIZE - 1) / BLOCK_SIZE;

    // Calculate the actual size to read, aligned to block boundaries
    let actual_buffer_size = (block_end - block_start) * BLOCK_SIZE;

    // Allocate a temporary buffer to read the aligned data
    let mut temp_buffer = vec![0u8; actual_buffer_size as usize];

    // Read the aligned data into the temporary buffer
    let read_result = syscall_block_read(
        bdev,
        temp_buffer.as_mut_ptr(),
        actual_buffer_size,
        block_start * BLOCK_SIZE,
    );

    if read_result != 0 {
        return read_result;
    }

    // Calculate the offset within the temporary buffer
    let internal_offset = (offset % BLOCK_SIZE) as usize;

    // Copy the relevant portion of the temporary buffer to the output buffer
    unsafe {
        core::ptr::copy_nonoverlapping(
            temp_buffer.as_ptr().add(internal_offset),
            buffer,
            size as usize,
        );
    }

    0 // Indicate success
}

pub fn syc_write(bdev: usize, buffer: *mut u8, size: u32, offset: u32) -> u8 {
    // Calculate the start and end blocks for read-modify-write
    let block_start = offset / BLOCK_SIZE;
    let block_end = (offset + size + BLOCK_SIZE - 1) / BLOCK_SIZE;

    // Calculate the actual size to read/write, aligned to block boundaries
    let actual_buffer_size = (block_end - block_start) * BLOCK_SIZE;

    // Allocate buffer for the entire block range
    let mut actual_buffer = Buffer::new(actual_buffer_size as usize);

    // Read the data covering the range to modify
    syc_read(
        bdev,
        actual_buffer.get_mut(),
        actual_buffer_size as u32,
        block_start * BLOCK_SIZE,
    );

    // Calculate the offset within the buffer where the write should start
    let internal_offset = (offset % BLOCK_SIZE) as usize;

    // Ensure the read data covers the entire range to be written
    assert!(internal_offset + size as usize <= actual_buffer.len());

    // Copy the data to the appropriate location within the buffer
    unsafe {
        memcpy(
            actual_buffer.get_mut().add(internal_offset),
            buffer,
            size as usize,
        );
    }

    // Write the modified buffer back to the device
    syscall_block_write(
        bdev,
        actual_buffer.get_mut(),
        actual_buffer_size as u32,
        block_start * BLOCK_SIZE,
    )
}

// We have to start a process when reading from a file since the block
// device will block. We only want to block in a process context, not an
// interrupt context.
struct ProcArgs {
    pub pid: u16,
    pub dev: usize,
    pub buffer: *mut u8,
    pub size: u32,
    pub offset: u32,
    pub node: u32,
}

// This is the actual code ran inside of the read process.
fn read_proc(args_addr: usize) {
    let args = unsafe { Box::from_raw(args_addr as *mut ProcArgs) };

    // Start the read! Since we're in a kernel process, we can block by putting this
    // process into a waiting state and wait until the block driver returns.
    let inode = MinixFileSystem::get_inode(args.dev, args.node);
    let bytes = MinixFileSystem::read(
        args.dev,
        &inode.unwrap(),
        args.buffer,
        args.size,
        args.offset,
    );

    // Let's write the return result into regs[10], which is A0.
    unsafe {
        let ptr = get_by_pid(args.pid);
        if !ptr.is_null() {
            (*(*ptr).frame).regs[Registers::A0 as usize] = bytes as usize;
        }
    }
    // This is the process making the system call. The system itself spawns another process
    // which goes out to the block device. Since we're passed the read call, we need to awaken
    // the process and get it ready to go. The only thing this process needs to clean up is the
    // tfree(), but the user process doesn't care about that.
    set_running(args.pid);
}

/// System calls will call process_read, which will spawn off a kernel process to read
/// the requested data.
pub fn process_read(pid: u16, dev: usize, node: u32, buffer: *mut u8, size: u32, offset: u32) {
    // println!("FS read {}, {}, 0x{:x}, {}, {}", pid, dev, buffer as usize, size, offset);
    let args = ProcArgs {
        pid,
        dev,
        buffer,
        size,
        offset,
        node,
    };
    let boxed_args = Box::new(args);
    set_waiting(pid);
    let _ = add_kernel_process_args(read_proc, Box::into_raw(boxed_args) as usize);
}

// This is the actual code ran inside of the write process
fn write_proc(args_addr: usize) {
    let args = unsafe { Box::from_raw(args_addr as *mut ProcArgs) };

    let inode = MinixFileSystem::get_inode(args.dev, args.node);
    let bytes = MinixFileSystem::write(
        args.dev,
        &mut inode.unwrap(),
        args.buffer,
        args.size,
        args.offset,
    );

    // write the return result into regs[10], which is A0
    unsafe {
        let ptr = get_by_pid(args.pid);
        if !ptr.is_null() {
            (*(*ptr).frame).regs[Registers::A0 as usize] = bytes as usize;
        }
    }
    set_running(args.pid);
}

/// System calls will call process_write, which will spawn off a kernel process to write
/// the requested data.
pub fn process_write(pid: u16, dev: usize, node: u32, buffer: *mut u8, size: u32, offset: u32) {
    let args = ProcArgs {
        pid,
        dev,
        buffer,
        size,
        offset,
        node,
    };

    let boxed_args = Box::new(args);
    set_waiting(pid);
    let _ = add_kernel_process_args(write_proc, Box::into_raw(boxed_args) as usize);
}

/// Stats on a file. This generally mimics an inode
/// since that's the information we want anyway.
/// However, inodes are filesystem specific, and we
/// want a more generic stat.
#[derive(Debug)]
pub struct Stat {
    pub mode: u16,
    pub size: u32,
    pub uid: u16,
    pub gid: u16,
}

#[derive(Debug)]
pub enum FsError {
    Success,
    FileNotFound,
    Permission,
    IsFile,
    IsDirectory,
    FileExists,
}
