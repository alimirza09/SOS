use alloc::string::{String, ToString};
use alloc::vec::Vec;
use embedded_sdmmc::{Directory, Mode, TimeSource, Timestamp, VolumeIdx, VolumeManager};
use spin::Mutex;

use crate::fs::ata_block::SosAtaBlockDevice;

struct DummyTime;
impl TimeSource for DummyTime {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp {
            year_since_1970: 54,
            zero_indexed_month: 0,
            zero_indexed_day: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}

pub static VOLUME_MANAGER: Mutex<Option<VolumeManager<SosAtaBlockDevice, DummyTime>>> =
    Mutex::new(None);

pub fn mount_root_fs(device: crate::drivers::ata::AtaDevice, block_count: u32) {
    let dev = SosAtaBlockDevice {
        primary: true,
        device,
        block_count,
    };
    let manager = VolumeManager::new(dev, DummyTime);
    *VOLUME_MANAGER.lock() = Some(manager);
}

fn split_path(path: &str) -> Vec<&str> {
    path.split('/').filter(|p| !p.is_empty()).collect()
}

pub fn write_file(path: &str, data: &[u8]) -> Result<(), &'static str> {
    let components = split_path(path);

    if components.len() != 1 {
        return Err("Only root directory files supported currently");
    }

    let file_name = components[0];

    let mut guard = VOLUME_MANAGER.lock();
    let manager = guard.as_mut().ok_or("No volume manager")?;
    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| "open_volume failed")?;

    let mut root_dir = volume.open_root_dir().map_err(|_| "open_root_dir failed")?;
    let mut file = root_dir
        .open_file_in_dir(file_name, Mode::ReadWriteCreateOrTruncate)
        .map_err(|_| "open_file failed")?;
    file.write(data).map_err(|_| "file.write failed")?;
    Ok(())
}

pub fn read_file(path: &str, buf: &mut [u8]) -> Result<usize, &'static str> {
    let components = split_path(path);

    if components.len() != 1 {
        return Err("Only root directory files supported currently");
    }

    let file_name = components[0];

    let mut guard = VOLUME_MANAGER.lock();
    let manager = guard.as_mut().ok_or("No volume manager")?;
    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| "open_volume failed")?;

    let mut root_dir = volume.open_root_dir().map_err(|_| "open_root_dir failed")?;
    let mut file = root_dir
        .open_file_in_dir(file_name, Mode::ReadOnly)
        .map_err(|_| "open_file failed")?;
    let n = file.read(buf).map_err(|_| "file.read failed")?;
    Ok(n)
}

pub fn remove_file(path: &str) -> Result<(), &'static str> {
    let components = split_path(path);

    if components.len() != 1 {
        return Err("Only root directory files supported currently");
    }

    let file_name = components[0];

    let mut guard = VOLUME_MANAGER.lock();
    let manager = guard.as_mut().ok_or("No volume manager")?;
    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| "open_volume failed")?;

    let mut root_dir = volume.open_root_dir().map_err(|_| "open_root_dir failed")?;
    root_dir
        .delete_file_in_dir(file_name)
        .map_err(|_| "delete_file failed")?;
    Ok(())
}

pub fn create_dir(path: &str) -> Result<(), &'static str> {
    let components = split_path(path);

    if components.len() != 1 {
        return Err("Only root directory creation supported currently");
    }

    let dir_name = components[0];

    let mut guard = VOLUME_MANAGER.lock();
    let manager = guard.as_mut().ok_or("No volume manager")?;
    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| "open_volume failed")?;

    let mut root_dir = volume.open_root_dir().map_err(|_| "open_root_dir failed")?;
    root_dir
        .make_dir_in_dir(dir_name)
        .map_err(|_| "make_dir_in_dir failed")?;
    Ok(())
}

