use alloc::string::String;

use crate::block;
// test.rs
use crate::fs::{Inode, MinixFileSystem};
use crate::kmem::{self, kfree, kmalloc};
use crate::syscall::*;
/// Test block will load raw binaries into memory to execute them. This function
/// will load ELF files and try to execute them.
pub fn test() {
    // The majority of the testing code needs to move into a system call (execv maybe?)
    MinixFileSystem::init(8);
    greetings();
    MinixFileSystem::show_fs_info(8);
    test_block_driver();
    test_read_file();
    test_open_file("/hello.txt");
    //test_find_free_inode();
    test_write_block();

    test_open_file("/file.txt");
    test_write_file();
    test_open_file("/file.txt");

    test_delete_file();
    MinixFileSystem::show_fs_info(8);
    // 	let path = "/shell\0".as_bytes().as_ptr();
    // 	syscall::syscall_execv(path,0);
    // 	println!("I should never get here, execv should destroy our process.");
}

fn greetings() {
    println!(
        "
__________________________________________________
|                                                | 
|         Welcome to simple file system!         |
|                                                |
--------------------------------------------------"
    );
}

// sudo losetup /dev/loop24 hdd.dsk
// sudo mount /dev/loop24 /mnt
// ls /mnt
fn test_read_file() {
    println!();
    print_divider("Reading from file");
    println!("inode #4: ");
    let buffer = kmalloc(100);
    // device, inode, buffer, size, offset
    let bytes_read = syscall_fs_read(8, 4, buffer, 100, 0);
    if bytes_read != 53 {
        println!(
            "Read {} bytes, but I thought the file was 53 bytes.",
            bytes_read
        );
    } else {
        for i in 0..53 {
            print!("{}", unsafe { buffer.add(i).read() as char });
        }
        println!();
    }
    kfree(buffer);
    //syscall_exit();
}

fn test_find_free_inode() {
    println!();
    print_divider("Finding next free inode");
    let num = MinixFileSystem::find_free_inode(8).unwrap();
    println!("{}", num);
}

fn test_block_driver() {
    println!();
    print_divider("Testing block driver");
    let buffer = kmem::kmalloc(512);
    let _ = block::read(8, buffer, 512, 0x400);
    for i in 0..48 {
        print!(" {:02x}", unsafe { buffer.add(i).read() });
        if 0 == ((i + 1) % 24) {
            println!();
        }
    }
    kmem::kfree(buffer);
    println!("Block driver done");
}

// Open(read) file by its name
fn test_open_file(path: &str) {
    println!();
    print_divider("Open file");
    println!("{} opened", path);
    let buffer = kmalloc(100);
    let inode = &MinixFileSystem::open(8, path).unwrap();
    let size = inode.size;
    MinixFileSystem::read(8, inode, buffer, 100, 0);
    println!();
    println!("{}", path);
    for i in 0..size as usize {
        print!("{}", unsafe { buffer.add(i).read() as char });
    }
    println!();
    kfree(buffer);
    //syscall_exit();
}

// Writing to block and read back
fn test_write_block() {
    println!();
    print_divider("Write to block");
    let test_string = String::from("Hello, block!");
    let mut bytes = test_string.into_bytes();
    let len = bytes.len() as u32;
    let buffer = bytes.as_mut_ptr();
    // The minimum size of writing is 512 bytes
    match block::write(8, buffer, 512, 0xadc00) {
        Ok(result) => {
            println!("Write successful! Result: {}", result);
        }
        Err(error) => {
            println!("Error occurred: {:?}", error);
        }
    }
    kmem::kfree(buffer);
    println!("wirte size: {} bytes", len);
    println!("now read: ");
    let read_buffer = kmalloc(512);
    let _ = block::read(8, read_buffer, 512, 0xadc00);
    for i in 0..len {
        print!("{}", unsafe { read_buffer.add(i as usize).read() as char });
        if 0 == ((i + 1) % 24) {
            println!();
        }
    }
    kfree(read_buffer);
    println!("\nWirte to block driver done!");
}

fn test_write_file() {
    println!();
    print_divider("Writing to file");
    println!("file.txt: ");
    let file_path = "/file.txt";
    let inode = &mut MinixFileSystem::open(8, file_path).unwrap();
    let test_string = String::from("something");
    let mut bytes = test_string.into_bytes();
    let len = bytes.len();
    let buffer = bytes.as_mut_ptr();

    let bytes_write = &MinixFileSystem::write(8, inode, buffer, len as u32, 0);
    println!("write bytes: {}", bytes_write);
    kfree(buffer);
}

fn show_inode_stat(inode: &Inode) {
    println!("{:?}", MinixFileSystem.stat(inode));
}

fn test_delete_file() {
    println!();
    print_divider("Delete file");
    println!("/file.txt deleted");
    MinixFileSystem::delete(8, "/file.txt");
}

fn print_divider(string: &str) {
    let total_length = 40; // Total length of the divider
    let string_length = string.len(); // Length of the input string
    let padding_length = (total_length - string_length - 6) / 2; // Calculate the number of spaces needed on each side

    // Build the left padding spaces
    let left_padding = " ".repeat(padding_length);
    // Build the right padding spaces
    let right_padding = " ".repeat(padding_length + if string_length % 2 == 0 { 0 } else { 1 });

    // Print the divider with the appropriate spacing
    println!(
        "-----------------------<{} {} {}>-----------------------",
        left_padding, string, right_padding
    );
}
