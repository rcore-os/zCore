extern crate std;

use crate::*;
use rcore_fs::{
    util::uninit_memory,
    vfs::{FileSystem, FileType, Metadata, Result, Timespec},
};
use std::{
    fs::{self, OpenOptions},
    sync::{Arc, Mutex},
};

fn _open_sample_file() -> Arc<SimpleFileSystem> {
    fs::copy("sfs.img", "test.img").expect("failed to open sfs.img");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open("test.img")
        .expect("failed to open test.img");
    SimpleFileSystem::open(Arc::new(Mutex::new(file))).expect("failed to open SFS")
}

fn _create_new_sfs() -> Arc<SimpleFileSystem> {
    let file = tempfile::tempfile().expect("failed to create file");
    SimpleFileSystem::create(Arc::new(Mutex::new(file)), 32 * 4096 * 4096)
        .expect("failed to create SFS")
}

#[test]
#[ignore]
fn open_sample_file() {
    _open_sample_file();
}

#[test]
fn create_new_sfs() {
    let sfs = _create_new_sfs();
    let _root = sfs.root_inode();
}

#[test]
fn create_file() -> Result<()> {
    let sfs = _create_new_sfs();
    let root = sfs.root_inode();
    let file1 = root.create("file1", FileType::File, 0o777)?;

    assert_eq!(
        file1.metadata()?,
        Metadata {
            inode: 7,
            size: 0,
            type_: FileType::File,
            mode: 0o777,
            blocks: 0,
            atime: Timespec { sec: 0, nsec: 0 },
            mtime: Timespec { sec: 0, nsec: 0 },
            nlinks: 1,
            uid: 0,
            ctime: Timespec { sec: 0, nsec: 0 },
            gid: 0,
            blk_size: 4096,
            dev: 0,
            rdev: 100, // dummo why 100 here, maybe legacy data?
        }
    );

    sfs.sync()?;
    Ok(())
}

#[test]
fn resize() -> Result<()> {
    let sfs = _create_new_sfs();
    let root = sfs.root_inode();
    let file1 = root.create("file1", FileType::File, 0o777)?;
    assert_eq!(file1.metadata()?.size, 0, "empty file size != 0");

    const SIZE1: usize = 0x1234;
    const SIZE2: usize = 0x1250;
    file1.resize(SIZE1)?;
    assert_eq!(file1.metadata()?.size, SIZE1, "wrong size after resize");
    let mut data1: [u8; SIZE2] = unsafe { uninit_memory() };
    let len = file1.read_at(0, data1.as_mut())?;
    assert_eq!(len, SIZE1, "wrong size returned by read_at()");
    assert_eq!(
        &data1[..SIZE1],
        &[0u8; SIZE1][..],
        "expanded data should be 0"
    );

    sfs.sync()?;
    Ok(())
}

#[test]
fn resize_on_dir_should_panic() -> Result<()> {
    let sfs = _create_new_sfs();
    let root = sfs.root_inode();
    assert!(root.resize(4096).is_err());
    sfs.sync()?;

    Ok(())
}

#[test]
fn resize_too_large_should_panic() -> Result<()> {
    let sfs = _create_new_sfs();
    let root = sfs.root_inode();
    let file1 = root.create("file1", FileType::File, 0o777)?;
    assert!(file1.resize(1 << 40).is_err());
    sfs.sync()?;

    Ok(())
}

