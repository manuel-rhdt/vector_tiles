use nom::*;
use geo::{bounding_rect::BoundingRect, area::Area};

use std::cmp::Ordering;

#[derive(Debug)]
pub struct Shapefile<'a> {
    pub records: Vec<ShapeRecord<'a>>,
}

#[derive(Debug, Copy, Clone)]
pub struct ShapeRecord<'a> {
    /// The bounding rect of the shape.
    pub bounding_rect: geo::Rect<f64>,
    /// Indices into the `points` array designating the start of a part.
    parts: &'a [u32],
    /// Slice of [x, y] arrays
    points: &'a [[f64; 2]],
}

impl ShapeRecord<'_> {
    fn part(&self, index: usize) -> &[[f64; 2]] {
        let start = self.parts[index] as usize;
        let end = self
            .parts
            .get(index + 1)
            .map(|&x| x as usize)
            .unwrap_or(self.points.len());
        &self.points[start..end]
    }

    fn linestring(&self, index: usize) -> geo::LineString<f64> {
        self.part(index).iter().cloned().collect()
    }

    fn multi_linestring(&self) -> geo::MultiLineString<f64> {
        (0..self.parts.len()).map(|i| self.linestring(i)).collect()
    }
}

impl From<ShapeRecord<'_>> for geo::Polygon<f64> {
    fn from(record: ShapeRecord<'_>) -> geo::Polygon<f64> {
        if record.parts.len() == 0 {
            return geo::Polygon::new(geo::LineString(vec![]), vec![]);
        }
        if record.parts.len() == 1 {
            return geo::Polygon::new(record.linestring(0), vec![]);
        }

        let mut linestrings = record.multi_linestring();
        linestrings.0.sort_by(|l1, l2| {
            let area1 = l1.bounding_rect().map(|x| x.area()).unwrap_or(0.0);
            let area2 = l2.bounding_rect().map(|x| x.area()).unwrap_or(0.0);
            if area1 > area2 {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        });

        let outer_line_string = linestrings.0.pop().unwrap();

        geo::Polygon::new(outer_line_string, linestrings.0)
    }
}


unsafe fn slice_transmute<T>(bytes: &[u8]) -> &[T] {
    assert!(bytes.len() % std::mem::size_of::<T>() == 0);
    std::slice::from_raw_parts(
        bytes.as_ptr() as *const T,
        bytes.len() / std::mem::size_of::<T>(),
    )
}

named!(
    parse_rect(&[u8]) -> geo::Rect<f64>,
    do_parse!(
        xmin: le_f64 >>
        ymin: le_f64 >>
        xmax: le_f64 >>
        ymax: le_f64 >>
        (geo::Rect {
            min: [xmin, ymin].into(),
            max: [xmax, ymax].into()
        })
    )
);

named!(
    parse_record(&[u8]) -> ShapeRecord,
    preceded!(
        take!(4),
        length_value!(map!(be_i32, |val| val as usize * 2), do_parse!(
            verify!(le_u32, |num| num == 5) >>
            bounding_rect: parse_rect >>
            num_parts: le_u32 >>
            num_points: le_u32 >>
            parts: take!(num_parts as usize * 4) >>
            points: take!(num_points as usize * 2 * 8) >>
            (unsafe { ShapeRecord { bounding_rect, parts: slice_transmute(parts), points: slice_transmute(points) } })
        ))
    )
);

named!(
    pub parse_shp(&[u8]) -> Shapefile,
    do_parse!(
        verify!(be_u32, |num| num == 9994) >>
        take!(24) >>
        verify!(le_u32, |version| version == 1000) >>
        // we only accept polygon shapefiles
        verify!(le_u32, |shape_type| shape_type == 5) >>
        take!(64) >>
        records: many1!(complete!(parse_record)) >>
        (Shapefile { records })
    )
);