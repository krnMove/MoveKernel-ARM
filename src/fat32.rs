use alloc::vec::Vec;
use core::fmt::{self, Write};

use crate::virtio_blk::{self, Error as BlockError};

const SECTOR_SIZE: usize = 512;
const END_OF_CHAIN: u32 = 0x0fff_fff8;
const BAD_CLUSTER: u32 = 0x0fff_fff7;

#[derive(Clone, Copy, Debug)]
pub enum Error {
    Block(BlockError),
    InvalidBootSector,
    UnsupportedGeometry,
    VolumeExceedsDisk,
    CorruptVolume,
    NotFound,
    IsDirectory,
    BufferTooSmall,
    InvalidShortName,
    DiskFull,
    DirectoryFull,
    OutOfMemory,
    FileTooLarge,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Block(error) => write!(formatter, "{error}"),
            Self::InvalidBootSector => formatter.write_str("invalid FAT32 boot sector"),
            Self::UnsupportedGeometry => formatter.write_str("unsupported FAT32 volume geometry"),
            Self::VolumeExceedsDisk => formatter.write_str("FAT32 volume exceeds the disk"),
            Self::CorruptVolume => formatter.write_str("corrupt FAT32 cluster chain"),
            Self::NotFound => formatter.write_str("file not found in the root directory"),
            Self::IsDirectory => formatter.write_str("the requested entry is a directory"),
            Self::BufferTooSmall => formatter.write_str("file does not fit in the output buffer"),
            Self::InvalidShortName => formatter.write_str("filename must use the 8.3 format"),
            Self::DiskFull => formatter.write_str("disk is full"),
            Self::DirectoryFull => formatter.write_str("root directory is full"),
            Self::OutOfMemory => formatter.write_str("not enough kernel memory"),
            Self::FileTooLarge => formatter.write_str("file is too large for FAT32"),
        }
    }
}

impl From<BlockError> for Error {
    fn from(error: BlockError) -> Self {
        Self::Block(error)
    }
}

#[derive(Clone, Copy)]
pub struct Fat32 {
    sectors_per_cluster: u8,
    reserved_sectors: u32,
    sectors_per_fat: u32,
    fat_count: u8,
    active_fat: u8,
    mirrored_fats: bool,
    root_cluster: u32,
    first_data_sector: u32,
    total_sectors: u32,
    cluster_count: u32,
}

#[derive(Clone, Copy)]
pub struct Entry {
    short_name: [u8; 11],
    attributes: u8,
    first_cluster: u32,
    size: u32,
}

impl Entry {
    pub fn is_directory(&self) -> bool {
        self.attributes & 0x10 != 0
    }

    pub fn name(&self) -> FileName {
        FileName(self.short_name)
    }
}

pub struct FileName([u8; 11]);

impl fmt::Display for FileName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let base_end = self.0[..8]
            .iter()
            .rposition(|byte| *byte != b' ')
            .map_or(0, |index| index + 1);
        for &byte in &self.0[..base_end] {
            formatter.write_char(if byte.is_ascii_graphic() {
                byte as char
            } else {
                '?'
            })?;
        }

        let extension_end = self.0[8..]
            .iter()
            .rposition(|byte| *byte != b' ')
            .map_or(0, |index| index + 1);
        if extension_end != 0 {
            formatter.write_char('.')?;
            for &byte in &self.0[8..8 + extension_end] {
                formatter.write_char(if byte.is_ascii_graphic() {
                    byte as char
                } else {
                    '?'
                })?;
            }
        }
        Ok(())
    }
}

fn le16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