#[test]
fn create_then_lookup() -> Result<()> {
    let sfs = _create_new_sfs();
    let root = sfs.root_inode();

    assert!(Arc::ptr_eq(&root.lookup(".")?, &root), "failed to find .");
    assert!(Arc::ptr_eq(&root.lookup("..")?, &root), "failed to find ..");

    let file1 = root
        .create("file1", FileType::File, 0o777)
        .expect("failed to create file1");
    assert!(
        Arc::ptr_eq(&root.lookup("file1")?, &file1),
        "failed to find file1"
    );
    assert!(root.lookup("file2").is_err(), "found non-existent file");

    let dir1 = root
        .create("dir1", FileType::Dir, 0o777)
        .expect("failed to create dir1");
    let file2 = dir1
        .create("file2", FileType::File, 0o777)
        .expect("failed to create /dir1/file2");
    assert!(
        Arc::ptr_eq(&root.lookup("dir1/file2")?, &file2),
        "failed to find dir1/file2"
    );
    assert!(
        Arc::ptr_eq(&root.lookup("/")?.lookup("dir1/file2")?, &file2),
        "failed to find dir1/file2"
    );
    assert!(
        Arc::ptr_eq(&dir1.lookup("..")?, &root),
        "failed to find .. from dir1"
    );

    assert!(
        Arc::ptr_eq(&dir1.lookup("../dir1/file2")?, &file2),
        "failed to find dir1/file2 by relative"
    );
    assert!(
        Arc::ptr_eq(&dir1.lookup("/dir1/file2")?, &file2),
        "failed to find dir1/file2 by absolute"
    );
    assert!(
        Arc::ptr_eq(&dir1.lookup("/dir1/../dir1/file2")?, &file2),
        "failed to find dir1/file2 by absolute"
    );
    assert!(
        Arc::ptr_eq(&dir1.lookup("../../..//dir1/../dir1/file2")?, &file2),
        "failed to find dir1/file2 by more than one .."
    );
    assert!(
        Arc::ptr_eq(&dir1.lookup("..//dir1/file2")?, &file2),
        "failed to find dir1/file2 by weird relative"
    );

    assert!(
        root.lookup("./dir1/../file2").is_err(),
        "found non-existent file"
    );
    assert!(
        root.lookup("./dir1/../file3").is_err(),
        "found non-existent file"
    );
    assert!(
        root.lookup("/dir1/../dir1/../file3").is_err(),
        "found non-existent file"
    );
    assert!(
        root.lookup("/dir1/../../../dir1/../file3").is_err(),
        "found non-existent file"
    );
    assert!(
        root.lookup("/").unwrap().lookup("dir1/../file2").is_err(),
        "found non-existent file"
    );

    sfs.sync()?;
    Ok(())
}

#[test]
fn test_symlinks() -> Result<()> {
    let sfs = _create_new_sfs();
    let root = sfs.root_inode();

    let file1 = root
        .create("file1", FileType::File, 0o777)
        .expect("failed to create file1");
    assert!(
        Arc::ptr_eq(&root.lookup("file1")?, &file1),
        "failed to find file1"
    );

    let link1 = root
        .create("link1", FileType::SymLink, 0o777)
        .expect("failed to create link1");
    let data = "file1".as_bytes();
    link1.resize(data.len())?;
    link1.write_at(0, data)?;

    let link2 = root
        .create("link2", FileType::SymLink, 0o777)
        .expect("failed to create link2");
    let data = "link1".as_bytes();
    link2.resize(data.len())?;
    link2.write_at(0, data)?;

    assert!(
        Arc::ptr_eq(&root.lookup("link1")?, &link1),
        "failed to find link1 by relative"
    );
    assert!(
        Arc::ptr_eq(&root.lookup_follow("link1", 1)?, &file1),
        "failed to find file1 by link1"
    );
    assert!(
        Arc::ptr_eq(&root.lookup_follow("link2", 0)?, &link2),
        "failed to find link2 by link2"
    );
    assert!(
        Arc::ptr_eq(&root.lookup_follow("link2", 1)?, &link1),
        "failed to find link1 by link2"
    );
    assert!(
        Arc::ptr_eq(&root.lookup_follow("link2", 2)?, &file1),
        "failed to find file1 by link2"
    );

    let link3 = root
        .create("link3", FileType::SymLink, 0o777)
        .expect("failed to create link3");
    let data = "/link2".as_bytes();
    link3.resize(data.len())?;
    link3.write_at(0, data)?;

    assert!(
        Arc::ptr_eq(&root.lookup_follow("link3", 0)?, &link3),
        "failed to find link3 by link3"
    );
    assert!(
        Arc::ptr_eq(&root.lookup_follow("link3", 1)?, &link2),
        "failed to find link2 by link3"
    );
    assert!(
        Arc::ptr_eq(&root.lookup_follow("link3", 2)?, &link1),
        "failed to find link1 by link3"
    );
    assert!(
        Arc::ptr_eq(&root.lookup_follow("link3", 3)?, &file1),
        "failed to find file1 by link2"
    );

    let dir1 = root
        .create("dir1", FileType::Dir, 0o777)
        .expect("failed to create dir1");
    let file2 = dir1
        .create("file2", FileType::File, 0o777)
        .expect("failed to create /dir1/file2");

    let link_dir = root
        .create("link_dir", FileType::SymLink, 0o777)
        .expect("failed to create link2");
    let data = "dir1".as_bytes();
    link_dir.resize(data.len())?;
    link_dir.write_at(0, data)?;

    assert!(
        Arc::ptr_eq(&root.lookup_follow("link_dir/file2", 1)?, &file2),
        "failed to find file2"
    );

    sfs.sync()?;
    Ok(())
}

