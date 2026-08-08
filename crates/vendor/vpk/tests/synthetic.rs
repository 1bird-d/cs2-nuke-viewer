//! Round-trip tests against hand-built VPK archives.
//!
//! The archive bytes are constructed here rather than checked in as binary
//! fixtures, so the expected layout is readable next to the assertions.

use std::fs;
use std::path::{Path, PathBuf};

use vpk::{Vpk, VpkError, ARCHIVE_INDEX_SELF, VPK_SIGNATURE};

/// A file to place into a synthetic archive.
struct Item {
    extension: &'static str,
    directory: &'static str,
    file_name: &'static str,
    preload: Vec<u8>,
    data: Vec<u8>,
    archive_index: u16,
}

/// Extension -> directories -> indices into the item list, in insertion order.
type TreeLayout<'a> = Vec<(&'a str, Vec<(&'a str, Vec<usize>)>)>;

/// Build a VPK. Returns `(dir_file_bytes, chunk_files)`.
fn build_vpk(version: u32, items: &[Item]) -> (Vec<u8>, Vec<(u16, Vec<u8>)>) {
    // Lay out data per archive index first so we know offsets.
    let mut chunk_data: std::collections::BTreeMap<u16, Vec<u8>> =
        std::collections::BTreeMap::new();
    let mut offsets = Vec::new();
    for item in items {
        let buf = chunk_data.entry(item.archive_index).or_default();
        let offset = buf.len() as u32;
        buf.extend_from_slice(&item.data);
        offsets.push(offset);
    }

    // Group items by extension then directory, preserving first-seen order.
    let mut tree: TreeLayout<'_> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let ext_slot = match tree.iter_mut().find(|(e, _)| *e == item.extension) {
            Some(slot) => slot,
            None => {
                tree.push((item.extension, Vec::new()));
                tree.last_mut().expect("just pushed")
            }
        };
        let dir_slot = match ext_slot.1.iter_mut().find(|(d, _)| *d == item.directory) {
            Some(slot) => slot,
            None => {
                ext_slot.1.push((item.directory, Vec::new()));
                ext_slot.1.last_mut().expect("just pushed")
            }
        };
        dir_slot.1.push(i);
    }

    let mut t = Vec::new();
    for (ext, dirs) in &tree {
        t.extend_from_slice(ext.as_bytes());
        t.push(0);
        for (dir, indices) in dirs {
            t.extend_from_slice(dir.as_bytes());
            t.push(0);
            for &i in indices {
                let item = &items[i];
                let full: Vec<u8> = item
                    .preload
                    .iter()
                    .chain(item.data.iter())
                    .copied()
                    .collect();
                t.extend_from_slice(item.file_name.as_bytes());
                t.push(0);
                t.extend_from_slice(&crc32fast::hash(&full).to_le_bytes());
                t.extend_from_slice(&(item.preload.len() as u16).to_le_bytes());
                t.extend_from_slice(&item.archive_index.to_le_bytes());
                t.extend_from_slice(&offsets[i].to_le_bytes());
                t.extend_from_slice(&(item.data.len() as u32).to_le_bytes());
                t.extend_from_slice(&0xFFFFu16.to_le_bytes());
                t.extend_from_slice(&item.preload);
            }
            t.push(0); // end of file names
        }
        t.push(0); // end of directories
    }
    t.push(0); // end of extensions

    let embedded = chunk_data.remove(&ARCHIVE_INDEX_SELF).unwrap_or_default();

    let mut out = Vec::new();
    out.extend_from_slice(&VPK_SIGNATURE.to_le_bytes());
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&(t.len() as u32).to_le_bytes());
    if version == 2 {
        // CS2 map archives report 0 here even though data follows the tree,
        // matching what Valve's own writer emits.
        out.extend_from_slice(&0u32.to_le_bytes()); // file data section size
        out.extend_from_slice(&0u32.to_le_bytes()); // archive md5 section size
        out.extend_from_slice(&0u32.to_le_bytes()); // other md5 section size
        out.extend_from_slice(&0u32.to_le_bytes()); // signature section size
    }
    out.extend_from_slice(&t);
    out.extend_from_slice(&embedded);

    (out, chunk_data.into_iter().collect())
}

/// Create a uniquely named scratch directory inside the system temp dir.
fn scratch(name: &str) -> PathBuf {
    let unique = format!(
        "mapview-vpk-test-{}-{}-{:?}",
        name,
        std::process::id(),
        std::thread::current().id()
    );
    let dir = std::env::temp_dir().join(unique.replace(['(', ')'], ""));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("write fixture");
}