impl Fat32 {
    pub fn mount() -> Result<Self, Error> {
        virtio_blk::init()?;

        let mut boot_sector = [0; SECTOR_SIZE];
        virtio_blk::read_sector(0, &mut boot_sector)?;
        if boot_sector[510] != 0x55 || boot_sector[511] != 0xaa {
            return Err(Error::InvalidBootSector);
        }

        let bytes_per_sector = le16(&boot_sector, 11);
        let sectors_per_cluster = boot_sector[13];
        let reserved_sectors = le16(&boot_sector, 14) as u32;
        let fat_count = boot_sector[16];
        let total_sectors = if le32(&boot_sector, 32) != 0 {
            le32(&boot_sector, 32)
        } else {
            le16(&boot_sector, 19) as u32
        };
        let sectors_per_fat = le32(&boot_sector, 36);
        let fat_flags = le16(&boot_sector, 40);
        let root_cluster = le32(&boot_sector, 44) & 0x0fff_ffff;
        let mirrored_fats = fat_flags & 0x80 == 0;
        let active_fat = if mirrored_fats {
            0
        } else {
            (fat_flags & 0x0f) as u8
        };

        if bytes_per_sector != SECTOR_SIZE as u16
            || sectors_per_cluster == 0
            || !sectors_per_cluster.is_power_of_two()
            || reserved_sectors == 0
            || fat_count == 0
            || sectors_per_fat == 0
            || total_sectors == 0
            || root_cluster < 2
            || active_fat >= fat_count
        {
            return Err(Error::UnsupportedGeometry);
        }
        if total_sectors as u64 > virtio_blk::capacity_sectors() {
            return Err(Error::VolumeExceedsDisk);
        }

        let first_data_sector = reserved_sectors
            .checked_add(
                (fat_count as u32)
                    .checked_mul(sectors_per_fat)
                    .ok_or(Error::UnsupportedGeometry)?,
            )
            .ok_or(Error::UnsupportedGeometry)?;
        if first_data_sector >= total_sectors {
            return Err(Error::UnsupportedGeometry);
        }
        let cluster_count = (total_sectors - first_data_sector) / sectors_per_cluster as u32;
        if cluster_count == 0
            || cluster_count > BAD_CLUSTER - 2
            || root_cluster > cluster_count.saturating_add(1)
            || (cluster_count as u64 + 2) * 4 > sectors_per_fat as u64 * SECTOR_SIZE as u64
        {
            return Err(Error::UnsupportedGeometry);
        }

        Ok(Self {
            sectors_per_cluster,
            reserved_sectors,
            sectors_per_fat,
            fat_count,
            active_fat,
            mirrored_fats,
            root_cluster,
            first_data_sector,
            total_sectors,
            cluster_count,
        })
    }

    pub fn total_sectors(&self) -> u32 {
        self.total_sectors
    }

    pub fn list_root(&self, mut visit: impl FnMut(Entry) -> bool) -> Result<(), Error> {
        let mut cluster = self.root_cluster;
        for _ in 0..self.cluster_count {
            let first_sector = self.cluster_sector(cluster)?;
            for offset in 0..self.sectors_per_cluster as u32 {
                let mut sector = [0; SECTOR_SIZE];
                virtio_blk::read_sector((first_sector + offset) as u64, &mut sector)?;
                for entry_offset in (0..SECTOR_SIZE).step_by(32) {
                    let entry = &sector[entry_offset..entry_offset + 32];
                    if entry[0] == 0 {
                        return Ok(());
                    }
                    if entry[0] == 0xe5 || entry[11] == 0x0f || entry[11] & 0x08 != 0 {
                        continue;
                    }

                    let parsed = Entry {
                        short_name: entry[..11].try_into().unwrap(),
                        attributes: entry[11],
                        first_cluster: (((le16(entry, 20) as u32) << 16) | le16(entry, 26) as u32)
                            & 0x0fff_ffff,
                        size: le32(entry, 28),
                    };
                    if !visit(parsed) {
                        return Ok(());
                    }
                }
            }

            match self.next_cluster(cluster)? {
                Some(next) => cluster = next,
                None => return Ok(()),
            }
        }
        Err(Error::CorruptVolume)
    }

