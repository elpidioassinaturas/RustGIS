//! LERC-compressed TIFF strips and tiles (Compression 34887), decoded here: the `tiff` crate has no
//! LERC codec (#1818).
//!
//! GDAL writes them (`COMPRESS=LERC`, `LERC_DEFLATE`, `LERC_ZSTD`), and so do the producers that use
//! it: LINZ's elevation models, ESA's BIOMASS products. A chunk holds one LERC blob (Esri's Limited
//! Error Raster Compression), possibly wrapped in Deflate or Zstandard: the `LercParameters` tag
//! (50674) gives the LERC version and that extra layer (0 none, 1 Deflate, 2 Zstandard). A blob of
//! a chunk with several samples per pixel holds them all, pixel by pixel, or one blob per band one
//! after another. Pixels LERC marks invalid come back as NaN in floating-point images and as the
//! image's no-data value (`GDAL_NODATA`), else 0, in integer ones, as GDAL reads them.

use std::io::{Read, Seek, SeekFrom};

use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

use crate::{DataError, Result};

/// The TIFF compression code of LERC.
pub(crate) const COMPRESSION: u16 = 34887;

/// Most bytes in a chunk, compressed or decoded (the decoder's own intermediate limit).
const MAX_CHUNK_BYTES: usize = 256 << 20;

fn err(e: impl std::fmt::Display) -> DataError {
    DataError::Format(format!("GeoTIFF: LERC: {e}"))
}

/// Whether the decoder's current image is LERC-compressed.
pub(crate) fn is_lerc<R: Read + Seek>(dec: &mut Decoder<R>) -> bool {
    dec.find_tag_unsigned::<u16>(Tag::Compression).ok().flatten() == Some(COMPRESSION)
}

/// Chunk `index` (a strip or tile, counting through every plane) of the decoder's current image,
/// cropped to the image as `Decoder::read_chunk` returns it.
pub(crate) fn chunk<R: Read + Seek>(dec: &mut Decoder<R>, index: u32) -> Result<DecodingResult> {
    let (w, h) = dec.dimensions().map_err(err)?;
    let tag = |dec: &mut Decoder<R>, t: Tag| dec.find_tag_unsigned::<u32>(t).ok().flatten();
    let tiled = tag(dec, Tag::TileWidth).is_some();
    let (cw, ch) = if tiled {
        (tag(dec, Tag::TileWidth).unwrap_or(0), tag(dec, Tag::TileLength).unwrap_or(0))
    } else {
        (w, tag(dec, Tag::RowsPerStrip).unwrap_or(h).min(h))
    };
    if cw == 0 || ch == 0 {
        return Err(err("a chunk has no size"));
    }
    let bits = dec.find_tag_unsigned_vec::<u16>(Tag::BitsPerSample).ok().flatten().unwrap_or_else(|| vec![8]);
    let bps = *bits.first().unwrap_or(&8);
    if bits.iter().any(|b| *b != bps) {
        return Err(err("bands of different sample sizes aren't supported"));
    }
    let format = dec.find_tag_unsigned_vec::<u16>(Tag::SampleFormat).ok().flatten().and_then(|v| v.first().copied()).unwrap_or(1);
    let spp = tag(dec, Tag::SamplesPerPixel).unwrap_or(1).clamp(1, u32::from(u16::MAX)) as usize;
    let planar = tag(dec, Tag::PlanarConfiguration) == Some(2);
    let samples = if planar { 1 } else { spp };
    let extra = dec.find_tag_unsigned_vec::<u32>(Tag::Unknown(50674)).ok().flatten().and_then(|v| v.get(1).copied()).unwrap_or(0);
    let nodata = dec
        .find_tag(Tag::Unknown(42113))
        .ok()
        .flatten()
        .and_then(|v| v.into_string().ok())
        .and_then(|s| s.trim_matches(char::from(0)).trim().parse::<f64>().ok());

    // Where the chunk is in its plane, and how much of it is image.
    let across = w.div_ceil(cw);
    let per_plane = across.saturating_mul(h.div_ceil(ch)).max(1);
    let c = index % per_plane;
    let (x0, y0) = ((c % across.max(1)) * cw, (c / across.max(1)) * ch);
    let dw = if tiled { cw.min(w.saturating_sub(x0)) } else { w } as usize;
    let dh = ch.min(h.saturating_sub(y0)) as usize;
    // A tile is coded whole, padding included; a strip only has the image's rows.
    let (bw, bh) = (cw as usize, if tiled { ch as usize } else { dh });

    let (offsets, counts) = if tiled { (Tag::TileOffsets, Tag::TileByteCounts) } else { (Tag::StripOffsets, Tag::StripByteCounts) };
    let at = |dec: &mut Decoder<R>, t: Tag| dec.find_tag_unsigned_vec::<u64>(t).ok().flatten().and_then(|v| v.get(index as usize).copied());
    let (Some(offset), Some(count)) = (at(dec, offsets), at(dec, counts)) else {
        return Err(err("no such chunk"));
    };
    let count = usize::try_from(count).ok().filter(|n| *n <= MAX_CHUNK_BYTES).ok_or_else(|| err("a chunk is too large"))?;
    // Read as far as the file goes, not allocated from the claim.
    let r = dec.inner();
    r.seek(SeekFrom::Start(offset)).map_err(err)?;
    let mut encoded = Vec::new();
    r.take(count as u64).read_to_end(&mut encoded).map_err(err)?;
    if encoded.len() < count {
        return Err(err("a chunk is cut short"));
    }
    let blob = unwrap_layer(encoded, extra)?;

    let (values, mask) = decode_blob(&blob, bw, bh, samples)?;
    let float = format == 3;
    let fill = if float { f64::NAN } else { nodata.unwrap_or(0.0) };
    let mut out = empty(format, bps, dw * dh * samples)?;
    for y in 0..dh {
        let row = y * bw;
        for x in 0..dw {
            let valid = mask.as_ref().is_none_or(|m| m.get(row + x).is_some_and(|v| *v != 0));
            for s in 0..samples {
                let i = (row + x) * samples + s;
                let v = if valid { values.get(i).copied() } else { None };
                push(&mut out, v, fill);
            }
        }
    }
    Ok(out)
}

