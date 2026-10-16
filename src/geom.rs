//! Geometry in eleceda's convention: mils, Y pointing down, rotations are visual counter-clockwise
//! degrees, and mirror (flip about the Y axis) is applied before rotation.

use serde::{Deserialize, Serialize};

pub const GRID: i32 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct Pt {
    pub x: i32,
    pub y: i32,
}

pub const fn pt(x: i32, y: i32) -> Pt {
    Pt { x, y }
}

impl std::ops::Add for Pt {
    type Output = Pt;
    fn add(self, o: Pt) -> Pt {
        pt(self.x + o.x, self.y + o.y)
    }
}
impl std::ops::Sub for Pt {
    type Output = Pt;
    fn sub(self, o: Pt) -> Pt {
        pt(self.x - o.x, self.y - o.y)
    }
}
impl std::ops::Mul<i32> for Pt {
    type Output = Pt;
    fn mul(self, k: i32) -> Pt {
        pt(self.x * k, self.y * k)
    }
}

impl Pt {
    pub fn manhattan(self, o: Pt) -> i32 {
        (self.x - o.x).abs() + (self.y - o.y).abs()
    }
}

/// Visual CCW rotation in degrees: 0, 90, 180 or 270.
pub type Rot = u16;

pub fn norm_rot(deg: i32) -> Rot {
    (((deg / 90 * 90) % 360 + 360) % 360) as Rot
}

/// Rotate a vector by a visual CCW angle on a Y-down plane.
pub fn rotate_vec(p: Pt, rot: Rot) -> Pt {
    match rot {
        90 => pt(p.y, -p.x),
        180 => pt(-p.x, -p.y),
        270 => pt(-p.y, p.x),
        _ => p,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Orient {
    pub rot: Rot,
    pub mirror: bool,
}

impl Orient {
    pub const IDENTITY: Orient = Orient { rot: 0, mirror: false };
    pub fn rot(rot: Rot) -> Self {
        Orient { rot, mirror: false }
    }
    pub fn apply(self, p: Pt) -> Pt {
        let m = if self.mirror { pt(-p.x, p.y) } else { p };
        rotate_vec(m, self.rot)
    }
    /// Direction angle after this orientation.
    pub fn angle(self, a: Rot) -> Rot {
        let mut a = a as i32;
        if self.mirror {
            a = (180 - a + 360) % 360;
        }
        norm_rot(a + self.rot as i32)
    }
    pub fn rect(self, r: Rect) -> Rect {
        Rect::from_points(self.apply(pt(r.x0, r.y0)), self.apply(pt(r.x1, r.y1)))
    }
}

/// Unit vector of a direction angle (0 = +x, 90 = up = -y, 180 = -x, 270 = down = +y).
pub fn dir_vec(a: Rot) -> Pt {
    rotate_vec(pt(1, 0), a)
}

pub fn opposite(a: Rot) -> Rot {
    norm_rot(a as i32 + 180)
}

pub fn is_horizontal(a: Rot) -> bool {
    a == 0 || a == 180
}

/// Axis-aligned rectangle with inclusive corners (x0 <= x1, y0 <= y1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Rect {
        Rect { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) }
    }
    pub fn from_points(a: Pt, b: Pt) -> Rect {
        Rect::new(a.x, a.y, b.x, b.y)
    }
    pub fn point(p: Pt) -> Rect {
        Rect::new(p.x, p.y, p.x, p.y)
    }
    pub fn w(&self) -> i32 {
        self.x1 - self.x0
    }
    pub fn h(&self) -> i32 {
        self.y1 - self.y0
    }
    pub fn union(self, o: Rect) -> Rect {
        Rect { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }
    pub fn add_point(self, p: Pt) -> Rect {
        self.union(Rect::point(p))
    }
    pub fn inflate(self, d: i32) -> Rect {
        Rect { x0: self.x0 - d, y0: self.y0 - d, x1: self.x1 + d, y1: self.y1 + d }
    }
    pub fn translate(self, d: Pt) -> Rect {
        Rect { x0: self.x0 + d.x, y0: self.y0 + d.y, x1: self.x1 + d.x, y1: self.y1 + d.y }
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x0 <= o.x1 && self.x1 >= o.x0 && self.y0 <= o.y1 && self.y1 >= o.y0
    }
    /// Overlap with positive area (touching edges do not count).
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x0 < o.x1 && self.x1 > o.x0 && self.y0 < o.y1 && self.y1 > o.y0
    }
    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.x0 && p.x <= self.x1 && p.y >= self.y0 && p.y <= self.y1
    }
    pub fn center(&self) -> Pt {
        pt((self.x0 + self.x1) / 2, (self.y0 + self.y1) / 2)
    }
}

pub fn bounds(points: impl IntoIterator<Item = Pt>) -> Option<Rect> {
    let mut r: Option<Rect> = None;
    for p in points {
        r = Some(match r {
            None => Rect::point(p),
            Some(r) => r.add_point(p),
        });
    }
    r
}

pub fn snap(v: i32) -> i32 {
    (v as f64 / GRID as f64).round() as i32 * GRID
}

/// Text width as eleceda draws it: a monospace font at 0.6 em per character.
pub fn text_width(s: &str, size: i32) -> i32 {
    (s.chars().count() as f64 * size as f64 * 0.6).ceil() as i32
}

/// True when p lies on the axis-aligned segment ab (endpoints included).
pub fn on_segment(p: Pt, a: Pt, b: Pt) -> bool {
    if a.x == b.x {
        p.x == a.x && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y)
    } else if a.y == b.y {
        p.y == a.y && p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x)
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_matches_eleceda() {
        // eleceda: rotateVec 90 => {x: p.y, y: -p.x}
        assert_eq!(rotate_vec(pt(0, -150), 90), pt(-150, 0));
        assert_eq!(dir_vec(90), pt(0, -1));
        assert_eq!(dir_vec(270), pt(0, 1));
        let o = Orient { rot: 0, mirror: true };
        assert_eq!(o.angle(0), 180);
        assert_eq!(Orient::rot(90).angle(270), 0);
    }
}