    pub fn read_file_83(&self, name: &str, output: &mut [u8]) -> Result<usize, Error> {
        let short_name = to_short_name(name)?;
        let mut found = None;
        self.list_root(|entry| {
            if entry.short_name == short_name {
                found = Some(entry);
                false
            } else {
                true
            }
        })?;

        let entry = found.ok_or(Error::NotFound)?;
        if entry.is_directory() {
            return Err(Error::IsDirectory);
        }
        let file_size = entry.size as usize;
        if file_size > output.len() {
            return Err(Error::BufferTooSmall);
        }
        if file_size == 0 {
            return Ok(0);
        }

        let mut cluster = entry.first_cluster;
        let mut copied = 0;
        let mut sector = [0; SECTOR_SIZE];
        let sectors_per_cluster = self.sectors_per_cluster as usize;
        for _ in 0..self.cluster_count {
            if cluster < 2 || cluster > self.cluster_count.saturating_add(1) {
                return Err(Error::CorruptVolume);
            }
            let first_sector = self.cluster_sector(cluster)?;
            for offset in 0..sectors_per_cluster {
                if copied == file_size {
                    return Ok(copied);
                }
                virtio_blk::read_sector((first_sector as usize + offset) as u64, &mut sector)?;
                let count = (file_size - copied).min(SECTOR_SIZE);
                output[copied..copied + count].copy_from_slice(&sector[..count]);
                copied += count;
            }

            if copied == file_size {
                return Ok(copied);
            }
            cluster = self.next_cluster(cluster)?.ok_or(Error::CorruptVolume)?;
        }
        Err(Error::CorruptVolume)
    }

    pub fn write_file_83(&self, name: &str, contents: &[u8]) -> Result<(), Error> {
        if contents.len() > u32::MAX as usize {
            return Err(Error::FileTooLarge);
        }

        let short_name = to_short_name(name)?;
        let (found, free_slot) = self.find_root_entry(short_name)?;
        if let Some((entry, _, _)) = found {
            if entry.is_directory() {
                return Err(Error::IsDirectory);
            }
        } else if free_slot.is_none() {
            return Err(Error::DirectoryFull);
        }

        let mut chain = match found {
            Some((entry, _, _)) if entry.first_cluster != 0 => {
                self.collect_cluster_chain(entry.first_cluster)?
            }
            _ => Vec::new(),
        };
        let bytes_per_cluster = self.sectors_per_cluster as usize * SECTOR_SIZE;
        let required_clusters = contents.len().div_ceil(bytes_per_cluster);
        if required_clusters > self.cluster_count as usize {
            return Err(Error::DiskFull);
        }
        chain
            .try_reserve_exact(required_clusters.saturating_sub(chain.len()))
            .map_err(|_| Error::OutOfMemory)?;

        let original_chain_len = chain.len();
        while chain.len() < required_clusters {
            let cluster = match self.find_free_cluster() {
                Ok(cluster) => cluster,
                Err(error) => {
                    self.free_clusters(&chain[original_chain_len..])?;
                    return Err(error);
                }
            };
            chain.push(cluster);
            if let Err(error) = self.set_fat_entry(cluster, END_OF_CHAIN) {
                self.free_clusters(&chain[original_chain_len..])?;
                return Err(error);
            }
        }

        let write_contents = (|| {
            for (index, &cluster) in chain.iter().take(required_clusters).enumerate() {
                let first_sector = self.cluster_sector(cluster)?;
                for sector_offset in 0..self.sectors_per_cluster as usize {
                    let file_offset = index * bytes_per_cluster + sector_offset * SECTOR_SIZE;
                    let mut sector = [0; SECTOR_SIZE];
                    if file_offset < contents.len() {
                        let count = (contents.len() - file_offset).min(SECTOR_SIZE);
                        sector[..count]
                            .copy_from_slice(&contents[file_offset..file_offset + count]);
                    }
                    virtio_blk::write_sector(
                        (first_sector as usize + sector_offset) as u64,
                        &sector,
                    )?;
                }
            }
            Ok::<(), Error>(())
        })();
        if let Err(error) = write_contents {
            self.free_clusters(&chain[original_chain_len..])?;
            return Err(error);
        }

        for (index, &cluster) in chain.iter().take(required_clusters).enumerate() {
            let next = chain.get(index + 1).copied().unwrap_or(END_OF_CHAIN);
            self.set_fat_entry(cluster, next)?;
        }

        let (directory_sector, directory_offset) = found
            .map(|(_, sector, offset)| (sector, offset))
            .or(free_slot)
            .ok_or(Error::DirectoryFull)?;
        let mut directory = [0; SECTOR_SIZE];
        virtio_blk::read_sector(directory_sector, &mut directory)?;
        let record = &mut directory[directory_offset..directory_offset + 32];
        record.fill(0);
        record[..11].copy_from_slice(&short_name);
        record[11] = 0x20;
        record[14..16].copy_from_slice(&0u16.to_le_bytes());
        record[16..18].copy_from_slice(&0x0021u16.to_le_bytes());
        record[18..20].copy_from_slice(&0x0021u16.to_le_bytes());
        record[22..24].copy_from_slice(&0u16.to_le_bytes());
        record[24..26].copy_from_slice(&0x0021u16.to_le_bytes());
        let first_cluster = if required_clusters == 0 { 0 } else { chain[0] };
        record[20..22].copy_from_slice(&((first_cluster >> 16) as u16).to_le_bytes());
        record[26..28].copy_from_slice(&(first_cluster as u16).to_le_bytes());
        record[28..32].copy_from_slice(&(contents.len() as u32).to_le_bytes());
        virtio_blk::write_sector(directory_sector, &directory)?;
        self.free_clusters(&chain[required_clusters.min(chain.len())..])?;
        Ok(())
    }

