// test.rs
use crate::kmem::{kfree, kmalloc};
use crate::syscall;
use crate::{block, kmem};
/// Test block will load raw binaries into memory to execute them. This function
/// will load ELF files and try to execute them.
pub fn test() {
    // The majority of the testing code needs to move into a system call (execv maybe?)
    // MinixFileSystem::init(8);
    // let path = "/bin/sh\0".as_bytes().as_ptr();
    // syscall::syscall_execv(path,0);
    // println!("I should never get here, execv should destroy our process.");
    test_block_driver();
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