#[test]
fn test_double_indirect_blocks() -> Result<()> {
    let sfs = _create_new_sfs();
    let root = sfs.root_inode();

    let file1 = root
        .create("file1", FileType::File, 0o777)
        .expect("failed to create file1");
    assert!(
        Arc::ptr_eq(&root.lookup("file1")?, &file1),
        "failed to find file1"
    );

    // resize to direct maximum size
    file1.resize(MAX_NBLOCK_DIRECT * BLKSIZE).unwrap();
    // force usage of indirect block
    file1.resize((MAX_NBLOCK_DIRECT + 1) * BLKSIZE).unwrap();
    file1.resize(MAX_NBLOCK_INDIRECT * BLKSIZE).unwrap();
    // force usage of double indirect block
    file1.resize((MAX_NBLOCK_INDIRECT + 1) * BLKSIZE).unwrap();
    file1.resize((MAX_NBLOCK_INDIRECT + 2) * BLKSIZE).unwrap();

    // resize up and down
    file1.resize(0).unwrap();
    file1.resize((MAX_NBLOCK_INDIRECT + 2) * BLKSIZE).unwrap();
    file1.resize(MAX_NBLOCK_DIRECT * BLKSIZE).unwrap();
    file1.resize((MAX_NBLOCK_DIRECT + 1) * BLKSIZE).unwrap();
    file1.resize(MAX_NBLOCK_DIRECT * BLKSIZE).unwrap();
    file1.resize(0).unwrap();
    file1.resize((MAX_NBLOCK_INDIRECT + 1) * BLKSIZE).unwrap();
    file1.resize(MAX_NBLOCK_DIRECT * BLKSIZE).unwrap();
    file1.resize((MAX_NBLOCK_INDIRECT + 2) * BLKSIZE).unwrap();
    file1.resize((MAX_NBLOCK_INDIRECT + 1) * BLKSIZE).unwrap();
    file1.resize(0).unwrap();

    sfs.sync()?;
    Ok(())
}

#[test]
fn arc_layout() {
    // [usize, usize, T]
    //  ^ start       ^ Arc::into_raw
    let p = Arc::new([2u8; 5]);
    let ptr = Arc::into_raw(p);
    let start = unsafe { (ptr as *const usize).offset(-2) };
    let ns = unsafe { &*(start as *const [usize; 2]) };
    assert_eq!(ns, &[1usize, 1]);
}

