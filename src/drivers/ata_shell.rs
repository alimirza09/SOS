use crate::ata::{ATA_CONTROLLER, AtaError};
use crate::fs::AtaFilesystem;
use crate::task::keyboard::read_line;
use crate::{print, println};
use alloc::string::String;
use alloc::vec::Vec;

pub struct AtaShell {
    filesystem: Option<AtaFilesystem>,
    current_device: usize,
}

impl AtaShell {
    pub fn new() -> Self {
        AtaShell {
            filesystem: None,
            current_device: 0,
        }
    }

    pub async fn run(&mut self) {
        println!("ATA Shell - Type 'help' for commands");

        loop {
            print!("ata> ");

            if let Some(command_line) = self.read_command().await {
                let parts: Vec<&str> = command_line.trim().split_whitespace().collect();
                if parts.is_empty() {
                    continue;
                }

                match parts[0] {
                    "help" => self.help(),
                    "devices" => self.list_devices(),
                    "select" => self.select_device(&parts).await,
                    "info" => self.device_info(),
                    "read" => self.read_sector(&parts).await,
                    "write" => self.write_sector(&parts).await,
                    "mount" => self.mount_filesystem().await,
                    "ls" => self.list_files(),
                    "cat" => self.cat_file(&parts).await,
                    "create" => self.create_file(&parts).await,
                    "delete" => self.delete_file(&parts).await,
                    "exit" => {
                        println!("Exiting ATA shell");
                        break;
                    }
                    _ => println!(
                        "Unknown command: {}. Type 'help' for available commands.",
                        parts[0]
                    ),
                }
            }
        }
    }

    async fn read_command(&self) -> Option<String> {
        let mut buffer = String::new();

        loop {
            if let Some(c) = read_line().await {
                match c {
                    '\n' | '\r' => {
                        println!();
                        return Some(buffer);
                    }
                    '\x08' => {
                        if !buffer.is_empty() {
                            buffer.pop();
                            print!("\x08 \x08");
                        }
                    }
                    c if c.is_ascii() && !c.is_control() => {
                        buffer.push(c);
                        print!("{}", c);
                    }
                    _ => {}
                }
            }
        }
    }

    fn help(&self) {
        println!("Available commands:");
        println!("  help        - Show this help message");
        println!("  devices     - List available ATA devices");
        println!("  select <n>  - Select ATA device n");
        println!("  info        - Show current device information");
        println!("  read <lba>  - Read sector at LBA address");
        println!("  write <lba> <data> - Write data to sector (dangerous!)");
        println!("  mount       - Mount filesystem on current device");
        println!("  ls          - List files in mounted filesystem");
        println!("  cat <file>  - Display file contents");
        println!("  create <file> <content> - Create a new file");
        println!("  delete <file> - Delete a file");
        println!("  exit        - Exit ATA shell");
    }

    fn list_devices(&self) {
        let controller = ATA_CONTROLLER.lock();
        let device_count = controller.get_device_count();

        if device_count == 0 {
            println!("No ATA devices found");
        } else {
            println!("ATA Devices:");
            for i in 0..device_count {
                if let Some(device) = controller.get_device(i) {
                    let marker = if i == self.current_device { "* " } else { "  " };
                    println!(
                        "{}Device {}: {} - {} sectors",
                        marker,
                        i,
                        device.get_model(),
                        device.get_sector_count()
                    );
                }
            }
        }
    }

    async fn select_device(&mut self, parts: &[&str]) {
        if parts.len() != 2 {
            println!("Usage: select <device_number>");
            return;
        }

        if let Ok(device_id) = parts[1].parse::<usize>() {
            let controller = ATA_CONTROLLER.lock();
            if device_id < controller.get_device_count() {
                self.current_device = device_id;
                self.filesystem = None; // Unmount any existing filesystem
                println!("Selected device {}", device_id);
            } else {
                println!("Device {} not found", device_id);
            }
        } else {
            println!("Invalid device number: {}", parts[1]);
        }
    }

    fn device_info(&self) {
        let controller = ATA_CONTROLLER.lock();
        if let Some(device) = controller.get_device(self.current_device) {
            println!("Device {}: {}", self.current_device, device.get_model());
            println!("Sectors: {}", device.get_sector_count());
            println!(
                "Capacity: {} MB",
                (device.get_sector_count() * 512) / (1024 * 1024)
            );
        } else {
            println!("No device selected or device not available");
        }
    }

