use crate::block;
// test.rs
use crate::fs::MinixFileSystem;
use crate::kmem::{self, kfree, kmalloc};
use crate::syscall::{syscall_exit, syscall_fs_read};
/// Test block will load raw binaries into memory to execute them. This function
/// will load ELF files and try to execute them.
pub fn test() {
    // The majority of the testing code needs to move into a system call (execv maybe?)
    MinixFileSystem::init(8);
    test_block_driver();
    test_read_proc();
    // 	let path = "/shell\0".as_bytes().as_ptr();
    // 	syscall::syscall_execv(path,0);
    // 	println!("I should never get here, execv should destroy our process.");
}

fn test_read_proc() {
    let buffer = kmalloc(100);
    // device, inode, buffer, size, offset
    let bytes_read = syscall_fs_read(8, 2, buffer, 100, 0);
    if bytes_read != 9 {
        println!(
            "Read {} bytes, but I thought the file was 53 bytes.",
            bytes_read
        );
    } else {
        for i in 0..9 {
            print!("{}", unsafe { buffer.add(i).read() as char });
        }
        println!();
    }
    kfree(buffer);
    syscall_exit();
}

fn test_block_driver() {
    // Let's test the block driver!
    println!("Testing block driver.");
    let buffer = kmem::kmalloc(512);
    block::read(8, buffer, 512, 0x400);
    for i in 0..48 {
        print!(" {:02x}", unsafe { buffer.add(i).read() });
        if 0 == ((i + 1) % 24) {
            println!();
        }
    }
    kmem::kfree(buffer);
    println!("Block driver done");
}
