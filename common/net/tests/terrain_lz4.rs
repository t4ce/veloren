use common::{
    terrain::{Block, BlockKind, SpriteKind, TerrainChunk, TerrainChunkMeta},
    vol::{ReadVol, WriteVol},
};
use vek::{Rgb, Vec2, Vec3};
use veloren_common_net::msg::{
    SerializedTerrainChunk, ServerGeneral,
    terrain_lz4::{Lz4TerrainChunkV1, TERRAIN_LZ4_MAX_HEIGHT},
};

fn fixture() -> TerrainChunk {
    let below = Block::new(BlockKind::Rock, Rgb::new(0, 0, 0));
    let above = Block::air(SpriteKind::Empty);
    let mut meta = TerrainChunkMeta::void();
    meta.add_debug_point(Vec3::new(1.25, -2.5, 7.0));
    let mut chunk = TerrainChunk::new(-32, below, above, meta);
    for z in -32i32..16 {
        for y in 0i32..32 {
            for x in 0i32..32 {
                let block = if (x + y + z).rem_euclid(11) == 0 {
                    Block::air(SpriteKind::Chest)
                        .with_ori(((x + y) % 8) as u8)
                        .unwrap()
                } else if (x + z).rem_euclid(7) == 0 {
                    Block::unfilled(BlockKind::Water, SpriteKind::Empty)
                } else {
                    Block::from_raw(BlockKind::Grass, [x as u8 * 7, y as u8 * 5, z as u8])
                };
                chunk.set(Vec3::new(x, y, z), block).unwrap();
            }
        }
    }
    chunk
}

fn assert_same(a: &TerrainChunk, b: &TerrainChunk) {
    assert_eq!(a.get_min_z(), b.get_min_z());
    assert_eq!(a.get_max_z(), b.get_max_z());
    let encode_meta = |c: &TerrainChunk| {
        bincode::serde::encode_to_vec(c.meta(), bincode::config::legacy()).unwrap()
    };
    assert_eq!(encode_meta(a), encode_meta(b));
    for z in a.get_min_z() - 1..=a.get_max_z() {
        for y in 0i32..32 {
            for x in 0i32..32 {
                let p = Vec3::new(x, y, z);
                assert_eq!(
                    a.get(p).unwrap().to_u32(),
                    b.get(p).unwrap().to_u32(),
                    "{p:?}"
                );
            }
        }
    }
}

#[test]
fn exact_round_trip_preserves_kind_color_sprites_metadata_bounds_and_defaults() {
    let chunk = fixture();
    let wire = Lz4TerrainChunkV1::from_chunk(&chunk, 123).unwrap();
    let dense = wire.decode().unwrap();
    assert_eq!(dense.revision, 123);
    assert_eq!(dense.voxels.len(), 32 * 32 * 48 * 4);
    // x fastest, then y, then z; suitable for the resident voxel upload.
    assert_eq!(
        &dense.voxels[4..8],
        &chunk
            .get(Vec3::new(1, 0, -32))
            .unwrap()
            .to_u32()
            .to_le_bytes()
    );
    assert_same(&chunk, &dense.into_chunk().unwrap());
    assert_same(&chunk, &wire.to_chunk().unwrap());
}

#[test]
fn empty_and_all_default_upper_tail_round_trip_with_exact_bounds() {
    let air = Block::empty();
    let empty = TerrainChunk::new(-16, air, air, TerrainChunkMeta::void());
    assert_same(
        &empty,
        &Lz4TerrainChunkV1::from_chunk(&empty, 0)
            .unwrap()
            .to_chunk()
            .unwrap(),
    );
    let mut tail = empty.clone();
    let last = Vec3::new(0, 0, 31);
    tail.set(last, Block::new(BlockKind::Rock, Rgb::new(0, 0, 0)))
        .unwrap();
    tail.set(last, air).unwrap();
    assert_eq!(tail.get_max_z(), 32);
    assert_same(
        &tail,
        &Lz4TerrainChunkV1::from_chunk(&tail, 0)
            .unwrap()
            .to_chunk()
            .unwrap(),
    );
}

#[test]
fn protocol_variant_is_append_only_and_preferred_format_is_lossless() {
    let chunk = fixture();
    let message = SerializedTerrainChunk::lossless_lz4(&chunk, 99);
    assert!(matches!(message, SerializedTerrainChunk::Lz4VoxelsV1(_)));
    let bytes = bincode::serde::encode_to_vec(&message, bincode::config::legacy()).unwrap();
    assert_eq!(&bytes[..4], &3u32.to_le_bytes());
    let (decoded, consumed): (SerializedTerrainChunk, _) =
        bincode::serde::decode_from_slice(&bytes, bincode::config::legacy()).unwrap();
    assert_eq!(consumed, bytes.len());
    assert_same(&chunk, &decoded.to_chunk().unwrap());
    assert!(message.approx_len() < 32 * 32 * 48 * 4);
    let legacy = SerializedTerrainChunk::deflate(&chunk);
    assert_same(&chunk, &legacy.to_chunk().unwrap());
}