    async fn read_sector(&self, parts: &[&str]) {
        if parts.len() != 2 {
            println!("Usage: read <lba>");
            return;
        }

        if let Ok(lba) = parts[1].parse::<u32>() {
            let mut buffer = [0u8; 512];
            match crate::ata::read_sector(self.current_device, lba, &mut buffer) {
                Ok(()) => {
                    println!("Sector {} contents:", lba);
                    for i in (0..512).step_by(16) {
                        print!("{:04X}: ", i);
                        for j in 0..16 {
                            if i + j < 512 {
                                print!("{:02X} ", buffer[i + j]);
                            }
                        }
                        print!("  ");
                        for j in 0..16 {
                            if i + j < 512 {
                                let c = buffer[i + j];
                                if c >= 32 && c <= 126 {
                                    print!("{}", c as char);
                                } else {
                                    print!(".");
                                }
                            }
                        }
                        println!();
                    }
                }
                Err(e) => println!("Failed to read sector {}: {:?}", lba, e),
            }
        } else {
            println!("Invalid LBA: {}", parts[1]);
        }
    }

    async fn write_sector(&self, parts: &[&str]) {
        if parts.len() < 3 {
            println!("Usage: write <lba> <data>");
            println!("Warning: This will overwrite data on the disk!");
            return;
        }

        if let Ok(lba) = parts[1].parse::<u32>() {
            let data_str = parts[2..].join(" ");
            let mut buffer = [0u8; 512];
            let bytes = data_str.as_bytes();
            let copy_len = bytes.len().min(512);
            buffer[..copy_len].copy_from_slice(&bytes[..copy_len]);

            println!(
                "Warning: This will write to sector {}. Continue? (y/N)",
                lba
            );
            if let Some(response) = self.read_command().await {
                if response.trim().to_lowercase() == "y" {
                    match crate::ata::write_sector(self.current_device, lba, &buffer) {
                        Ok(()) => println!("Successfully wrote to sector {}", lba),
                        Err(e) => println!("Failed to write sector {}: {:?}", lba, e),
                    }
                } else {
                    println!("Write cancelled");
                }
            }
        } else {
            println!("Invalid LBA: {}", parts[1]);
        }
    }

    async fn mount_filesystem(&mut self) {
        match AtaFilesystem::new(self.current_device) {
            Ok(fs) => {
                println!(
                    "Successfully mounted filesystem on device {}",
                    self.current_device
                );
                self.filesystem = Some(fs);
            }
            Err(e) => {
                println!("Failed to mount filesystem: {:?}", e);
            }
        }
    }

    fn list_files(&self) {
        if let Some(ref fs) = self.filesystem {
            let files = fs.list_files();
            if files.is_empty() {
                println!("No files found");
            } else {
                println!("Files:");
                for (name, size) in files {
                    println!("  {} ({} bytes)", name, size);
                }
            }
        } else {
            println!("No filesystem mounted. Use 'mount' command first.");
        }
    }

    async fn cat_file(&self, parts: &[&str]) {
        if parts.len() != 2 {
            println!("Usage: cat <filename>");
            return;
        }

        if let Some(ref fs) = self.filesystem {
            match fs.read_file(parts[1]) {
                Ok(data) => {
                    println!("Contents of '{}':", parts[1]);
                    match core::str::from_utf8(&data) {
                        Ok(text) => println!("{}", text),
                        Err(_) => {
                            println!("Binary file - showing hex dump:");
                            for (i, chunk) in data.chunks(16).enumerate() {
                                print!("{:04X}: ", i * 16);
                                for byte in chunk {
                                    print!("{:02X} ", byte);
                                }
                                println!();
                            }
                        }
                    }
                }
                Err(e) => println!("Failed to read file '{}': {:?}", parts[1], e),
            }
        } else {
            println!("No filesystem mounted. Use 'mount' command first.");
        }
    }

    async fn create_file(&mut self, parts: &[&str]) {
        if parts.len() < 3 {
            println!("Usage: create <filename> <content>");
            return;
        }

        if let Some(ref mut fs) = self.filesystem {
            let filename = parts[1];
            let content = parts[2..].join(" ");

            match fs.create_file(filename, content.as_bytes()) {
                Ok(()) => println!("Successfully created file '{}'", filename),
                Err(e) => println!("Failed to create file '{}': {:?}", filename, e),
            }
        } else {
            println!("No filesystem mounted. Use 'mount' command first.");
        }
    }

    async fn delete_file(&mut self, parts: &[&str]) {
        if parts.len() != 2 {
            println!("Usage: delete <filename>");
            return;
        }

        if let Some(ref mut fs) = self.filesystem {
            match fs.delete_file(parts[1]) {
                Ok(()) => println!("Successfully deleted file '{}'", parts[1]),
                Err(e) => println!("Failed to delete file '{}': {:?}", parts[1], e),
            }
        } else {
            println!("No filesystem mounted. Use 'mount' command first.");
        }
    }
}

pub async fn run_ata_shell() {
    let mut shell = AtaShell::new();
    shell.run().await;
}