    fn find_root_entry(
        &self,
        short_name: [u8; 11],
    ) -> Result<(Option<(Entry, u64, usize)>, Option<(u64, usize)>), Error> {
        let mut cluster = self.root_cluster;
        let mut free_slot = None;
        for _ in 0..self.cluster_count {
            let first_sector = self.cluster_sector(cluster)?;
            for offset in 0..self.sectors_per_cluster as u32 {
                let sector_number = (first_sector + offset) as u64;
                let mut sector = [0; SECTOR_SIZE];
                virtio_blk::read_sector(sector_number, &mut sector)?;
                for entry_offset in (0..SECTOR_SIZE).step_by(32) {
                    let raw = &sector[entry_offset..entry_offset + 32];
                    if raw[0] == 0 {
                        return Ok((None, free_slot.or(Some((sector_number, entry_offset)))));
                    }
                    if raw[0] == 0xe5 {
                        free_slot.get_or_insert((sector_number, entry_offset));
                        continue;
                    }
                    if raw[11] == 0x0f || raw[11] & 0x08 != 0 {
                        continue;
                    }

                    let entry = parse_entry(raw);
                    if entry.short_name == short_name {
                        return Ok((Some((entry, sector_number, entry_offset)), free_slot));
                    }
                }
            }

            match self.next_cluster(cluster)? {
                Some(next) => cluster = next,
                None => return Ok((None, free_slot)),
            }
        }
        Err(Error::CorruptVolume)
    }

    fn collect_cluster_chain(&self, first: u32) -> Result<Vec<u32>, Error> {
        let mut chain = Vec::new();
        let mut cluster = first;
        for _ in 0..self.cluster_count {
            if cluster < 2 || cluster > self.cluster_count.saturating_add(1) {
                return Err(Error::CorruptVolume);
            }
            chain.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
            chain.push(cluster);
            match self.next_cluster(cluster)? {
                Some(next) => cluster = next,
                None => return Ok(chain),
            }
        }
        Err(Error::CorruptVolume)
    }

    fn find_free_cluster(&self) -> Result<u32, Error> {
        let last_cluster = self.cluster_count.saturating_add(1);
        for sector_index in 0..self.sectors_per_fat {
            let fat_offset = sector_index
                .checked_mul(SECTOR_SIZE as u32)
                .ok_or(Error::CorruptVolume)?;
            let sector_number = self.fat_sector(self.active_fat, fat_offset)?;
            let mut sector = [0; SECTOR_SIZE];
            virtio_blk::read_sector(sector_number as u64, &mut sector)?;
            for entry_index in 0..SECTOR_SIZE / 4 {
                let cluster = sector_index as u64 * (SECTOR_SIZE / 4) as u64 + entry_index as u64;
                if cluster > last_cluster as u64 {
                    return Err(Error::DiskFull);
                }
                if cluster >= 2 && le32(&sector, entry_index * 4) & 0x0fff_ffff == 0 {
                    return Ok(cluster as u32);
                }
            }
        }
        Err(Error::DiskFull)
    }

