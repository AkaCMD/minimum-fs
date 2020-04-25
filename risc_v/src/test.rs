// test.rs

use crate::syscall::syscall_fs_read;

pub fn test_block() {
    // Let's test the block driver!
    let buffer = crate::kmem::kmalloc(1024);
    println!("Started test block process, buffer is at {:p}.", buffer);
    unsafe {
        syscall_fs_read(8, 12, buffer, 1024, 0);
        for i in 0..32 {
            print!("{:02x}  ", buffer.add(i).read());
            if (i+1) % 16 == 0 {
                println!();
            }
        }
    }
    println!();
    crate::kmem::kfree(buffer);
    println!("Test block finished");
}