#[test]
fn malformed_version_dimensions_sizes_and_lz4_are_rejected() {
    let wire = Lz4TerrainChunkV1::from_chunk(&fixture(), 0).unwrap();
    for which in 0..9 {
        let mut bad = wire.clone();
        match which {
            0 => bad.version += 1,
            1 => bad.width = 0,
            2 => bad.depth = 31,
            3 => bad.zmax = bad.zmin - 1,
            4 => bad.zmax = bad.zmin + TERRAIN_LZ4_MAX_HEIGHT as i32 + 16,
            5 => bad.zmax += 1,
            6 => bad.uncompressed_len += 4,
            7 => bad.data.truncate(bad.data.len() / 2),
            8 => bad.data = lz4_flex::block::compress(&[0; 4]),
            _ => unreachable!(),
        }
        assert!(bad.decode().is_none(), "invalid case {which}");
    }
    let mut bad = wire.clone();
    bad.zmin = i32::MIN;
    bad.zmax = i32::MAX;
    assert!(bad.decode().is_none());
    let mut bad = wire.clone();
    bad.data.push(0); // appended bytes cannot change the advertised output
    assert!(bad.decode().is_none());
}

#[test]
fn invalid_block_kinds_and_payloads_are_rejected_before_chunk_insertion() {
    let mut wire = Lz4TerrainChunkV1::from_chunk(&fixture(), 0).unwrap();
    let invalid = (0..=255)
        .find(|kind| Block::from_u32(u32::from_le_bytes([*kind, 0, 0, 0])).is_none())
        .unwrap();
    let mut raw = wire.decode().unwrap().voxels;
    raw[0] = invalid;
    wire.data = lz4_flex::block::compress(&raw);
    assert!(wire.decode().is_none());
    let invalid_sprite = [BlockKind::Air as u8, 255, 255, 255];
    assert!(Block::from_u32(u32::from_le_bytes(invalid_sprite)).is_none());
    raw[..4].copy_from_slice(&invalid_sprite);
    wire.data = lz4_flex::block::compress(&raw);
    assert!(wire.decode().is_none());
    wire.below = [invalid, 0, 0, 0];
    assert!(wire.decode().is_none());
}

#[test]
fn excessive_vertical_span_falls_back_to_existing_lossless_codec() {
    let air = Block::empty();
    let mut chunk = TerrainChunk::new(0, air, air, TerrainChunkMeta::void());
    chunk
        .set(
            Vec3::new(0, 0, TERRAIN_LZ4_MAX_HEIGHT as i32),
            Block::new(BlockKind::Rock, Rgb::new(0, 0, 0)),
        )
        .unwrap();
    assert!(Lz4TerrainChunkV1::from_chunk(&chunk, 0).is_none());
    let message = SerializedTerrainChunk::lossless_lz4(&chunk, 0);
    assert!(matches!(message, SerializedTerrainChunk::DeflatedChonk(_)));
    let decoded = message.to_chunk().unwrap();
    assert_eq!(
        decoded
            .get(Vec3::new(0, 0, TERRAIN_LZ4_MAX_HEIGHT as i32))
            .unwrap(),
        chunk
            .get(Vec3::new(0, 0, TERRAIN_LZ4_MAX_HEIGHT as i32))
            .unwrap()
    );
}

// Run once per endpoint with TERRAIN_WIRE_WRITE, then consume the other
// endpoint's message with TERRAIN_WIRE_READ. Uses production enum/metadata ABI.
#[test]
fn cross_endpoint_serialized_fixture() {
    let chunk = fixture();
    let config = bincode::config::legacy();
    if let Ok(path) = std::env::var("TERRAIN_WIRE_WRITE") {
        let message = ServerGeneral::TerrainChunkUpdate {
            key: Vec2::new(-7, 11),
            chunk: Ok(SerializedTerrainChunk::lossless_lz4(&chunk, 9876)),
        };
        std::fs::write(
            path,
            bincode::serde::encode_to_vec(&message, config).unwrap(),
        )
        .unwrap();
    }
    if let Ok(path) = std::env::var("TERRAIN_WIRE_READ") {
        let bytes = std::fs::read(path).unwrap();
        let (message, used): (ServerGeneral, _) =
            bincode::serde::decode_from_slice(&bytes, config).unwrap();
        assert_eq!(used, bytes.len());
        let ServerGeneral::TerrainChunkUpdate {
            key,
            chunk: Ok(SerializedTerrainChunk::Lz4VoxelsV1(wire)),
        } = message
        else {
            panic!("expected LZ4 voxels")
        };
        assert_eq!(key, Vec2::new(-7, 11));
        assert_eq!(wire.revision, 9876);
        assert_same(&chunk, &wire.to_chunk().unwrap());
    }
}
