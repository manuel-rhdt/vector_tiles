use geo::{Polygon, Rect};
use geo::contains::Contains;
use num::Float;

use std::borrow::Cow;

pub trait Axis {
    const INDEX: usize;
}
pub struct X;
impl Axis for X {
    const INDEX: usize = 0;
}
pub struct Y;
impl Axis for Y {
    const INDEX: usize = 1;
}

pub trait GetCoord<U> {
    fn coord<T: Axis>(self) -> U;
}

impl<U: geo::CoordinateType> GetCoord<U> for geo::Coordinate<U> {
    fn coord<T: Axis>(self) -> U {
        match T::INDEX {
            0 => self.x,
            1 => self.y,
            _ => panic!(),
        }
    }
}

pub trait Clip<T> {
    fn clip(&self, clipto: T) -> Self;
}

impl<T: Float> Clip<Rect<T>> for Polygon<T> {
    fn clip(&self, rect: Rect<T>) -> Polygon<T> {
        clip_polygon(self, rect)
    }
}

fn interpolate<T: Float>(a: geo::Coordinate<T>, b: geo::Coordinate<T>, t: T) -> geo::Coordinate<T> {
    let v = (b.x - a.x, b.y - a.y);
    [a.x + t * v.0, a.y + t * v.1].into()
}

fn intersect<U: Float, T: Axis>(
    a: geo::Coordinate<U>,
    b: geo::Coordinate<U>,
    k: U,
) -> geo::Coordinate<U> {
    let t = (k - a.coord::<T>()) / (b.coord::<T>() - a.coord::<T>());
    interpolate(a, b, t)
}

fn clip_line<T: Float, A: Axis>(
    line_strip: &geo::LineString<T>,
    k1: T,
    k2: T,
) -> Cow<'_, geo::LineString<T>> {
    assert!(k1 <= k2);

    // trivial reject
    if !line_strip
        .0
        .iter()
        .map(|point| point.coord::<A>())
        .any(|coord| coord < k2 && coord > k1)
    {
        return Cow::Owned(geo::LineString(vec![]));
    }

    // trivial accept
    if line_strip
        .0
        .iter()
        .map(|point| point.coord::<A>())
        .all(|coord| coord < k2 && coord > k1)
    {
        return Cow::Borrowed(line_strip);
    }

    let mut result = Vec::new();
    for line in line_strip.0.windows(2) {
        let a = line[0].coord::<A>();
        let b = line[1].coord::<A>();

        if a < k1 {
            // ---|-->  | (line enters the clip region from the left)
            if b > k1 {
                result.push(intersect::<T, A>(line[0], line[1], k1));
            }
        } else if a > k2 {
            // |  <--|--- (line enters the clip region from the right)
            if b < k2 {
                result.push(intersect::<T, A>(line[0], line[1], k2));
            }
        } else {
            result.push(line[0])
        }

        if b < k1 && a >= k1 {
            // <--|---  | or <--|-----|--- (line exits the clip region on the
            // left)
            result.push(intersect::<T, A>(line[0], line[1], k1));
        }
        if b > k2 && a <= k2 {
            // |  ---|--> or ---|-----|--> (line exits the clip region on the
            // right)
            result.push(intersect::<T, A>(line[0], line[1], k2));
        }
    }

    // add last point
    let last = line_strip.0.last();
    if let Some(&last) = last {
        let a = geo::Coordinate::from(last).coord::<A>();
        if a >= k1 && a <= k2 {
            result.push(last)
        }
    }

    // close the polygon if its endpoints are not the same after clipping
    if result.first() != result.last() {
        if let Some(&first) = result.first() {
            result.push(first)
        }
    }

    Cow::Owned(geo::LineString(result))
}

fn clip_polygon<T: Float>(polygon: &geo::Polygon<T>, rect: geo::Rect<T>) -> geo::Polygon<T> {
    let exterior = &polygon.exterior;
    let exterior = clip_line::<T, X>(exterior, rect.min.x, rect.max.x);
    let mut exterior = clip_line::<T, Y>(&exterior, rect.min.y, rect.max.y);

    // If the rect is contained entirely in the polygon we want to return the
    // rect itself as polygon.
    if exterior.0.len() == 0 {
        let rect_polygon = geo::Polygon::from(rect);
        if polygon.contains(&rect_polygon) {
            exterior = Cow::Owned(rect_polygon.exterior);
        }
    }

    let interiors = polygon
        .interiors
        .iter()
        .map(|line| {
            let line = clip_line::<T, X>(line, rect.min.x, rect.max.x);
            let line = clip_line::<T, Y>(&line, rect.min.y, rect.max.y);
            line.into_owned()
        })
        .collect();

    geo::Polygon::new(exterior.into_owned(), interiors)
}
