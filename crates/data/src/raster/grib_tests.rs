use super::*;

/// `values` packed in `bits` bits each, most significant first.
fn pack(values: &[u32], bits: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut acc, mut n) = (0u64, 0u32);
    for &v in values {
        acc = (acc << bits) | u64::from(v);
        n += bits;
        while n >= 8 {
            out.push((acc >> (n - 8)) as u8);
            n -= 8;
        }
    }
    if n > 0 {
        out.push((acc << (8 - n)) as u8);
    }
    out
}

/// A sign bit and magnitude in `n` octets.
fn sm(v: i64, n: usize) -> Vec<u8> {
    let m = v.unsigned_abs() | if v < 0 { 1 << (8 * n - 1) } else { 0 };
    (0..n).rev().map(|i| (m >> (8 * i)) as u8).collect()
}

fn u(v: u64, n: usize) -> Vec<u8> {
    (0..n).rev().map(|i| (v >> (8 * i)) as u8).collect()
}

/// The bits of a bitmap keeping the points `keep` says.
fn bitmap(keep: &[bool]) -> Vec<u8> {
    pack(&keep.iter().map(|&k| u32::from(k)).collect::<Vec<_>>(), 1)
}

struct G1 {
    ni: usize,
    nj: usize,
    /// Millidegrees.
    lat1: i64,
    lon1: i64,
    lat2: i64,
    lon2: i64,
    di: u64,
    dj: u64,
    scan: u8,
    /// IBM reference value, E, D, bits.
    r: u32,
    e: i64,
    d: i64,
    bits: u32,
    /// Packed values, and the bitmap when some points are masked.
    x: Vec<u32>,
    keep: Option<Vec<bool>>,
    /// BDS flag bits (0x40: second-order packing).
    bds_flags: u8,
    param: u8,
}

impl G1 {
    fn new(ni: usize, nj: usize, x: Vec<u32>) -> G1 {
        // 1° cells from 60° N, 10° E, north to south.
        G1 {
            ni,
            nj,
            lat1: 60_000,
            lon1: 10_000,
            lat2: 60_000 - (nj as i64 - 1) * 1000,
            lon2: 10_000 + (ni as i64 - 1) * 1000,
            di: 1000,
            dj: 1000,
            scan: 0,
            r: 0,
            e: 0,
            d: 0,
            bits: 16,
            x,
            keep: None,
            bds_flags: 0,
            param: 11,
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut pds = u(28, 3);
        pds.extend([2, 7, 96, 255, 0x80 | if self.keep.is_some() { 0x40 } else { 0 }, self.param, 105]);
        pds.extend(u(2, 2));
        pds.extend([26, 11, 18, 6, 0, 1, 0, 0, 0]);
        pds.extend(u(0, 2));
        pds.extend([0, 21, 0]);
        pds.extend(sm(self.d, 2));
        let mut gds = u(32, 3);
        gds.extend([0, 255, 0]);
        gds.extend(u(self.ni as u64, 2));
        gds.extend(u(self.nj as u64, 2));
        gds.extend(sm(self.lat1, 3));
        gds.extend(sm(self.lon1, 3));
        gds.push(0x80);
        gds.extend(sm(self.lat2, 3));
        gds.extend(sm(self.lon2, 3));
        gds.extend(u(self.di, 2));
        gds.extend(u(self.dj, 2));
        gds.push(self.scan);
        gds.extend([0, 0, 0, 0]);
        let mut body = pds;
        body.extend(gds);
        if let Some(keep) = &self.keep {
            let bits = bitmap(keep);
            let mut bms = u(6 + bits.len() as u64, 3);
            bms.extend([0, 0, 0]);
            bms.extend(bits);
            body.extend(bms);
        }
        let data = pack(&self.x, self.bits);
        let mut bds = u(11 + data.len() as u64, 3);
        bds.push(self.bds_flags);
        bds.extend(sm(self.e, 2));
        bds.extend(u(u64::from(self.r), 4));
        bds.push(self.bits as u8);
        bds.extend(data);
        body.extend(bds);
        let mut out = b"GRIB".to_vec();
        out.extend(u(8 + body.len() as u64 + 4, 3));
        out.push(1);
        out.extend(body);
        out.extend(b"7777");
        out
    }
}

/// A GRIB2 field: its grid, product and data sections.
struct F2 {
    ni: usize,
    nj: usize,
    /// Micro-degrees.
    lat1: i64,
    lon1: i64,
    lat2: i64,
    lon2: i64,
    di: u64,
    dj: u64,
    scan: u8,
    template5: u16,
    r: f32,
    e: i64,
    d: i64,
    bits: u32,
    x: Vec<u32>,
    keep: Option<Vec<bool>>,
    category: u8,
    number: u8,
    /// The first fixed surface's scale factor and scaled value.
    surface_scale: u8,
    surface_value: u64,
}

impl F2 {
    fn new(ni: usize, nj: usize, x: Vec<u32>) -> F2 {
        // 0.5° cells from 10° S, 300° E, south to north.
        F2 {
            ni,
            nj,
            lat1: -10_000_000,
            lon1: 300_000_000,
            lat2: -10_000_000 + (nj as i64 - 1) * 500_000,
            lon2: 300_000_000 + (ni as i64 - 1) * 500_000,
            di: 500_000,
            dj: 500_000,
            scan: 0x40,
            template5: 0,
            r: 0.0,
            e: 0,
            d: 0,
            bits: 12,
            x,
            keep: None,
            category: 0,
            number: 0,
            surface_scale: 0,
            surface_value: 2,
        }
    }