/// The LERC blob inside a chunk: undoes the Deflate or Zstandard layer `extra` names.
fn unwrap_layer(encoded: Vec<u8>, extra: u32) -> Result<Vec<u8>> {
    match extra {
        0 => Ok(encoded),
        1 => miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&encoded, MAX_CHUNK_BYTES)
            .map_err(|e| err(format!("its Deflate layer: {e:?}"))),
        2 => {
            let mut dec =
                ruzstd::decoding::StreamingDecoder::new(encoded.as_slice()).map_err(|e| err(format!("its Zstandard layer: {e}")))?;
            let mut out = Vec::new();
            (&mut dec).take(MAX_CHUNK_BYTES as u64 + 1).read_to_end(&mut out).map_err(|e| err(format!("its Zstandard layer: {e}")))?;
            if out.len() > MAX_CHUNK_BYTES {
                return Err(err("a chunk is too large"));
            }
            Ok(out)
        }
        n => Err(err(format!("extra compression {n} isn't supported"))),
    }
}

/// The chunk's samples (pixel by pixel, every sample of a pixel together) and its validity mask
/// (one byte a pixel, 0 for invalid), checked against the chunk's `width × height × samples`. The
/// blob holds every sample of a pixel, or is one blob per sample after another.
fn decode_blob(blob: &[u8], width: usize, height: usize, samples: usize) -> Result<(Vec<f64>, Option<Vec<u8>>)> {
    let pixels = width.checked_mul(height).ok_or_else(|| err("a chunk is too large"))?;
    let mut parts: Vec<(usize, Vec<f64>, Option<Vec<u8>>)> = Vec::new();
    let mut rest = blob;
    let mut depth = 0usize;
    while depth < samples && !rest.is_empty() {
        let d = lerc_reader::decode_first_to_f64(rest).map_err(err)?;
        if (d.info.width as usize, d.info.height as usize) != (width, height) {
            return Err(err(format!("a blob is {} × {}, its chunk {width} × {height}", d.info.width, d.info.height)));
        }
        let n = d.info.depth.max(1) as usize;
        if d.pixels.len() != pixels.saturating_mul(n) {
            return Err(err("a blob holds fewer samples than it says"));
        }
        depth += n;
        rest = rest.get(d.info.blob_size.max(1)..).unwrap_or_default();
        parts.push((n, d.pixels, d.mask));
    }
    if depth != samples {
        return Err(err(format!("a chunk holds {depth} samples a pixel, its image {samples}")));
    }
    if parts.len() == 1 {
        let (_, values, mask) = parts.into_iter().next().ok_or_else(|| err("a chunk holds no blob"))?;
        return Ok((values, mask));
    }
    // One blob per sample (or a few): interleaved pixel by pixel.
    let mut values = Vec::with_capacity(pixels.saturating_mul(samples));
    for i in 0..pixels {
        for (n, v, _) in &parts {
            values.extend((0..*n).map(|k| v.get(i * n + k).copied().unwrap_or(f64::NAN)));
        }
    }
    // A pixel is valid when every blob says so.
    let masks: Vec<&Vec<u8>> = parts.iter().filter_map(|(_, _, m)| m.as_ref()).collect();
    let mask =
        (!masks.is_empty()).then(|| (0..pixels).map(|i| u8::from(masks.iter().all(|m| m.get(i).is_some_and(|v| *v != 0)))).collect());
    Ok((values, mask))
}

