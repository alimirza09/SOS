use crate::drivers::ata::AtaDevice;
use crate::fs::ata_block::SosAtaBlockDevice;
use embedded_sdmmc::{Mode, TimeSource, Timestamp, VolumeIdx, VolumeManager};
use spin::Mutex;

struct DummyTime;
impl TimeSource for DummyTime {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp {
            year_since_1970: 54, // 2024
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

pub fn mount_root_fs(device: AtaDevice, block_count: u32) {
    let dev = SosAtaBlockDevice {
        primary: true,
        device,
        block_count,
    };
    let manager = VolumeManager::new(dev, DummyTime);
    *VOLUME_MANAGER.lock() = Some(manager);
}

pub fn write_file(path: &str, data: &[u8]) -> Result<(), &'static str> {
    let mut guard = VOLUME_MANAGER.lock();
    let manager = guard.as_mut().ok_or("No volume manager")?;

    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| "open_volume failed")?;
    let mut root_dir = volume.open_root_dir().map_err(|_| "open_root_dir failed")?;
    let mut file = root_dir
        .open_file_in_dir(path, Mode::ReadWriteCreateOrTruncate)
        .map_err(|_| "open_file failed")?;
    file.write(data).map_err(|_| "file.write failed")?;
    Ok(())
}

pub fn read_file(path: &str, buf: &mut [u8]) -> Result<usize, &'static str> {
    let mut guard = VOLUME_MANAGER.lock();
    let manager = guard.as_mut().ok_or("No volume manager")?;

    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| "open_volume failed")?;
    let mut root_dir = volume.open_root_dir().map_err(|_| "open_root_dir failed")?;
    let mut file = root_dir
        .open_file_in_dir(path, Mode::ReadOnly)
        .map_err(|_| "open_file failed")?;
    let n = file.read(buf).map_err(|_| "file.read failed")?;
    Ok(n)
}

pub fn remove_file(path: &str) -> Result<(), &'static str> {
    let mut guard = VOLUME_MANAGER.lock();
    let manager = guard.as_mut().ok_or("No volume manager")?;

    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| "open_volume failed")?;
    let mut root_dir = volume.open_root_dir().map_err(|_| "open_root_dir failed")?;
    root_dir
        .delete_file_in_dir(path)
        .map_err(|_| "delete_file failed")?;
    Ok(())
}

use crate::serial_println as println;

pub fn test_fat32() {
    let test_path = "FATTEST.TXT";
    let test_data = b"Hello FAT32 from SOS";
    let mut buf = [0u8; 64];

    match write_file(test_path, test_data) {
        Ok(()) => {
            println!("FAT32 test: file written");
        }
        Err(e) => {
            println!("FAT32 test: write failed: {}", e);
            return;
        }
    }

    // Read file
    let n = match read_file(test_path, &mut buf) {
        Ok(n) => {
            println!("FAT32 test: file read, {} bytes", n);
            n
        }
        Err(e) => {
            println!("FAT32 test: read failed: {}", e);
            return;
        }
    };

    // Verify content
    if &buf[..n] == test_data {
        println!("FAT32 test: content matches");
    } else {
        println!("FAT32 test: content does not match");
    }
}

pub fn test_fat32_on_primary_slave() {
    use crate::drivers::ata::AtaDevice;

    let device = AtaDevice::Slave;
    let block_count: u32 = 131072;

    // mount_root_fs should accept &mut AtaController, etc.
    crate::fs::fat::mount_root_fs(device, block_count);

    crate::fs::fat::test_fat32();
}