    fn sections(&self) -> Vec<u8> {
        let mut s3 = u(72, 4);
        s3.extend([3, 0]);
        s3.extend(u((self.ni * self.nj) as u64, 4));
        s3.extend([0, 0]);
        s3.extend(u(0, 2));
        s3.push(6);
        s3.extend([0; 15]);
        s3.extend(u(self.ni as u64, 4));
        s3.extend(u(self.nj as u64, 4));
        s3.extend(u(0, 4));
        s3.extend(u(u64::from(u32::MAX), 4));
        s3.extend(sm(self.lat1, 4));
        s3.extend(sm(self.lon1, 4));
        s3.push(0x30);
        s3.extend(sm(self.lat2, 4));
        s3.extend(sm(self.lon2, 4));
        s3.extend(u(self.di, 4));
        s3.extend(u(self.dj, 4));
        s3.push(self.scan);
        assert_eq!(s3.len(), 72);
        let mut s4 = u(34, 4);
        s4.push(4);
        s4.extend(u(0, 2));
        s4.extend(u(0, 2));
        s4.extend([self.category, self.number, 2, 0, 96]);
        s4.extend(u(0, 2));
        s4.extend([0, 1]);
        s4.extend(u(6, 4));
        s4.extend([103, self.surface_scale]);
        s4.extend(u(self.surface_value, 4));
        s4.extend([255, 0]);
        s4.extend(u(0, 4));
        assert_eq!(s4.len(), 34);
        let count = self.keep.as_ref().map_or(self.x.len(), |k| k.iter().filter(|b| **b).count());
        let mut s5 = u(21, 4);
        s5.push(5);
        s5.extend(u(count as u64, 4));
        s5.extend(u(u64::from(self.template5), 2));
        s5.extend(u(u64::from(self.r.to_bits()), 4));
        s5.extend(sm(self.e, 2));
        s5.extend(sm(self.d, 2));
        s5.extend([self.bits as u8, 0]);
        let mut s6 = match &self.keep {
            None => {
                let mut s6 = u(6, 4);
                s6.extend([6, 255]);
                s6
            }
            Some(keep) => {
                let bits = bitmap(keep);
                let mut s6 = u(6 + bits.len() as u64, 4);
                s6.extend([6, 0]);
                s6.extend(bits);
                s6
            }
        };
        let data = pack(&self.x, self.bits);
        let mut s7 = u(5 + data.len() as u64, 4);
        s7.push(7);
        s7.extend(data);
        let mut out = s3;
        out.append(&mut s4);
        out.append(&mut s5);
        out.append(&mut s6);
        out.append(&mut s7);
        out
    }
}

/// A GRIB2 message of `fields` (meteorological products, discipline 0).
fn grib2_message(fields: &[F2]) -> Vec<u8> {
    let mut s1 = u(21, 4);
    s1.extend([1, 0, 7, 0, 0, 2, 1, 1]);
    s1.extend(u(2026, 2));
    s1.extend([11, 18, 0, 0, 0, 0, 1]);
    let mut body = s1;
    for f in fields {
        body.extend(f.sections());
    }
    let mut out = b"GRIB".to_vec();
    out.extend([0, 0, 0, 2]);
    out.extend(u(16 + body.len() as u64 + 4, 8));
    out.extend(body);
    out.extend(b"7777");
    out
}

fn approx(a: f32, b: f64) -> bool {
    (f64::from(a) - b).abs() < 1e-4
}

#[test]
fn ibm_reference_values() {
    assert_eq!(ibm(0), 0.0);
    assert_eq!(ibm(0x4264_0000), 100.0);
    assert_eq!(ibm(0xc110_0000), -1.0);
    // BAM's land-sea mask (an actual record): R = -4.5555553...
    assert!((ibm(0xc148_e38e) + 4.555_555_3).abs() < 1e-6);
}

#[test]
fn grib1_simple_packing_north_to_south() {
    let mut g = G1::new(3, 2, vec![0, 1, 2, 30, 40, 50]);
    g.r = 0x4264_0000; // 100
    g.d = 1;
    let r = read_bytes(&g.bytes(), "t").unwrap();
    assert_eq!((r.width, r.height, r.bands.len()), (3, 2, 1));
    let v = &r.bands[0].data;
    for (got, want) in v.iter().zip([10.0, 10.1, 10.2, 13.0, 14.0, 15.0]) {
        assert!(approx(*got, want), "{v:?}");
    }
    let t = r.transform;
    assert_eq!((t.origin_x, t.origin_y, t.pixel_width, t.pixel_height), (9.5, 60.5, 1.0, -1.0));
    assert_eq!(r.bands[0].name, "d=2026111806:kpds5=11:kpds6=105:kpds7=2:TR=0:P1=0:P2=0:TimeU=1");
    // GRIB1's earth is a sphere: said once.
    assert_eq!(r.lost, vec!["its earth is a sphere of radius 6,367,470 m (GRIB1's); latitudes and longitudes are read as WGS 84's"]);
    // 10.1 and 10.2 change in `f32`; 10, 13, 14 and 15 don't.
    assert_eq!(r.rounded, vec![2]);
}

#[test]
fn grib1_bitmap_masks_points() {
    let mut g = G1::new(2, 2, vec![5, 7]);
    g.keep = Some(vec![true, false, false, true]);
    let r = read_bytes(&g.bytes(), "t").unwrap();
    let v = &r.bands[0].data;
    assert_eq!(v[0], 5.0);
    assert!(v[1].is_nan() && v[2].is_nan());
    assert_eq!(v[3], 7.0);
}

#[test]
fn grib1_constant_field() {
    let mut g = G1::new(2, 1, vec![]);
    g.bits = 0;
    g.r = 0x4264_0000;
    let r = read_bytes(&g.bytes(), "t").unwrap();
    assert_eq!(r.bands[0].data, vec![100.0, 100.0]);
}

#[test]
fn bam_latitudes_against_the_scanning_mode() {
    // CPTEC's BAM: the first point at the south pole's row, scanning north to south, round the
    // globe from 0°. Rows follow the scanning mode; columns run from −180°.
    // (GRIB1 increments are two octets of millidegrees: 60° cells.)
    let mut g = G1::new(6, 2, (1..=12).collect());
    (g.lat1, g.lat2, g.lon1, g.lon2, g.di, g.dj) = (-30_000, 30_000, 0, 300_000, 60_000, 60_000);
    let r = read_bytes(&g.bytes(), "t").unwrap();
    // The first row is the northern one; the 180° column comes first.
    assert_eq!(r.bands[0].data, vec![4.0, 5.0, 6.0, 1.0, 2.0, 3.0, 10.0, 11.0, 12.0, 7.0, 8.0, 9.0]);
    let t = r.transform;
    assert_eq!((t.origin_x, t.origin_y), (-210.0, 60.0));
    assert!(
        r.lost.contains(&"the grid gives its first row at -30° and its last at 30° but scans north to south: its 2 rows of 60° are laid out as the scanning mode says, centred between the two, from 30° to -30°".to_string()),
        "{:?}",
        r.lost
    );
    // BAM's own header: −90° and 90°, the grid's edges, for 1000 rows of 0.18° whose centres
    // its GrADS descriptor puts at ±89.91°.
    let mut g = G1::new(2, 1000, vec![0; 2000]);
    (g.lat1, g.lat2, g.lon1, g.lon2, g.di, g.dj, g.bits) = (-90_000, 90_000, 0, 180, 180, 180, 1);
    let r = read_bytes(&g.bytes(), "t").unwrap();
    assert!((r.transform.origin_y - 90.0).abs() < 1e-9, "{:?}", r.transform);
    assert!(r.lost[0].ends_with("from 89.91° to -89.91°"), "{:?}", r.lost);
}

#[test]
fn grib1_second_order_packing_is_refused_by_name() {
    let mut g = G1::new(2, 1, vec![1, 2]);
    g.bds_flags = 0x40;
    let e = read_bytes(&g.bytes(), "t").unwrap_err().to_string();
    assert!(e.contains("nothing in it can be read here: 1 field left out: GRIB1 second-order (complex) packing, not read yet"), "{e}");
}

#[test]
fn grib2_fields_south_to_north_with_a_bitmap() {
    let a = F2::new(2, 2, vec![1, 2, 3, 4]);
    let mut b = F2::new(2, 2, vec![9, 8]);
    (b.r, b.e, b.d, b.category, b.number) = (273.15, 1, 1, 0, 0);
    b.keep = Some(vec![false, true, true, false]);
    let r = read_bytes(&grib2_message(&[a, b]), "t").unwrap();
    assert_eq!(r.bands.len(), 2);
    // Rows flipped north up: the second row scanned is the first.
    assert_eq!(r.bands[0].data, vec![3.0, 4.0, 1.0, 2.0]);
    let v = &r.bands[1].data;
    assert!(v[1].is_nan() && v[2].is_nan());
    // (R + X × 2^E) / 10^D: the point scanned second (east, south) gets 9, the third (west,
    // north) 8.
    assert!(approx(v[3], (273.15f32 as f64 + 9.0 * 2.0) / 10.0) && approx(v[0], (273.15f32 as f64 + 8.0 * 2.0) / 10.0), "{v:?}");
    // From 300° E: moved west of the antimeridian, half a cell west and north of the corners.
    let t = r.transform;
    assert_eq!((t.origin_x, t.origin_y, t.pixel_width, t.pixel_height), (-60.25, -9.25, 0.5, -0.5));
    assert_eq!(r.bands[0].name, "d=2026111800:discipline=0:category=0:number=0:template=4.0:surface=103:value=2:TimeU=1:forecast=6");
    assert!(r.lost.iter().any(|l| l.contains("6,371,229")), "{:?}", r.lost);
}

#[test]
fn grib2_other_packings_are_left_out_and_said() {
    let a = F2::new(2, 1, vec![1, 2]);
    let mut b = F2::new(2, 1, vec![1, 2]);
    b.template5 = 3;
    let r = read_bytes(&grib2_message(&[a, b]), "t").unwrap();
    assert_eq!(r.bands.len(), 1);
    assert!(r.lost.contains(&"1 field left out: GRIB2 complex packing (template 5.3), not read yet".to_string()), "{:?}", r.lost);
    // A file with nothing readable is an error naming why.
    let mut c = F2::new(2, 1, vec![1, 2]);
    c.template5 = 40;
    // The surface's value is scaled: 25 at scale factor 1 is 2.5.
    let mut d = F2::new(2, 1, vec![1, 2]);
    (d.surface_scale, d.surface_value) = (1, 25);
    let r = read_bytes(&grib2_message(&[d]), "t").unwrap();
    assert!(r.bands[0].name.contains(":surface=103:value=2.5:"), "{}", r.bands[0].name);
    let e = read_bytes(&grib2_message(&[c]), "t").unwrap_err().to_string();
    assert!(e.contains("JPEG 2000 (template 5.40)"), "{e}");
}

#[test]
fn fields_on_another_grid_are_left_out_and_said() {
    let mut file = G1::new(2, 2, vec![1, 2, 3, 4]).bytes();
    // Padding between messages, as bulletins have.
    file.extend([0u8; 10]);
    file.extend(G1::new(3, 2, vec![1, 2, 3, 4, 5, 6]).bytes());
    file.extend(G1::new(2, 2, vec![5, 6, 7, 8]).bytes());
    let r = read_bytes(&file, "t").unwrap();
    assert_eq!(r.bands.len(), 2);
    assert_eq!(r.bands[1].data, vec![5.0, 6.0, 7.0, 8.0]);
    assert!(r.lost.contains(&"1 field left out: on another grid than the first field's".to_string()), "{:?}", r.lost);
}

#[test]
fn a_cut_message_is_said_or_refused() {
    let one = G1::new(2, 2, vec![1, 2, 3, 4]).bytes();
    let mut file = one.clone();
    file.extend(&one[..one.len() - 5]);
    let r = read_bytes(&file, "t").unwrap();
    assert_eq!(r.bands.len(), 1);
    assert!(r.lost.contains(&"the file ends inside a message, which is left out".to_string()), "{:?}", r.lost);
    let e = read_bytes(&one[..one.len() - 1], "t").unwrap_err().to_string();
    assert!(e.contains("cut short"), "{e}");
    let e = read_bytes(b"not a grib file at all", "t").unwrap_err().to_string();
    assert!(e.contains("no GRIB message"), "{e}");
}

#[test]
fn hostile_bytes_never_panic() {
    let mut file = G1::new(3, 2, vec![0, 1, 2, 3, 4, 5]).bytes();
    file.extend(grib2_message(&[F2::new(2, 2, vec![1, 2, 3, 4])]));
    // Every prefix, and every octet flipped in turn.
    for n in 0..file.len() {
        let _ = read_bytes(&file[..n], "t");
    }
    for i in 0..file.len() {
        for x in [0x00, 0xff, 0x80, 0x7f] {
            let mut bad = file.clone();
            bad[i] = x;
            let _ = read_bytes(&bad, "t");
        }
    }
}

#[test]
fn grib_names_are_rasters() {
    for n in ["a.grb", "A.GRIB", "b.grb2", "c.grib2"] {
        assert!(is_grib_name(n) && crate::raster::is_raster_name(n), "{n}");
    }
    assert!(!is_grib_name("a.grd"));
    let r = crate::raster::read_bytes("run.grb2", &grib2_message(&[F2::new(2, 1, vec![1, 2])])).unwrap();
    assert_eq!(r.name, "run");
}

#[test]
fn another_edition_is_said_and_the_next_message_read() {
    let mut other = G1::new(2, 1, vec![1, 2]).bytes();
    other[7] = 3;
    let mut file = other;
    file.extend(G1::new(2, 1, vec![3, 4]).bytes());
    let r = read_bytes(&file, "t").unwrap();
    assert_eq!(r.bands.len(), 1);
    assert_eq!(r.bands[0].data, vec![3.0, 4.0]);
    assert!(r.lost.contains(&"1 field left out: GRIB edition 3, not read yet".to_string()), "{:?}", r.lost);
    assert!(!r.lost.iter().any(|l| l.contains("ends inside")), "{:?}", r.lost);
}

#[test]
fn a_long_gap_between_messages_is_passed_and_said() {
    let mut file = G1::new(2, 1, vec![1, 2]).bytes();
    // About 70 KiB of something else, with "GRI" split over a chunk's end on the way.
    file.extend(std::iter::repeat_n(b'x', 70 * 1024));
    file.extend(b"GRI");
    file.extend(G1::new(2, 1, vec![3, 4]).bytes());
    let r = read_bytes(&file, "t").unwrap();
    assert_eq!(r.bands.len(), 2);
    assert_eq!(r.bands[1].data, vec![3.0, 4.0]);
    assert!(r.lost.contains(&format!("{} bytes outside its messages aren't GRIB, and are passed over", 70 * 1024 + 3)), "{:?}", r.lost);
    // A "GRIB" split over two chunks is found.
    for pad in 4090..4100 {
        let mut file = vec![0u8; pad];
        file.extend(G1::new(2, 1, vec![1, 2]).bytes());
        assert_eq!(read_bytes(&file, "t").unwrap().bands.len(), 1, "{pad}");
    }
}

#[test]
fn ensemble_members_are_named_apart() {
    let mut s = F2::new(2, 1, vec![1, 2]).sections();
    // Section 4 is at 72 (after section 3): template 4.1, with its ensemble octets.
    let at = 72;
    s[at + 8] = 1;
    let mut s4_tail = vec![3u8, 7, 20];
    // Grow section 4 from 34 to 37 octets.
    s.splice(at + 34..at + 34, s4_tail.drain(..));
    s[at + 3] = 37;
    let mut s1 = u(21, 4);
    s1.extend([1, 0, 7, 0, 0, 2, 1, 1]);
    s1.extend(u(2026, 2));
    s1.extend([11, 18, 0, 0, 0, 0, 1]);
    let mut body = s1;
    body.extend(s);
    let mut msg = b"GRIB".to_vec();
    msg.extend([0, 0, 0, 2]);
    msg.extend(u(16 + body.len() as u64 + 4, 8));
    msg.extend(body);
    msg.extend(b"7777");
    let r = read_bytes(&msg, "t").unwrap();
    assert!(r.bands[0].name.ends_with(":template=4.1:surface=103:value=2:TimeU=1:forecast=6:ensemble=3:member=7"), "{}", r.bands[0].name);
}
