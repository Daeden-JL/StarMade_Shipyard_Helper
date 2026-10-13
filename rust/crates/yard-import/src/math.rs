//! Small vector and matrix helpers for placing the parts of a model.

pub type V3 = [f64; 3];

/// A 4x4 transform in rows; a point is a column on the right, so
/// `a.times(&b)` applies `b` first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4(pub [[f64; 4]; 4]);

impl Mat4 {
    pub const SAME: Mat4 = Mat4([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]);

    pub fn times(&self, other: &Mat4) -> Mat4 {
        let mut out = [[0.0; 4]; 4];
        for (i, row) in out.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..4).map(|k| self.0[i][k] * other.0[k][j]).sum();
            }
        }
        Mat4(out)
    }

    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let p = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
        [0, 1, 2].map(|i| (self.0[i][0] * p[0] + self.0[i][1] * p[1] + self.0[i][2] * p[2] + self.0[i][3]) as f32)
    }

    pub fn shift(t: V3) -> Mat4 {
        let mut m = Mat4::SAME;
        for (row, by) in m.0.iter_mut().zip(t) {
            row[3] = by;
        }
        m
    }

    pub fn stretch(s: V3) -> Mat4 {
        let mut m = Mat4::SAME;
        for (i, by) in s.into_iter().enumerate() {
            m.0[i][i] = by;
        }
        m
    }

    /// A turn about an axis by an angle in radians (right-handed).
    pub fn about(axis: V3, angle: f64) -> Mat4 {
        let length = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
        if length < 1e-12 || !angle.is_finite() {
            return Mat4::SAME;
        }
        let (x, y, z) = (axis[0] / length, axis[1] / length, axis[2] / length);
        let (s, c) = angle.sin_cos();
        let t = 1.0 - c;
        Mat4([
            [t * x * x + c, t * x * y - s * z, t * x * z + s * y, 0.0],
            [t * x * y + s * z, t * y * y + c, t * y * z - s * x, 0.0],
            [t * x * z - s * y, t * y * z + s * x, t * z * z + c, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    /// The turn a quaternion (x, y, z, w) stands for.
    pub fn quaternion(q: [f64; 4]) -> Mat4 {
        let length = q.iter().map(|c| c * c).sum::<f64>().sqrt();
        if length < 1e-12 {
            return Mat4::SAME;
        }
        let [x, y, z, w] = q.map(|c| c / length);
        Mat4([
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w), 0.0],
            [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w), 0.0],
            [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y), 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    /// From sixteen numbers written row after row.
    pub fn from_rows(n: &[f64]) -> Option<Mat4> {
        if n.len() < 16 || n.iter().take(16).any(|v| !v.is_finite()) {
            return None;
        }
        Some(Mat4([0, 1, 2, 3].map(|i| [0, 1, 2, 3].map(|j| n[i * 4 + j]))))
    }

    /// From sixteen numbers written column after column.
    pub fn from_columns(n: &[f64]) -> Option<Mat4> {
        if n.len() < 16 || n.iter().take(16).any(|v| !v.is_finite()) {
            return None;
        }
        Some(Mat4([0, 1, 2, 3].map(|i| [0, 1, 2, 3].map(|j| n[j * 4 + i]))))
    }

    /// The transform that undoes this one, for the usual kind whose last row
    /// is 0 0 0 1. A transform that flattens space has none.
    pub fn inverse(&self) -> Option<Mat4> {
        let m = &self.0;
        let c = |i: usize, j: usize| {
            let (a, b) = ((i + 1) % 3, (i + 2) % 3);
            let (p, q) = ((j + 1) % 3, (j + 2) % 3);
            m[a][p] * m[b][q] - m[a][q] * m[b][p]
        };
        let det = m[0][0] * c(0, 0) + m[0][1] * c(0, 1) + m[0][2] * c(0, 2);
        if det.abs() < 1e-300 || !det.is_finite() {
            return None;
        }
        let mut out = Mat4::SAME;
        for i in 0..3 {
            for j in 0..3 {
                out.0[i][j] = c(j, i) / det;
            }
        }
        for i in 0..3 {
            out.0[i][3] = -(0..3).map(|k| out.0[i][k] * m[k][3]).sum::<f64>();
        }
        Some(out)
    }
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn wide(p: [f32; 3]) -> V3 {
    [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]
}

/// The point of a triangle nearest to `p`, as the weights of its three
/// corners (Ericson, Real-Time Collision Detection, 5.1.5).
pub fn nearest_on_triangle(p: V3, [a, b, c]: [V3; 3]) -> [f64; 3] {
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return [1.0, 0.0, 0.0];
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return [0.0, 1.0, 0.0];
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = ratio(d1, d1 - d3);
        return [1.0 - v, v, 0.0];
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return [0.0, 0.0, 1.0];
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = ratio(d2, d2 - d6);
        return [1.0 - w, 0.0, w];
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = ratio(d4 - d3, (d4 - d3) + (d5 - d6));
        return [0.0, 1.0 - w, w];
    }
    let sum = va + vb + vc;
    if sum.abs() < 1e-300 {
        return [1.0, 0.0, 0.0];
    }
    let (v, w) = (vb / sum, vc / sum);
    [1.0 - v - w, v, w]
}

/// A part of a whole between 0 and 1, also when the whole is nothing.
fn ratio(part: f64, whole: f64) -> f64 {
    if whole.abs() < 1e-300 {
        0.0
    } else {
        (part / whole).clamp(0.0, 1.0)
    }
}

pub fn blend(weights: [f64; 3], [a, b, c]: [V3; 3]) -> V3 {
    [0, 1, 2].map(|i| weights[0] * a[i] + weights[1] * b[i] + weights[2] * c[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_undoes_a_shifted_turned_stretched_transform() {
        let m = Mat4::shift([1.0, 2.0, 3.0]).times(&Mat4::about([0.0, 1.0, 0.0], 0.7)).times(&Mat4::stretch([2.0, 3.0, 4.0]));
        let back = m.inverse().expect("has an inverse").times(&m);
        for i in 0..4 {
            for j in 0..4 {
                assert!((back.0[i][j] - Mat4::SAME.0[i][j]).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn nearest_point_of_a_triangle_is_a_corner_an_edge_or_the_face() {
        let t = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]];
        assert_eq!(nearest_on_triangle([-1.0, -1.0, 0.0], t), [1.0, 0.0, 0.0]);
        assert_eq!(nearest_on_triangle([1.0, -1.0, 5.0], t), [0.5, 0.5, 0.0]);
        let inside = nearest_on_triangle([0.5, 0.5, 3.0], t);
        assert!((inside[0] - 0.5).abs() < 1e-12 && (inside[1] - 0.25).abs() < 1e-12);
    }
}