#[test]
fn reads_single_file_v2_archive() {
    let items = vec![
        Item {
            extension: "vmdl_c",
            directory: "models/props",
            file_name: "crate",
            preload: Vec::new(),
            data: b"MODEL-DATA-0123456789".to_vec(),
            archive_index: ARCHIVE_INDEX_SELF,
        },
        Item {
            extension: "txt",
            directory: " ",
            file_name: "readme",
            preload: Vec::new(),
            data: b"hello world".to_vec(),
            archive_index: ARCHIVE_INDEX_SELF,
        },
    ];
    let (bytes, chunks) = build_vpk(2, &items);
    assert!(chunks.is_empty(), "everything should be embedded");

    let dir = scratch("single");
    let path = dir.join("de_test.vpk");
    write(&path, &bytes);

    let archive = Vpk::open(&path).expect("open archive");
    assert_eq!(archive.header().version, 2);
    assert_eq!(archive.header().header_size, 28);
    assert_eq!(archive.entries().len(), 2);
    assert!(!archive.is_multi_chunk());

    let paths: Vec<&str> = archive.entries().iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, vec!["models/props/crate.vmdl_c", "readme.txt"]);

    // Root-level file: the " " directory sentinel becomes an empty directory.
    let readme = archive.find("readme.txt").expect("find readme");
    assert_eq!(readme.directory, "");
    assert_eq!(readme.extension, "txt");

    assert_eq!(
        archive.read_path("models/props/crate.vmdl_c").unwrap(),
        b"MODEL-DATA-0123456789"
    );
    assert_eq!(archive.read_path("readme.txt").unwrap(), b"hello world");

    // Case-insensitive lookup and backslashes both work.
    assert!(archive.find("MODELS\\PROPS\\CRATE.VMDL_C").is_some());

    // CRC verification passes for well-formed data.
    for entry in archive.entries() {
        archive.read_verified(entry).expect("crc ok");
    }

    assert_eq!(archive.total_entry_bytes(), 21 + 11);
    assert!(archive.entries()[0].is_compiled_resource());
    assert!(!archive.entries()[1].is_compiled_resource());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn reads_multi_chunk_archive() {
    let items = vec![
        Item {
            extension: "vtex_c",
            directory: "materials",
            file_name: "wall",
            preload: b"PRE".to_vec(),
            data: b"TEXTURE-BYTES".to_vec(),
            archive_index: 0,
        },
        Item {
            extension: "vtex_c",
            directory: "materials",
            file_name: "floor",
            preload: Vec::new(),
            data: b"MORE-TEXTURE-BYTES".to_vec(),
            archive_index: 1,
        },
    ];
    let (bytes, chunks) = build_vpk(2, &items);
    assert_eq!(chunks.len(), 2);

    let dir = scratch("multi");
    write(&dir.join("pak01_dir.vpk"), &bytes);
    for (index, data) in &chunks {
        write(&dir.join(format!("pak01_{index:03}.vpk")), data);
    }

    let archive = Vpk::open(dir.join("pak01_dir.vpk")).expect("open archive");
    assert!(archive.is_multi_chunk());
    assert_eq!(archive.entries().len(), 2);

    // Preload bytes are prepended to the archive data.
    assert_eq!(
        archive.read_path("materials/wall.vtex_c").unwrap(),
        b"PRETEXTURE-BYTES"
    );
    assert_eq!(
        archive.read_path("materials/floor.vtex_c").unwrap(),
        b"MORE-TEXTURE-BYTES"
    );

    for entry in archive.entries() {
        archive.read_verified(entry).expect("crc ok");
    }

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn reads_v1_archive() {
    let items = vec![Item {
        extension: "txt",
        directory: "docs",
        file_name: "a",
        preload: Vec::new(),
        data: b"v1 data".to_vec(),
        archive_index: ARCHIVE_INDEX_SELF,
    }];
    let (bytes, _) = build_vpk(1, &items);

    let dir = scratch("v1");
    let path = dir.join("v1.vpk");
    write(&path, &bytes);

    let archive = Vpk::open(&path).expect("open v1");
    assert_eq!(archive.header().version, 1);
    assert_eq!(archive.header().header_size, 12);
    assert_eq!(archive.read_path("docs/a.txt").unwrap(), b"v1 data");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn detects_crc_mismatch() {
    let items = vec![Item {
        extension: "txt",
        directory: "docs",
        file_name: "a",
        preload: Vec::new(),
        data: b"correct".to_vec(),
        archive_index: ARCHIVE_INDEX_SELF,
    }];
    let (mut bytes, _) = build_vpk(2, &items);
    // Corrupt the last data byte without touching the recorded CRC.
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;

    let dir = scratch("crc");
    let path = dir.join("bad.vpk");
    write(&path, &bytes);

    let archive = Vpk::open(&path).expect("open");
    let entry = &archive.entries()[0];
    // Unverified read succeeds...
    assert!(archive.read(entry).is_ok());
    // ...verified read reports the mismatch.
    assert!(matches!(
        archive.read_verified(entry),
        Err(VpkError::CrcMismatch { .. })
    ));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn rejects_bad_signature() {
    let dir = scratch("sig");
    let path = dir.join("notavpk.vpk");
    write(&path, b"this is definitely not a vpk file at all");

    assert!(matches!(
        Vpk::open(&path),
        Err(VpkError::BadSignature { .. })
    ));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn rejects_respawn_version() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&VPK_SIGNATURE.to_le_bytes());
    bytes.extend_from_slice(&0x0003_0002u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());

    let dir = scratch("respawn");
    let path = dir.join("respawn.vpk");
    write(&path, &bytes);

    assert!(matches!(
        Vpk::open(&path),
        Err(VpkError::UnsupportedVersion {
            version: 0x30002,
            ..
        })
    ));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn rejects_bad_entry_terminator() {
    let items = vec![Item {
        extension: "txt",
        directory: "docs",
        file_name: "a",
        preload: Vec::new(),
        data: b"data".to_vec(),
        archive_index: ARCHIVE_INDEX_SELF,
    }];
    let (mut bytes, _) = build_vpk(2, &items);
    // The terminator sits just before the embedded data, at the end of the tree.
    let tree_size = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let tree_end = 28 + tree_size;
    // The tree ends with three NUL bytes closing the file/directory/extension
    // loops, so the entry's 0xFFFF terminator sits just before them.
    bytes[tree_end - 5] = 0x00;

    let dir = scratch("terminator");
    let path = dir.join("bad-term.vpk");
    write(&path, &bytes);

    assert!(matches!(
        Vpk::open(&path),
        Err(VpkError::BadTerminator { .. })
    ));

    fs::remove_dir_all(&dir).ok();
}

/// Truncating the archive at every byte boundary must produce errors, never
/// panics. This is the cheap stand-in for a fuzz run.
#[test]
fn truncation_never_panics() {
    let items = vec![
        Item {
            extension: "vmdl_c",
            directory: "models",
            file_name: "a",
            preload: b"PP".to_vec(),
            data: b"DDDD".to_vec(),
            archive_index: ARCHIVE_INDEX_SELF,
        },
        Item {
            extension: "txt",
            directory: " ",
            file_name: "b",
            preload: Vec::new(),
            data: b"EEEE".to_vec(),
            archive_index: ARCHIVE_INDEX_SELF,
        },
    ];
    let (bytes, _) = build_vpk(2, &items);

    let dir = scratch("truncate");
    for cut in 0..bytes.len() {
        let path = dir.join("t.vpk");
        write(&path, &bytes[..cut]);
        if let Ok(archive) = Vpk::open(&path) {
            // Reading entries out of a truncated archive must also not panic.
            for entry in archive.entries() {
                let _ = archive.read_verified(entry);
            }
        }
    }

    fs::remove_dir_all(&dir).ok();
}

/// Flipping single bytes in the header and tree must never panic.
#[test]
fn bit_flips_never_panic() {
    let items = vec![Item {
        extension: "vmdl_c",
        directory: "models",
        file_name: "a",
        preload: b"PP".to_vec(),
        data: b"DDDD".to_vec(),
        archive_index: ARCHIVE_INDEX_SELF,
    }];
    let (bytes, _) = build_vpk(2, &items);

    let dir = scratch("flip");
    let path = dir.join("f.vpk");
    for i in 4..bytes.len() {
        for mask in [0x01u8, 0x80, 0xFF] {
            let mut corrupt = bytes.clone();
            corrupt[i] ^= mask;
            write(&path, &corrupt);
            if let Ok(archive) = Vpk::open(&path) {
                for entry in archive.entries() {
                    let _ = archive.read_verified(entry);
                }
            }
        }
    }

    fs::remove_dir_all(&dir).ok();
}
