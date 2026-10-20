//! Times map renders (for the GPU, building its scene: the draw itself isn't timed) of a vector tile style (OpenFreeMap Liberty) while panning and zooming:
//! `cargo run --release -p rustgis-render --example vector_tile_bench`. Needs the network.
use rustgis_geom::{Coord, lonlat_to_merc};
use rustgis_map::{Camera, Map, TileKind, TileLayer};
use rustgis_render::{Backend, View, render_backend, rustgis_tiles};

fn main() {
    let mut m = Map::new("m");
    m.graticule = false;
    m.basemap = rustgis_map::Basemap::Blank;
    m.tiles.push(TileLayer::new("Liberty", TileKind::Style, "https://tiles.openfreemap.org/styles/liberty"));
    let view = |lon: f64, res: f64| View {
        width: 1400.0,
        height: 900.0,
        pixels_per_point: 2.0,
        camera: Camera { center: lonlat_to_merc(Coord::new(lon, 40.75)), res, bearing: 0.0, pitch: 0.0 },
        sky: false,
    };
    for (name, res) in [("city z12", 38.0), ("city z14", 9.5), ("city z15.5", 3.4)] {
        // Each zoom a little further in draws new pictures; panning draws a few at the edge.
        let zooms: Vec<View> = (0..6).map(|i| view(-73.98, res * (1.0 - 0.02 * f64::from(i)))).collect();
        let pans: Vec<View> = (1..=20).map(|i| view(-73.98 + f64::from(i) * res * 2e-5, res)).collect();
        // Every tile fetched first.
        for _ in 0..100 {
            for v in zooms.iter().chain(&pans) {
                render_backend(&mut m, v, Backend::Cpu);
            }
            if rustgis_tiles::cache::in_flight() == 0 {
                break;
            }
            rustgis_tiles::cache::wait(std::time::Duration::from_secs(5));
        }
        for backend in [Backend::Cpu, Backend::Gpu] {
            let ms = |views: &[View], m: &mut Map| {
                let t = std::time::Instant::now();
                for v in views {
                    render_backend(m, v, backend);
                }
                t.elapsed().as_secs_f64() * 1000.0 / views.len() as f64
            };
            // Off the pictures drawn while fetching.
            let zooms: Vec<View> =
                zooms.iter().map(|v| View { pixels_per_point: if backend == Backend::Cpu { 2.01 } else { 1.99 }, ..*v }).collect();
            let cold = ms(&zooms[1..], &mut m);
            let pans: Vec<View> = pans.iter().map(|v| View { pixels_per_point: zooms[0].pixels_per_point, ..*v }).collect();
            // The pan starts from a view already drawn.
            render_backend(&mut m, &zooms[0], backend);
            let pan = ms(&pans, &mut m);
            println!("{name:12} {backend:?}: zooming {cold:.1} ms/frame, panning {pan:.1} ms/frame");
        }
    }
}