/// No samples yet, of the type `format` and `bps` name, with room for `n`.
fn empty(format: u16, bps: u16, n: usize) -> Result<DecodingResult> {
    Ok(match (format, bps) {
        (3, 32) => DecodingResult::F32(Vec::with_capacity(n)),
        (3, 64) => DecodingResult::F64(Vec::with_capacity(n)),
        (2, 8) => DecodingResult::I8(Vec::with_capacity(n)),
        (2, 16) => DecodingResult::I16(Vec::with_capacity(n)),
        (2, 32) => DecodingResult::I32(Vec::with_capacity(n)),
        (1 | 4, 8) => DecodingResult::U8(Vec::with_capacity(n)),
        (1 | 4, 16) => DecodingResult::U16(Vec::with_capacity(n)),
        (1 | 4, 32) => DecodingResult::U32(Vec::with_capacity(n)),
        (f, b) => return Err(err(format!("{b}-bit samples of format {f} aren't supported"))),
    })
}

/// Adds one sample (`fill` for an invalid or missing one) to `out`, in its type. The values come
/// from a blob of the image's own type, so the casts keep them.
fn push(out: &mut DecodingResult, v: Option<f64>, fill: f64) {
    let x = v.unwrap_or(fill);
    match out {
        DecodingResult::F32(o) => o.push(x as f32),
        DecodingResult::F64(o) => o.push(x),
        DecodingResult::I8(o) => o.push(x as i8),
        DecodingResult::I16(o) => o.push(x as i16),
        DecodingResult::I32(o) => o.push(x as i32),
        DecodingResult::U8(o) => o.push(x as u8),
        DecodingResult::U16(o) => o.push(x as u16),
        DecodingResult::U32(o) => o.push(x as u32),
        // Never made by `empty`.
        DecodingResult::F16(_) | DecodingResult::I64(_) | DecodingResult::U64(_) => {}
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    //! The files in `tests/data/lerc` were written by GDAL 3.9 (`make_lerc.py` there); their
    //! values follow the formulas computed again here.

    use std::io::Cursor;

    use super::super::{Raster, geotiff};

    fn open(name: &str) -> Raster {
        let path = format!("{}/tests/data/lerc/{name}", env!("CARGO_MANIFEST_DIR"));
        geotiff::read(Cursor::new(std::fs::read(&path).unwrap()), name).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    #[test]
    fn floats_in_tiles_with_a_hole() {
        let r = open("lerc_f32_tiles.tif");
        assert_eq!((r.width, r.height, r.bands.len()), (100, 70, 1));
        for y in 0..70i64 {
            for x in 0..100i64 {
                let v = r.value(0, x, y);
                if (5..9).contains(&y) && (10..20).contains(&x) {
                    assert_eq!(v, None, "({x}, {y}) is in LERC's mask");
                } else {
                    assert_eq!(v, Some(((x * 7 + y * 13) % 1000) as f32 / 8.0), "({x}, {y}), edge tiles included");
                }
            }
        }
    }

    #[test]
    fn three_bands_in_deflate() {
        let r = open("lerc_deflate_u16_3bands.tif");
        assert_eq!((r.width, r.height, r.bands.len()), (70, 50, 3));
        for b in 0..3 {
            for y in 0..50i64 {
                for x in 0..70i64 {
                    assert_eq!(r.value(b, x, y), Some(((x * 3 + y * 5 + b as i64 * 100) % 60000) as f32), "band {b} at ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn planes_in_strips_in_zstandard() {
        let r = open("lerc_zstd_i16_planar_strips.tif");
        assert_eq!((r.width, r.height, r.bands.len()), (45, 30, 2));
        for b in 0..2 {
            for y in 0..30i64 {
                for x in 0..45i64 {
                    assert_eq!(r.value(b, x, y), Some((x - y * 2 + b as i64 * 50) as f32), "band {b} at ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn doubles_in_zstandard() {
        let r = open("lerc_zstd_f64.tif");
        assert_eq!((r.width, r.height), (40, 30));
        for y in 0..30i64 {
            for x in 0..40i64 {
                assert_eq!(r.value(0, x, y), Some((x as f64 * 0.25 - y as f64 * 1.5) as f32), "({x}, {y})");
            }
        }
    }

    #[test]
    fn a_damaged_chunk_is_an_error() {
        let path = format!("{}/tests/data/lerc/lerc_zstd_f64.tif", env!("CARGO_MANIFEST_DIR"));
        let good = std::fs::read(path).unwrap();
        // Every byte past the header flipped in turn (in steps, to stay quick): no panic, and a
        // file that still reads keeps its size.
        for at in (16..good.len()).step_by(7) {
            let mut bad = good.clone();
            if let Some(b) = bad.get_mut(at) {
                *b ^= 0x5a;
            }
            if let Ok(r) = geotiff::read(Cursor::new(bad), "bad") {
                assert_eq!((r.width, r.height), (40, 30));
            }
        }
        // A file cut short in its tiles.
        assert!(geotiff::read(Cursor::new(good.get(..good.len() - 200).unwrap().to_vec()), "cut").is_err());
    }
}