#[test]
#[ignore]
fn kernel_image_file_create() -> Result<()> {
    let sfs = _open_sample_file();
    let root = sfs.root_inode();
    let files_count_before = root.list()?.len();
    root.create("hello2", FileType::File, 0o777)?;
    let files_count_after = root.list()?.len();
    assert_eq!(files_count_before + 1, files_count_after);
    assert!(root.lookup("hello2").is_ok());

    sfs.sync()?;
    Ok(())
}

#[test]
#[ignore]
fn kernel_image_file_unlink() -> Result<()> {
    let sfs = _open_sample_file();
    let root = sfs.root_inode();
    let files_count_before = root.list()?.len();
    root.unlink("hello")?;
    let files_count_after = root.list()?.len();
    assert_eq!(files_count_before, files_count_after + 1);
    assert!(root.lookup("hello").is_err());

    sfs.sync()?;
    Ok(())
}

#[test]
#[ignore]
fn kernel_image_file_rename() -> Result<()> {
    let sfs = _open_sample_file();
    let root = sfs.root_inode();
    let files_count_before = root.list()?.len();
    root.move_("hello", &root, "hello2")?;
    let files_count_after = root.list()?.len();
    assert_eq!(files_count_before, files_count_after);
    assert!(root.lookup("hello").is_err());
    assert!(root.lookup("hello2").is_ok());

    sfs.sync()?;
    Ok(())
}

#[test]
#[ignore]
fn kernel_image_file_move() -> Result<()> {
    let sfs = _open_sample_file();
    let root = sfs.root_inode();
    let files_count_before = root.list()?.len();
    root.unlink("divzero")?;
    let rust_dir = root.create("rust", FileType::Dir, 0o777)?;
    root.move_("hello", &rust_dir, "hello_world")?;
    let files_count_after = root.list()?.len();
    assert_eq!(files_count_before, files_count_after + 1);
    assert!(root.lookup("hello").is_err());
    assert!(root.lookup("divzero").is_err());
    assert!(root.lookup("rust").is_ok());
    assert!(rust_dir.lookup("hello_world").is_ok());

    sfs.sync()?;
    Ok(())
}

#[test]
fn hard_link() -> Result<()> {
    let sfs = _create_new_sfs();
    let root = sfs.root_inode();
    let file1 = root.create("file1", FileType::File, 0o777)?;
    root.link("file2", &file1)?;
    let file2 = root.lookup("file2")?;
    file1.resize(100)?;
    assert_eq!(file2.metadata()?.size, 100);

    sfs.sync()?;
    Ok(())
}

