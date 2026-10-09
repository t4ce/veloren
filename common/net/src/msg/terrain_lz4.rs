//! Lossless terrain transport, independent of Chonk's internal storage.
//!
//! V1 voxel order is z, y, x (x fastest). Every cell is exactly four bytes:
//! BlockKind followed by the three raw RGB/sprite/attribute bytes. LZ4 is one
//! independent raw block, without a dictionary, frame, or size prefix. Bounds
//! and defaults describe the infinite columns outside the stored z interval.
use common::{
    terrain::{Block, TerrainChunk, TerrainChunkMeta, TerrainChunkSize},
    vol::{ReadVol, RectVolSize, WriteVol},
};
use serde::{Deserialize, Serialize};
use vek::Vec3;

pub const TERRAIN_LZ4_VERSION: u16 = 1;
pub const TERRAIN_LZ4_MAX_HEIGHT: u32 = 2048;
const BLOCK_BYTES: usize = 4;
const SUBCHUNK_HEIGHT: u32 = TerrainChunkSize::RECT_SIZE.x / 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lz4TerrainChunkV1 {
    pub version: u16,
    /// Server tick at serialization; scoped to this connection/world.
    pub revision: u64,
    pub width: u16,
    pub depth: u16,
    pub zmin: i32,
    pub zmax: i32,
    pub below: [u8; 4],
    pub above: [u8; 4],
    /// Existing game-protocol metadata, not compressed Chonk internals.
    pub meta: TerrainChunkMeta,
    pub uncompressed_len: u32,
    pub data: Vec<u8>,
}

/// The renderer can upload these same exact bytes without PNG reconstruction
/// or serializing the CPU chunk again. CPU materialization is a separate step.
#[derive(Debug, Clone)]
pub struct DecodedTerrainChunkV1 {
    pub revision: u64,
    pub width: u16,
    pub depth: u16,
    pub zmin: i32,
    pub zmax: i32,
    pub below: Block,
    pub above: Block,
    pub meta: TerrainChunkMeta,
    pub voxels: Vec<u8>,
}

fn voxel_bytes(width: u16, depth: u16, zmin: i32, zmax: i32) -> Option<usize> {
    if u32::from(width) != TerrainChunkSize::RECT_SIZE.x
        || u32::from(depth) != TerrainChunkSize::RECT_SIZE.y
    {
        return None;
    }
    let height = u32::try_from(zmax.checked_sub(zmin)?).ok()?;
    if height > TERRAIN_LZ4_MAX_HEIGHT || height % SUBCHUNK_HEIGHT != 0 {
        return None;
    }
    usize::from(width)
        .checked_mul(usize::from(depth))?
        .checked_mul(height as usize)?
        .checked_mul(BLOCK_BYTES)
}

impl Lz4TerrainChunkV1 {
    pub fn from_chunk(chunk: &TerrainChunk, revision: u64) -> Option<Self> {
        let width = u16::try_from(TerrainChunkSize::RECT_SIZE.x).ok()?;
        let depth = u16::try_from(TerrainChunkSize::RECT_SIZE.y).ok()?;
        let zmin = chunk.get_min_z();
        let zmax = chunk.get_max_z();
        let len = voxel_bytes(width, depth, zmin, zmax)?;
        let below = chunk
            .get(Vec3::new(0, 0, zmin.checked_sub(1)?))
            .ok()?
            .to_u32()
            .to_le_bytes();
        let above = chunk
            .get(Vec3::new(0, 0, zmax))
            .ok()?
            .to_u32()
            .to_le_bytes();
        let mut voxels = Vec::with_capacity(len);
        for z in zmin..zmax {
            for y in 0..i32::from(depth) {
                for x in 0..i32::from(width) {
                    voxels.extend_from_slice(
                        &chunk.get(Vec3::new(x, y, z)).ok()?.to_u32().to_le_bytes(),
                    );
                }
            }
        }
        Some(Self {
            version: TERRAIN_LZ4_VERSION,
            revision,
            width,
            depth,
            zmin,
            zmax,
            below,
            above,
            meta: chunk.meta().clone(),
            uncompressed_len: u32::try_from(len).ok()?,
            data: lz4_flex::block::compress(&voxels),
        })
    }

    pub fn decode(&self) -> Option<DecodedTerrainChunkV1> {
        if self.version != TERRAIN_LZ4_VERSION {
            return None;
        }
        let len = voxel_bytes(self.width, self.depth, self.zmin, self.zmax)?;
        if self.uncompressed_len as usize != len
            || self.data.len() > lz4_flex::block::get_maximum_output_size(len)
        {
            return None;
        }
        let below = Block::from_u32(u32::from_le_bytes(self.below))?;
        let above = Block::from_u32(u32::from_le_bytes(self.above))?;
        // Fixed-size output bounds both allocation and every decompressor write.
        let mut voxels = vec![0; len];
        if lz4_flex::block::decompress_into(&self.data, &mut voxels).ok()? != len {
            return None;
        }
        for raw in voxels.chunks_exact(BLOCK_BYTES) {
            Block::from_u32(u32::from_le_bytes(raw.try_into().ok()?))?;
        }
        Some(DecodedTerrainChunkV1 {
            revision: self.revision,
            width: self.width,
            depth: self.depth,
            zmin: self.zmin,
            zmax: self.zmax,
            below,
            above,
            meta: self.meta.clone(),
            voxels,
        })
    }

    pub fn to_chunk(&self) -> Option<TerrainChunk> { self.decode()?.into_chunk() }
}

impl DecodedTerrainChunkV1 {
    pub fn into_chunk(self) -> Option<TerrainChunk> {
        let len = voxel_bytes(self.width, self.depth, self.zmin, self.zmax)?;
        if self.voxels.len() != len {
            return None;
        }
        let mut chunk = TerrainChunk::new(self.zmin, self.below, self.above, self.meta);
        if self.zmax > self.zmin {
            // Chonk avoids allocating writes equal to its upper default. Force
            // the last subchunk into existence so even all-default tails retain
            // their advertised bounds, then restore the exact block below.
            let last = Vec3::new(0, 0, self.zmax.checked_sub(1)?);
            let sentinel = if self.above == Block::empty() {
                Block::new(common::terrain::BlockKind::Rock, vek::Rgb::new(0, 0, 0))
            } else {
                Block::empty()
            };
            chunk.set(last, sentinel).ok()?;
            chunk.set(last, self.above).ok()?;
        }
        for (cell, raw) in self.voxels.chunks_exact(BLOCK_BYTES).enumerate() {
            let x = cell % usize::from(self.width);
            let y = cell / usize::from(self.width) % usize::from(self.depth);
            let z = self.zmin.checked_add(
                i32::try_from(cell / (usize::from(self.width) * usize::from(self.depth))).ok()?,
            )?;
            let block = Block::from_u32(u32::from_le_bytes(raw.try_into().ok()?))?;
            chunk.set(Vec3::new(x as i32, y as i32, z), block).ok()?;
        }
        Some(chunk)
    }
}