    fn set_fat_entry(&self, cluster: u32, value: u32) -> Result<(), Error> {
        let fat_offset = cluster.checked_mul(4).ok_or(Error::CorruptVolume)?;
        let first_fat = if self.mirrored_fats {
            0
        } else {
            self.active_fat
        };
        let fat_count = if self.mirrored_fats {
            self.fat_count
        } else {
            1
        };
        for index in 0..fat_count {
            let fat_index = first_fat + index;
            let sector_number = self.fat_sector(fat_index, fat_offset)?;
            let mut sector = [0; SECTOR_SIZE];
            virtio_blk::read_sector(sector_number as u64, &mut sector)?;
            let offset = (fat_offset % SECTOR_SIZE as u32) as usize;
            let reserved = le32(&sector, offset) & 0xf000_0000;
            sector[offset..offset + 4]
                .copy_from_slice(&(reserved | (value & 0x0fff_ffff)).to_le_bytes());
            virtio_blk::write_sector(sector_number as u64, &sector)?;
        }
        Ok(())
    }

    fn fat_sector(&self, fat_index: u8, fat_offset: u32) -> Result<u32, Error> {
        if fat_offset / SECTOR_SIZE as u32 >= self.sectors_per_fat {
            return Err(Error::CorruptVolume);
        }
        self.reserved_sectors
            .checked_add(
                (fat_index as u32)
                    .checked_mul(self.sectors_per_fat)
                    .ok_or(Error::CorruptVolume)?,
            )
            .and_then(|start| start.checked_add(fat_offset / SECTOR_SIZE as u32))
            .ok_or(Error::CorruptVolume)
    }

    fn free_clusters(&self, clusters: &[u32]) -> Result<(), Error> {
        for &cluster in clusters {
            self.set_fat_entry(cluster, 0)?;
        }
        Ok(())
    }

    fn cluster_sector(&self, cluster: u32) -> Result<u32, Error> {
        if cluster < 2 || cluster > self.cluster_count.saturating_add(1) {
            return Err(Error::CorruptVolume);
        }
        self.first_data_sector
            .checked_add(
                (cluster - 2)
                    .checked_mul(self.sectors_per_cluster as u32)
                    .ok_or(Error::CorruptVolume)?,
            )
            .ok_or(Error::CorruptVolume)
    }

    fn next_cluster(&self, cluster: u32) -> Result<Option<u32>, Error> {
        let fat_offset = cluster.checked_mul(4).ok_or(Error::CorruptVolume)?;
        let fat_sector = self.fat_sector(self.active_fat, fat_offset)?;
        let mut sector = [0; SECTOR_SIZE];
        virtio_blk::read_sector(fat_sector as u64, &mut sector)?;
        let value = le32(&sector, (fat_offset % SECTOR_SIZE as u32) as usize) & 0x0fff_ffff;
        if value >= END_OF_CHAIN {
            return Ok(None);
        }
        if value == 0 || value == 1 || value == BAD_CLUSTER {
            return Err(Error::CorruptVolume);
        }
        Ok(Some(value))
    }
}

fn parse_entry(entry: &[u8]) -> Entry {
    Entry {
        short_name: entry[..11].try_into().unwrap(),
        attributes: entry[11],
        first_cluster: (((le16(entry, 20) as u32) << 16) | le16(entry, 26) as u32) & 0x0fff_ffff,
        size: le32(entry, 28),
    }
}

fn to_short_name(name: &str) -> Result<[u8; 11], Error> {
    let mut result = [b' '; 11];
    let mut parts = name.split('.');
    let base = parts.next().ok_or(Error::InvalidShortName)?;
    let extension = parts.next().unwrap_or("");
    if parts.next().is_some()
        || base.is_empty()
        || base.len() > 8
        || extension.len() > 3
        || !base.is_ascii()
        || !extension.is_ascii()
    {
        return Err(Error::InvalidShortName);
    }

    for (index, byte) in base.bytes().enumerate() {
        if !is_short_name_character(byte) {
            return Err(Error::InvalidShortName);
        }
        result[index] = byte.to_ascii_uppercase();
    }
    for (index, byte) in extension.bytes().enumerate() {
        if !is_short_name_character(byte) {
            return Err(Error::InvalidShortName);
        }
        result[8 + index] = byte.to_ascii_uppercase();
    }
    Ok(result)
}

fn is_short_name_character(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"$%'-_@~`!(){}^#&".contains(&byte)
}