pub fn remove_dir(path: &str) -> Result<(), &'static str> {
    let components = split_path(path);

    if components.len() != 1 {
        return Err("Only root directory removal supported currently");
    }

    let dir_name = components[0];

    let mut guard = VOLUME_MANAGER.lock();
    let manager = guard.as_mut().ok_or("No volume manager")?;
    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| "open_volume failed")?;

    let mut root_dir = volume.open_root_dir().map_err(|_| "open_root_dir failed")?;

    root_dir
        .delete_file_in_dir(dir_name)
        .map_err(|_| "Directory removal failed - method may not exist or directory not empty")?;
    Ok(())
}

pub fn list_dir(path: &str) -> Result<Vec<String>, &'static str> {
    let components = split_path(path);

    if !components.is_empty() {
        return Err("Only root directory listing supported currently");
    }

    let mut guard = VOLUME_MANAGER.lock();
    let manager = guard.as_mut().ok_or("No volume manager")?;
    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| "open_volume failed")?;

    let mut root_dir = volume.open_root_dir().map_err(|_| "open_root_dir failed")?;
    let mut names = Vec::new();
    root_dir
        .iterate_dir(|entry| {
            names.push(entry.name.to_string());
        })
        .map_err(|_| "iterate_dir failed")?;
    Ok(names)
}

pub fn test_fat32() {
    use crate::serial_println as println;

    println!("FAT32 test: Starting filesystem tests...");

    let test_path = "SOSTEST.TXT";
    let test_data = b"Hello FAT32 from SOS kernel!";
    let mut buf = [0u8; 128];

    match write_file(test_path, test_data) {
        Ok(()) => {
            println!("FAT32 test:  File written successfully");
        }
        Err(e) => {
            println!("FAT32 test:  Write failed: {}", e);
            return;
        }
    }

    let bytes_read = match read_file(test_path, &mut buf) {
        Ok(n) => {
            println!("FAT32 test:  File read successfully, {} bytes", n);
            n
        }
        Err(e) => {
            println!("FAT32 test:  Read failed: {}", e);
            return;
        }
    };

    if &buf[..bytes_read] == test_data {
        println!("FAT32 test:  Content verification passed");
    } else {
        println!("FAT32 test:  Content verification failed");
        println!(
            "Expected: {:?}",
            core::str::from_utf8(test_data).unwrap_or("invalid utf8")
        );
        println!(
            "Got: {:?}",
            core::str::from_utf8(&buf[..bytes_read]).unwrap_or("invalid utf8")
        );
        return;
    }

    let test_dir = "TESTDIR";
    match create_dir(test_dir) {
        Ok(()) => {
            println!("FAT32 test:  Directory created successfully");
        }
        Err(e) => {
            println!("FAT32 test:  Directory creation failed: {}", e);
        }
    }

    match list_dir("") {
        Ok(entries) => {
            println!("FAT32 test: Directory listing successful");
            println!("Root directory contains {} entries:", entries.len());
            for (i, entry) in entries.iter().enumerate() {
                if i < 10 {
                    println!("  - {}", entry);
                }
            }
            if entries.len() > 10 {
                println!("  ... and {} more entries", entries.len() - 10);
            }
        }
        Err(e) => {
            println!("FAT32 test:  Directory listing failed: {}", e);
        }
    }

    match remove_file(test_path) {
        Ok(()) => {
            println!("FAT32 test:  File removal successful");
        }
        Err(e) => {
            println!("FAT32 test:  File removal failed: {}", e);
        }
    }

    match read_file(test_path, &mut buf) {
        Ok(_) => {
            println!("FAT32 test:  File still exists after deletion");
        }
        Err(_) => {
            println!("FAT32 test:  File successfully deleted (read failed as expected)");
        }
    }

    println!("FAT32 test: All tests completed!");
}

pub fn test_fat32_with_device(device: crate::drivers::ata::AtaDevice, block_count: u32) {
    use crate::serial_println as println;

    println!(
        "Mounting FAT32 filesystem on device {:?} with {} blocks...",
        device, block_count
    );

    mount_root_fs(device, block_count);

    test_fat32();
}
