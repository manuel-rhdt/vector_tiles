use std::error::Error;
use std::fs;
use std::io::{Cursor, Read};

use std::path::Path;
use std::sync::mpsc;

use rayon;
use rayon::prelude::*;

use serde_derive::*;

use geo::{area::Area, simplifyvw::SimplifyVW};

use indicatif::{ProgressBar, ProgressStyle};
use zip::ZipArchive;

mod clip;
use crate::clip::Clip;

mod shapefile;
use crate::shapefile::parse_shp;

fn tiles_for_z(z: u32) -> u32 {
    (0..=z).map(|z| 4u32.pow(z)).sum()
}

// returns the tile rect for given (x, y, -z).
fn get_tile(x: i32, y: i32, zoom: i32, overlap: f64) -> geo::Rect<f64> {
    let tile_width = 360.0 * 2.0f64.powi(zoom);
    let tile_height = 180.0 * 2.0f64.powi(zoom);
    let xmin = -180.0 + x as f64 * tile_width;
    let xmax = xmin + tile_width;
    let ymin = -90.0 + y as f64 * tile_height;
    let ymax = ymin + tile_height;

    let x_overlap = (xmax - xmin) * overlap / 2.0;
    let y_overlap = (ymax - ymin) * overlap / 2.0;

    geo::Rect {
        min: geo::Coordinate {
            x: xmin - x_overlap,
            y: ymin - y_overlap,
        },
        max: geo::Coordinate {
            x: xmax + x_overlap,
            y: ymax + y_overlap,
        },
    }
}

fn create_tile(
    polygons: &geo::MultiPolygon<f64>,
    tile_rect: &geo::Rect<f64>,
) -> geo::MultiPolygon<f64> {
    polygons
        .0
        .iter()
        .map(|poly| poly.clip(*tile_rect))
        .filter(|poly| poly.exterior.0.len() > 3)
        .collect()
}

fn write_geojson(filename: &Path, polygons: &geo::MultiPolygon<f64>) -> Result<(), Box<dyn Error>> {
    let geometry = geojson::Geometry::new(polygons.into());

    let geojson = geojson::GeoJson::Feature(geojson::Feature {
        bbox: None,
        geometry: Some(geometry),
        id: None,
        properties: None,
        foreign_members: None,
    });

    fs::write(filename, geojson.to_string())?;

    Ok(())
}

struct WriteRequest {
    polygon: geo::MultiPolygon<f64>,
    tile: (u32, u32, u32),
    tile_options: TileOptions,
}

fn write_tile_recursive(
    tx: mpsc::Sender<WriteRequest>,
    poly: &geo::MultiPolygon<f64>,
    tile: (u32, u32, u32),
    tile_options: TileOptions,
) {
    let (z, x, y) = tile;

    // 1 % overlap between tiles
    let tile_rect = get_tile(x as i32, y as i32, -(z as i32), 0.01);
    let poly = create_tile(&poly, &tile_rect);

    // recurse through the sub-tiles
    if z < tile_options.max_level {
        let tx = tx.clone();
        rayon::scope(|s| {
            let tile_options = tile_options.clone();
            let tx = tx;
            let tx1 = tx.clone();
            let to = tile_options.clone();
            s.spawn(|_| write_tile_recursive(tx1, &poly, (z + 1, 2 * x, 2 * y), to));
            let tx2 = tx.clone();
            let to = tile_options.clone();
            s.spawn(|_| write_tile_recursive(tx2, &poly, (z + 1, 2 * x + 1, 2 * y), to));
            let tx3 = tx.clone();
            let to = tile_options.clone();
            s.spawn(|_| write_tile_recursive(tx3, &poly, (z + 1, 2 * x, 2 * y + 1), to));
            let tx4 = tx.clone();
            let to = tile_options.clone();
            s.spawn(|_| write_tile_recursive(tx4, &poly, (z + 1, 2 * x + 1, 2 * y + 1), to));
        })
    }

    // write this tile

    let min_area = tile_rect.area() / 1024f64 / 512f64;
    // don't simplify if we reach a very small area
    let simplified_polygon = if min_area > 0.00001 {
        geo::MultiPolygon(
            poly.0
                .into_par_iter()
                .map(|poly| poly.simplifyvw(&min_area))
                .filter(|polygon| polygon.exterior.0.len() > 3)
                .collect(),
        )
    } else {
        poly
    };

    let req = WriteRequest {
        tile: (z, x, y),
        polygon: simplified_polygon,
        tile_options,
    };
    tx.send(req).unwrap();
}

#[derive(Deserialize)]
struct Configuration {
    tiles: Vec<TileOptions>,
}