#[test]
fn nlinks() -> Result<()> {
    let sfs = _create_new_sfs();
    let root = sfs.root_inode();
    // -root
    assert_eq!(root.metadata()?.nlinks, 2);

    let file1 = root.create("file1", FileType::File, 0o777)?;
    // -root
    //   `-file1 <f1>
    assert_eq!(file1.metadata()?.nlinks, 1);
    assert_eq!(root.metadata()?.nlinks, 2);

    let dir1 = root.create("dir1", FileType::Dir, 0o777)?;
    // -root
    //   +-dir1
    //   `-file1 <f1>
    assert_eq!(dir1.metadata()?.nlinks, 2);
    assert_eq!(root.metadata()?.nlinks, 3);

    root.move_("dir1", &root, "dir_1")?;
    // -root
    //   +-dir_1
    //   `-file1 <f1>
    assert_eq!(dir1.metadata()?.nlinks, 2);
    assert_eq!(root.metadata()?.nlinks, 3);

    dir1.link("file1_", &file1)?;
    // -root
    //   +-dir_1
    //   |  `-file1_ <f1>
    //   `-file1 <f1>
    assert_eq!(dir1.metadata()?.nlinks, 2);
    assert_eq!(root.metadata()?.nlinks, 3);
    assert_eq!(file1.metadata()?.nlinks, 2);

    let dir2 = root.create("dir2", FileType::Dir, 0o777)?;
    // -root
    //   +-dir_1
    //   |  `-file1_ <f1>
    //   +-dir2
    //   `-file1 <f1>
    assert_eq!(dir1.metadata()?.nlinks, 2);
    assert_eq!(dir2.metadata()?.nlinks, 2);
    assert_eq!(root.metadata()?.nlinks, 4);
    assert_eq!(file1.metadata()?.nlinks, 2);

    root.move_("file1", &root, "file_1")?;
    // -root
    //   +-dir_1
    //   |  `-file1_ <f1>
    //   +-dir2
    //   `-file_1 <f1>
    assert_eq!(dir1.metadata()?.nlinks, 2);
    assert_eq!(dir2.metadata()?.nlinks, 2);
    assert_eq!(root.metadata()?.nlinks, 4);
    assert_eq!(file1.metadata()?.nlinks, 2);

    root.move_("file_1", &dir2, "file__1")?;
    // -root
    //   +-dir_1
    //   |  `-file1_ <f1>
    //   `-dir2
    //      `-file__1 <f1>
    assert_eq!(dir1.metadata()?.nlinks, 2);
    assert_eq!(dir2.metadata()?.nlinks, 2);
    assert_eq!(root.metadata()?.nlinks, 4);
    assert_eq!(file1.metadata()?.nlinks, 2);

    root.move_("dir_1", &dir2, "dir__1")?;
    // -root
    //   `-dir2
    //      +-dir__1
    //      |  `-file1_ <f1>
    //      `-file__1 <f1>
    assert_eq!(dir1.metadata()?.nlinks, 2);
    assert_eq!(dir2.metadata()?.nlinks, 3);
    assert_eq!(root.metadata()?.nlinks, 3);
    assert_eq!(file1.metadata()?.nlinks, 2);

    dir2.unlink("file__1")?;
    // -root
    //   `-dir2
    //      `-dir__1
    //         `-file1_ <f1>
    assert_eq!(file1.metadata()?.nlinks, 1);
    assert_eq!(dir1.metadata()?.nlinks, 2);
    assert_eq!(dir2.metadata()?.nlinks, 3);
    assert_eq!(root.metadata()?.nlinks, 3);

    dir1.unlink("file1_")?;
    // -root
    //   `-dir2
    //      `-dir__1
    assert_eq!(file1.metadata()?.nlinks, 0);
    assert_eq!(dir1.metadata()?.nlinks, 2);
    assert_eq!(dir2.metadata()?.nlinks, 3);
    assert_eq!(root.metadata()?.nlinks, 3);

    dir2.unlink("dir__1")?;
    // -root
    //   `-dir2
    assert_eq!(file1.metadata()?.nlinks, 0);
    assert_eq!(dir1.metadata()?.nlinks, 0);
    assert_eq!(root.metadata()?.nlinks, 3);
    assert_eq!(dir2.metadata()?.nlinks, 2);

    root.unlink("dir2")?;
    // -root
    assert_eq!(file1.metadata()?.nlinks, 0);
    assert_eq!(dir1.metadata()?.nlinks, 0);
    assert_eq!(root.metadata()?.nlinks, 2);
    assert_eq!(dir2.metadata()?.nlinks, 0);

    sfs.sync()?;
    Ok(())
}

#[test]
fn create_then_get_entry() -> Result<()> {
    let sfs = _create_new_sfs();
    let root = sfs.root_inode();

    assert!(root.get_entry(0).unwrap() == *".", "entry 0 is .");
    assert!(
        root.get_entry_with_metadata(0).unwrap().1 == *".",
        "entry 0 is ."
    );
    assert!(root.get_entry(1).unwrap() == *"..", "entry 1 is ..");
    assert!(
        root.get_entry_with_metadata(1).unwrap().1 == *"..",
        "entry 1 is .."
    );

    let _file1 = root.create("file1", FileType::File, 0o777)?;
    assert!(
        root.get_entry_with_metadata(2).unwrap().1 == *"file1",
        "entry 2 is file1"
    );

    sfs.sync()?;
    Ok(())
}
