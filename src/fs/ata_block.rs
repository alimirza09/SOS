use crate::drivers::ata::{read_sectors, write_sectors, AtaDevice, AtaError};
use embedded_sdmmc::{Block, BlockCount, BlockDevice, BlockIdx};

pub struct SosAtaBlockDevice {
    pub primary: bool,
    pub device: AtaDevice,
    pub block_count: u32,
}

impl BlockDevice for SosAtaBlockDevice {
    type Error = AtaError;

    fn read(
        &self,
        blocks: &mut [Block],
        start_block_idx: BlockIdx,
        _reason: &str,
    ) -> Result<(), Self::Error> {
        for (i, block) in blocks.iter_mut().enumerate() {
            let lba = start_block_idx.0 + i as u32;
            let buf = block.as_mut();
            read_sectors(self.primary, self.device, lba as u64, 1, buf)?;
        }
        Ok(())
    }

    fn write(&self, blocks: &[Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        for (i, block) in blocks.iter().enumerate() {
            let lba = start_block_idx.0 + i as u32;
            let buf = block.as_ref();
            write_sectors(self.primary, self.device, lba as u64, buf)?;
        }
        Ok(())
    }

    fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
        Ok(BlockCount(self.block_count))
    }
}