#[derive(Deserialize, Clone)]
struct TileOptions {
    source: Source,
    max_level: u32,
    output: String,
    #[serde(default = "default_prefix")]
    tile_prefix: String,
}

#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum Source {
    Filename(String),
    Online {
        url: String,
        encoding: Option<Encoding>,
    },
    Local {
        path: String,
        encoding: Option<Encoding>,
    },
}

impl Source {
    fn canonicalize(&self) -> Source {
        match self {
            Source::Filename(path) => Source::Local {
                path: path.clone(),
                encoding: None,
            },
            x => x.clone(),
        }
    }
}

#[derive(Deserialize, Copy, Clone)]
#[serde(rename_all = "snake_case")]
enum Encoding {
    Zip,
}

fn default_prefix() -> String {
    "tile_".into()
}

fn download_resource(url: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let resp = reqwest::get(url)?;

    let content_len = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|ct_len| ct_len.to_str().ok())
        .and_then(|ct_len| ct_len.parse().ok())
        .unwrap_or(0);

    let bar = ProgressBar::new(content_len);
    bar.set_style(
        ProgressStyle::default_bar()
            .template("> {msg}\n[{percent} %] {bar} [{bytes} / {total_bytes}] [ETA {eta}]"),
    );
    bar.set_message(&format!("Downloading {}", url));

    let mut vec = Vec::with_capacity(content_len as usize);
    bar.wrap_read(resp).read_to_end(&mut vec)?;

    bar.finish();
    Ok(vec)
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut settings = config::Config::default();
    settings.merge(config::File::with_name("Settings"))?;
    let conf: Configuration = settings.try_into()?;

    let (tx, rx) = mpsc::channel();
    let mut number_of_tiles = 0;
    for tile_options in conf.tiles {
        // first thing to do is check the existence of output directory
        let path = Path::new(&tile_options.output);
        if !path.exists() {
            fs::create_dir(path)?;
        }

        let (data, encoding) = match &tile_options.source.canonicalize() {
            Source::Local { path, encoding } => (fs::read(path)?, *encoding),
            Source::Online { url, encoding } => (download_resource(url)?, *encoding),
            _ => unreachable!(),
        };

        let data = match encoding {
            Some(Encoding::Zip) => {
                let mut archive = ZipArchive::new(Cursor::new(data))?;
                let mut data = vec![];
                for i in 0..archive.len() {
                    let file = archive.by_index(i)?;
                    let name = file.sanitized_name();
                    if name.extension().map(|ext| ext == "shp").unwrap_or(false) {
                        let bar = ProgressBar::new(file.size());
                        bar.set_style(ProgressStyle::default_bar().template(
                            "> {msg}\n[{percent} %] {bar} [{bytes} / {total_bytes}] [ETA {eta}]",
                        ));
                        bar.set_message(&format!("Decompressing"));

                        data.reserve(file.size() as usize);
                        bar.wrap_read(file).read_to_end(&mut data)?;
                        bar.finish();
                        break;
                    }
                }
                data
            }
            None => data,
        };

        let (_, shapefile) = parse_shp(&data)
            .map_err(|err| err.into_error_kind().description().to_string())
            .unwrap();

        let polygons: geo::MultiPolygon<f64> = shapefile
            .records
            .into_iter()
            .map(|record| geo::Polygon::from(record))
            .collect();

        let opts = tile_options.clone();
        let tx1 = tx.clone();
        rayon::spawn(move || {
            write_tile_recursive(tx1, &polygons, (0, 0, 0), opts);
        });

        number_of_tiles += tiles_for_z(tile_options.max_level);
    }
    std::mem::drop(tx);

    let bar = ProgressBar::new(number_of_tiles as u64);
    bar.set_style(ProgressStyle::default_bar().template("> {msg}\n[{percent} %] {bar} {pos}/{len}"));

    bar.set_message("Generating Tiles...");
    for req in rx {
        let path = Path::new(&req.tile_options.output);
        let filename = &format!(
            "{}{}.{}.{}.json",
            req.tile_options.tile_prefix, req.tile.0, req.tile.1, req.tile.2
        );
        let path = path.join(filename);
        write_geojson(&path, &req.polygon)?;
        bar.inc(1);
    }
    bar.finish();

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_tiles() {
        assert_eq!(
            get_tile(0, 0, 0, 0.0),
            geo::Rect {
                min: [-180.0, -90.0].into(),
                max: [180.0, 90.0].into()
            }
        );
        assert_eq!(
            get_tile(0, 0, -1, 0.0),
            geo::Rect {
                min: [-180.0, -90.0].into(),
                max: [0.0, 0.0].into()
            }
        );
    }

}
