//! 2D/3D curves.
//!
//! `SplineKind::Polyline` evaluates the points as straight segments.
//! `CatmullRom` uses a uniform Catmull-Rom spline through every point (closed
//! curves wrap, open curves duplicate their end points for the tangents).
//! `Bezier` treats the points as a chain of cubic segments: each segment
//! consumes four points and the next starts at the previous segment's endpoint
//! (so points are grouped `[p0,p1,p2,p3]`, then `[p3,p4,p5,p6]`, ...). A
//! trailing 2-point remainder becomes a straight cubic and a trailing 3-point
//! remainder is a quadratic raised to cubic; a single point is a degenerate
//! point curve.
//!
//! `t` is always in `0..=1`; values outside are clamped. A closed curve has
//! `eval(1) == eval(0)`. `flatten` returns the vertex list without repeating
//! the first point for closed curves — the `closed` flag carries the closure.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplineKind {
    #[default]
    Polyline,
    CatmullRom,
    Bezier,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curve2 {
    pub points: Vec<[f32; 2]>,
    pub closed: bool,
    pub kind: SplineKind,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curve3 {
    pub points: Vec<[f32; 3]>,
    pub closed: bool,
    pub kind: SplineKind,
}

fn lerp2(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn cubic2(p: [[f32; 2]; 4], u: f32) -> [f32; 2] {
    let v = 1.0 - u;
    let (a, b, c, d) = (v * v * v, 3.0 * v * v * u, 3.0 * v * u * u, u * u * u);
    [
        a * p[0][0] + b * p[1][0] + c * p[2][0] + d * p[3][0],
        a * p[0][1] + b * p[1][1] + c * p[2][1] + d * p[3][1],
    ]
}

fn cubic3(p: [[f32; 3]; 4], u: f32) -> [f32; 3] {
    let v = 1.0 - u;
    let (a, b, c, d) = (v * v * v, 3.0 * v * v * u, 3.0 * v * u * u, u * u * u);
    [
        a * p[0][0] + b * p[1][0] + c * p[2][0] + d * p[3][0],
        a * p[0][1] + b * p[1][1] + c * p[2][1] + d * p[3][1],
        a * p[0][2] + b * p[1][2] + c * p[2][2] + d * p[3][2],
    ]
}

fn cr2(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], u: f32) -> [f32; 2] {
    let u2 = u * u;
    let u3 = u2 * u;
    let mut out = [0.0f32; 2];
    for k in 0..2 {
        out[k] = 0.5
            * (2.0 * p1[k]
                + (-p0[k] + p2[k]) * u
                + (2.0 * p0[k] - 5.0 * p1[k] + 4.0 * p2[k] - p3[k]) * u2
                + (-p0[k] + 3.0 * p1[k] - 3.0 * p2[k] + p3[k]) * u3);
    }
    out
}

fn cr3(p0: [f32; 3], p1: [f32; 3], p2: [f32; 3], p3: [f32; 3], u: f32) -> [f32; 3] {
    let u2 = u * u;
    let u3 = u2 * u;
    let mut out = [0.0f32; 3];
    for k in 0..3 {
        out[k] = 0.5
            * (2.0 * p1[k]
                + (-p0[k] + p2[k]) * u
                + (2.0 * p0[k] - 5.0 * p1[k] + 4.0 * p2[k] - p3[k]) * u2
                + (-p0[k] + 3.0 * p1[k] - 3.0 * p2[k] + p3[k]) * u3);
    }
    out
}

fn bezier_segments2(points: &[[f32; 2]]) -> Vec<[[f32; 2]; 4]> {
    let n = points.len();
    let mut segs = Vec::new();
    if n == 0 {
        return segs;
    }
    if n == 1 {
        segs.push([points[0]; 4]);
        return segs;
    }
    let mut i = 0;
    while i + 3 < n {
        segs.push([points[i], points[i + 1], points[i + 2], points[i + 3]]);
        i += 3;
    }
    match n - i {
        2 => {
            let p0 = points[i];
            let p3 = points[i + 1];
            segs.push([p0, lerp2(p0, p3, 1.0 / 3.0), lerp2(p0, p3, 2.0 / 3.0), p3]);
        }
        3 => {
            let p0 = points[i];
            let q = points[i + 1];
            let p3 = points[i + 2];
            segs.push([p0, lerp2(p0, q, 2.0 / 3.0), lerp2(p3, q, 2.0 / 3.0), p3]);
        }
        _ => {}
    }
    segs
}

fn bezier_segments3(points: &[[f32; 3]]) -> Vec<[[f32; 3]; 4]> {
    let n = points.len();
    let mut segs = Vec::new();
    if n == 0 {
        return segs;
    }
    if n == 1 {
        segs.push([points[0]; 4]);
        return segs;
    }
    let mut i = 0;
    while i + 3 < n {
        segs.push([points[i], points[i + 1], points[i + 2], points[i + 3]]);
        i += 3;
    }
    match n - i {
        2 => {
            let p0 = points[i];
            let p3 = points[i + 1];
            segs.push([p0, lerp3(p0, p3, 1.0 / 3.0), lerp3(p0, p3, 2.0 / 3.0), p3]);
        }
        3 => {
            let p0 = points[i];
            let q = points[i + 1];
            let p3 = points[i + 2];
            segs.push([p0, lerp3(p0, q, 2.0 / 3.0), lerp3(p3, q, 2.0 / 3.0), p3]);
        }
        _ => {}
    }
    segs
}

fn polyline_length2(points: &[[f32; 2]], closed: bool) -> f32 {
    let mut total = 0.0;
    for w in points.windows(2) {
        total += dist2(w[0], w[1]);
    }
    if closed && points.len() > 1 {
        total += dist2(points[points.len() - 1], points[0]);
    }
    total
}

fn polyline_length3(points: &[[f32; 3]], closed: bool) -> f32 {
    let mut total = 0.0;
    for w in points.windows(2) {
        total += dist3(w[0], w[1]);
    }
    if closed && points.len() > 1 {
        total += dist3(points[points.len() - 1], points[0]);
    }
    total
}

impl Curve2 {
    pub fn eval(&self, t: f32) -> [f32; 2] {
        let t = t.clamp(0.0, 1.0);
        let n = self.points.len();
        if n == 0 {
            return [0.0, 0.0];
        }
        if n == 1 {
            return self.points[0];
        }
        match self.kind {
            SplineKind::Polyline => {
                let segs = if self.closed { n } else { n - 1 };
                let x = t * segs as f32;
                let mut i = x.floor() as usize;
                if i >= segs {
                    i = segs - 1;
                }
                let u = (x - i as f32).clamp(0.0, 1.0);
                let a = self.points[i];
                let b = if i + 1 < n {
                    self.points[i + 1]
                } else {
                    self.points[0]
                };
                lerp2(a, b, u)
            }
            SplineKind::CatmullRom => {
                let segs = if self.closed { n } else { n - 1 };
                let x = t * segs as f32;
                let mut i = x.floor() as usize;
                if i >= segs {
                    i = segs - 1;
                }
                let u = (x - i as f32).clamp(0.0, 1.0);
                let (p0, p1, p2, p3) = if self.closed {
                    (
                        self.points[(i + n - 1) % n],
                        self.points[i % n],
                        self.points[(i + 1) % n],
                        self.points[(i + 2) % n],
                    )
                } else {
                    (
                        self.points[i.saturating_sub(1)],
                        self.points[i],
                        self.points[i + 1],
                        self.points[(i + 2).min(n - 1)],
                    )
                };
                cr2(p0, p1, p2, p3, u)
            }
            SplineKind::Bezier => {
                let segs = bezier_segments2(&self.points);
                let m = segs.len();
                let x = t * m as f32;
                let mut i = x.floor() as usize;
                if i >= m {
                    i = m - 1;
                }
                let u = (x - i as f32).clamp(0.0, 1.0);
                cubic2(segs[i], u)
            }
        }
    }

    pub fn flatten(&self, tolerance: f32) -> Vec<[f32; 2]> {
        let tol = tolerance.max(1.0e-5);
        match self.kind {
            SplineKind::Polyline => self.points.clone(),
            SplineKind::Bezier => self.flatten_bezier_kurbo(tol),
            SplineKind::CatmullRom => {
                let mut out = flatten_smooth2(self, tol);
                if self.closed && out.len() > 1 {
                    out.pop();
                }
                out
            }
        }
    }

    fn flatten_bezier_kurbo(&self, tol: f32) -> Vec<[f32; 2]> {
        let mut pts = self.points.clone();
        if self.closed {
            if let Some(&first) = pts.first() {
                if pts.last() != Some(&first) {
                    pts.push(first);
                }
            }
        }
        let segs = bezier_segments2(&pts);
        if segs.is_empty() {
            return Vec::new();
        }
        let mut path = kurbo::BezPath::new();
        path.move_to(kurbo::Point::new(
            segs[0][0][0] as f64,
            segs[0][0][1] as f64,
        ));
        for s in &segs {
            path.curve_to(
                kurbo::Point::new(s[1][0] as f64, s[1][1] as f64),
                kurbo::Point::new(s[2][0] as f64, s[2][1] as f64),
                kurbo::Point::new(s[3][0] as f64, s[3][1] as f64),
            );
        }
        let mut out = Vec::new();
        kurbo::flatten(path.elements().iter().copied(), tol as f64, |el| match el {
            kurbo::PathEl::MoveTo(p) => out.push([p.x as f32, p.y as f32]),
            kurbo::PathEl::LineTo(p) => out.push([p.x as f32, p.y as f32]),
            _ => {}
        });
        if out.len() > 1 && out[0] == out[out.len() - 1] {
            out.pop();
        }
        out
    }

    pub fn length(&self) -> f32 {
        if self.points.len() < 2 {
            return 0.0;
        }
        let pts = self.flatten(1.0e-4);
        polyline_length2(&pts, self.closed)
    }

    /// Arc-length resampling. An open curve yields `n` points including both
    /// endpoints; a closed curve yields `n` points around the loop without
    /// repeating the first. The result is always a polyline.
    pub fn resample(&self, n: usize) -> Curve2 {
        let mut out = Curve2 {
            points: Vec::new(),
            closed: self.closed,
            kind: SplineKind::Polyline,
        };
        if n == 0 || self.points.is_empty() {
            return out;
        }
        if n == 1 {
            out.points.push(self.eval(0.0));
            return out;
        }
        let pts = self.flatten(1.0e-4);
        if pts.len() < 2 {
            out.points = vec![self.eval(0.0); n];
            return out;
        }
        let total = polyline_length2(&pts, self.closed);
        if total <= 1.0e-9 {
            out.points = vec![pts[0]; n];
            return out;
        }
        let count = if self.closed { n } else { n - 1 };
        let mut seg = 0usize;
        let mut seg_start = 0.0f32;
        let mut seg_len = dist2(pts[0], pts[1]);
        for i in 0..n {
            let target = total * (i as f32) / (count as f32);
            while seg + 1 < pts.len() && seg_start + seg_len < target {
                seg_start += seg_len;
                seg += 1;
                let j = (seg + 1) % pts.len();
                seg_len = dist2(pts[seg], pts[j]);
            }
            let u = if seg_len > 1.0e-12 {
                ((target - seg_start) / seg_len).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let a = pts[seg];
            let b = pts[(seg + 1) % pts.len()];
            out.points.push(lerp2(a, b, u));
        }
        out
    }

    /// Signed polygon area; positive means counter-clockwise.
    pub fn signed_area(&self) -> f32 {
        let pts = self.flatten(1.0e-4);
        let n = pts.len();
        if n < 3 {
            return 0.0;
        }
        let mut sum = 0.0f32;
        for i in 0..n {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            sum += a[0] * b[1] - b[0] * a[1];
        }
        sum * 0.5
    }

    pub fn reverse(&self) -> Curve2 {
        let mut c = self.clone();
        c.points.reverse();
        c
    }
}

fn flatten_smooth2(curve: &Curve2, tol: f32) -> Vec<[f32; 2]> {
    let n = curve.points.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![curve.points[0]];
    }
    let segs = if curve.closed { n } else { n - 1 };
    let mut out = vec![curve.eval(0.0)];
    for i in 0..segs {
        let t0 = i as f32 / segs as f32;
        let t1 = (i + 1) as f32 / segs as f32;
        flatten_segment2(
            curve,
            t0,
            t1,
            curve.eval(t0),
            curve.eval(t1),
            tol,
            0,
            &mut out,
        );
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn flatten_segment2(
    curve: &Curve2,
    t0: f32,
    t1: f32,
    p0: [f32; 2],
    p1: [f32; 2],
    tol: f32,
    depth: usize,
    out: &mut Vec<[f32; 2]>,
) {
    let tm = (t0 + t1) * 0.5;
    let pm = curve.eval(tm);
    let chord = dist2(p0, p1);
    let deviation = if chord > 1.0e-9 {
        let dx = p1[0] - p0[0];
        let dy = p1[1] - p0[1];
        let cross = ((pm[0] - p0[0]) * dy - (pm[1] - p0[1]) * dx).abs();
        cross / chord
    } else {
        dist2(pm, p0)
    };
    if depth >= 14 || deviation <= tol {
        out.push(p1);
        return;
    }
    flatten_segment2(curve, t0, tm, p0, pm, tol, depth + 1, out);
    flatten_segment2(curve, tm, t1, pm, p1, tol, depth + 1, out);
}

impl Curve3 {
    pub fn eval(&self, t: f32) -> [f32; 3] {
        let t = t.clamp(0.0, 1.0);
        let n = self.points.len();
        if n == 0 {
            return [0.0; 3];
        }
        if n == 1 {
            return self.points[0];
        }
        match self.kind {
            SplineKind::Polyline => {
                let segs = if self.closed { n } else { n - 1 };
                let x = t * segs as f32;
                let mut i = x.floor() as usize;
                if i >= segs {
                    i = segs - 1;
                }
                let u = (x - i as f32).clamp(0.0, 1.0);
                let a = self.points[i];
                let b = if i + 1 < n {
                    self.points[i + 1]
                } else {
                    self.points[0]
                };
                lerp3(a, b, u)
            }
            SplineKind::CatmullRom => {
                let segs = if self.closed { n } else { n - 1 };
                let x = t * segs as f32;
                let mut i = x.floor() as usize;
                if i >= segs {
                    i = segs - 1;
                }
                let u = (x - i as f32).clamp(0.0, 1.0);
                let (p0, p1, p2, p3) = if self.closed {
                    (
                        self.points[(i + n - 1) % n],
                        self.points[i % n],
                        self.points[(i + 1) % n],
                        self.points[(i + 2) % n],
                    )
                } else {
                    (
                        self.points[i.saturating_sub(1)],
                        self.points[i],
                        self.points[i + 1],
                        self.points[(i + 2).min(n - 1)],
                    )
                };
                cr3(p0, p1, p2, p3, u)
            }
            SplineKind::Bezier => {
                let segs = bezier_segments3(&self.points);
                let m = segs.len();
                let x = t * m as f32;
                let mut i = x.floor() as usize;
                if i >= m {
                    i = m - 1;
                }
                let u = (x - i as f32).clamp(0.0, 1.0);
                cubic3(segs[i], u)
            }
        }
    }

    pub fn flatten(&self, tolerance: f32) -> Vec<[f32; 3]> {
        let tol = tolerance.max(1.0e-5);
        match self.kind {
            SplineKind::Polyline => self.points.clone(),
            SplineKind::CatmullRom | SplineKind::Bezier => {
                let mut out = flatten_smooth3(self, tol);
                if self.closed && out.len() > 1 {
                    out.pop();
                }
                out
            }
        }
    }

    pub fn length(&self) -> f32 {
        if self.points.len() < 2 {
            return 0.0;
        }
        let pts = self.flatten(1.0e-4);
        polyline_length3(&pts, self.closed)
    }

    /// Arc-length resampling, with the same conventions as [`Curve2::resample`].
    pub fn resample(&self, n: usize) -> Curve3 {
        let mut out = Curve3 {
            points: Vec::new(),
            closed: self.closed,
            kind: SplineKind::Polyline,
        };
        if n == 0 || self.points.is_empty() {
            return out;
        }
        if n == 1 {
            out.points.push(self.eval(0.0));
            return out;
        }
        let pts = self.flatten(1.0e-4);
        if pts.len() < 2 {
            out.points = vec![self.eval(0.0); n];
            return out;
        }
        let total = polyline_length3(&pts, self.closed);
        if total <= 1.0e-9 {
            out.points = vec![pts[0]; n];
            return out;
        }
        let count = if self.closed { n } else { n - 1 };
        let mut seg = 0usize;
        let mut seg_start = 0.0f32;
        let mut seg_len = dist3(pts[0], pts[1]);
        for i in 0..n {
            let target = total * (i as f32) / (count as f32);
            while seg + 1 < pts.len() && seg_start + seg_len < target {
                seg_start += seg_len;
                seg += 1;
                let j = (seg + 1) % pts.len();
                seg_len = dist3(pts[seg], pts[j]);
            }
            let u = if seg_len > 1.0e-12 {
                ((target - seg_start) / seg_len).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let a = pts[seg];
            let b = pts[(seg + 1) % pts.len()];
            out.points.push(lerp3(a, b, u));
        }
        out
    }

    /// Laplacian smoothing of the control points. Endpoints are pinned for
    /// open curves; a closed curve smooths all points cyclically.
    pub fn smooth(&self, iterations: usize, factor: f32) -> Curve3 {
        let f = factor.clamp(0.0, 1.0);
        let mut c = self.clone();
        let n = c.points.len();
        if n < 3 || f <= 0.0 {
            return c;
        }
        for _ in 0..iterations {
            let prev = c.points.clone();
            for i in 0..n {
                if !c.closed && (i == 0 || i == n - 1) {
                    continue;
                }
                let a = if i == 0 { prev[n - 1] } else { prev[i - 1] };
                let b = if i + 1 == n { prev[0] } else { prev[i + 1] };
                let avg = [
                    (a[0] + b[0]) * 0.5,
                    (a[1] + b[1]) * 0.5,
                    (a[2] + b[2]) * 0.5,
                ];
                for k in 0..3 {
                    c.points[i][k] = prev[i][k] + (avg[k] - prev[i][k]) * f;
                }
            }
        }
        c
    }

    /// Extract the `t0..t1` sub-curve as a polyline.
    pub fn trim(&self, t0: f32, t1: f32) -> Curve3 {
        let t0 = t0.clamp(0.0, 1.0);
        let t1 = t1.clamp(0.0, 1.0);
        let samples = (self.points.len() * 8).clamp(8, 256);
        let mut out = Curve3 {
            points: Vec::with_capacity(samples),
            closed: false,
            kind: SplineKind::Polyline,
        };
        for i in 0..=samples {
            let t = t0 + (t1 - t0) * (i as f32 / samples as f32);
            out.points.push(self.eval(t));
        }
        out
    }

    /// Extend both ends along their end tangents by `by` metres. Closed
    /// curves and curves with fewer than two points are returned unchanged.
    /// The result is a polyline.
    pub fn extend(&self, by: f32) -> Curve3 {
        if self.closed || self.points.len() < 2 || by <= 0.0 {
            return self.clone();
        }
        let n = self.points.len();
        let first = self.points[0];
        let second = self.points[1];
        let last = self.points[n - 1];
        let penultimate = self.points[n - 2];
        let ds = dist3(first, second);
        let de = dist3(last, penultimate);
        if ds <= 1.0e-9 || de <= 1.0e-9 {
            return self.clone();
        }
        let dir_s = [
            (first[0] - second[0]) / ds,
            (first[1] - second[1]) / ds,
            (first[2] - second[2]) / ds,
        ];
        let dir_e = [
            (last[0] - penultimate[0]) / de,
            (last[1] - penultimate[1]) / de,
            (last[2] - penultimate[2]) / de,
        ];
        let mut points = Vec::with_capacity(n + 2);
        points.push([
            first[0] + dir_s[0] * by,
            first[1] + dir_s[1] * by,
            first[2] + dir_s[2] * by,
        ]);
        points.extend_from_slice(&self.points);
        points.push([
            last[0] + dir_e[0] * by,
            last[1] + dir_e[1] * by,
            last[2] + dir_e[2] * by,
        ]);
        Curve3 {
            points,
            closed: false,
            kind: SplineKind::Polyline,
        }
    }

    pub fn reverse(&self) -> Curve3 {
        let mut c = self.clone();
        c.points.reverse();
        c
    }
}

fn flatten_smooth3(curve: &Curve3, tol: f32) -> Vec<[f32; 3]> {
    let n = curve.points.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![curve.points[0]];
    }
    let segs = if curve.closed { n } else { n - 1 };
    let mut out = vec![curve.eval(0.0)];
    for i in 0..segs {
        let t0 = i as f32 / segs as f32;
        let t1 = (i + 1) as f32 / segs as f32;
        flatten_segment3(
            curve,
            t0,
            t1,
            curve.eval(t0),
            curve.eval(t1),
            tol,
            0,
            &mut out,
        );
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn flatten_segment3(
    curve: &Curve3,
    t0: f32,
    t1: f32,
    p0: [f32; 3],
    p1: [f32; 3],
    tol: f32,
    depth: usize,
    out: &mut Vec<[f32; 3]>,
) {
    let tm = (t0 + t1) * 0.5;
    let pm = curve.eval(tm);
    let chord = dist3(p0, p1);
    let deviation = if chord > 1.0e-9 {
        let d = [
            (p1[0] - p0[0]) / chord,
            (p1[1] - p0[1]) / chord,
            (p1[2] - p0[2]) / chord,
        ];
        let v = [pm[0] - p0[0], pm[1] - p0[1], pm[2] - p0[2]];
        let proj = v[0] * d[0] + v[1] * d[1] + v[2] * d[2];
        let perp = [v[0] - d[0] * proj, v[1] - d[1] * proj, v[2] - d[2] * proj];
        (perp[0] * perp[0] + perp[1] * perp[1] + perp[2] * perp[2]).sqrt()
    } else {
        dist3(pm, p0)
    };
    if depth >= 14 || deviation <= tol {
        out.push(p1);
        return;
    }
    flatten_segment3(curve, t0, tm, p0, pm, tol, depth + 1, out);
    flatten_segment3(curve, tm, t1, pm, p1, tol, depth + 1, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line2() -> Curve2 {
        Curve2 {
            points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            closed: false,
            kind: SplineKind::Polyline,
        }
    }

    #[test]
    fn polyline_eval_and_length() {
        let c = line2();
        assert_eq!(c.eval(0.0), [0.0, 0.0]);
        assert_eq!(c.eval(0.5), [1.0, 0.0]);
        assert_eq!(c.eval(1.0), [1.0, 1.0]);
        assert!((c.length() - 2.0).abs() < 1.0e-5);
    }

    #[test]
    fn resample_endpoints_open() {
        let c = line2();
        let r = c.resample(7);
        assert_eq!(r.points.len(), 7);
        assert_eq!(r.points[0], c.eval(0.0));
        assert_eq!(r.points[6], c.eval(1.0));
    }

    #[test]
    fn resample_closed_does_not_repeat() {
        let c = Curve2 {
            points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            closed: true,
            kind: SplineKind::Polyline,
        };
        let r = c.resample(8);
        assert_eq!(r.points.len(), 8);
        assert_ne!(r.points[0], r.points[7]);
        assert!((c.signed_area() - 1.0).abs() < 1.0e-5);
    }

    #[test]
    fn reverse_flips_area_sign() {
        let c = Curve2 {
            points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            closed: true,
            kind: SplineKind::Polyline,
        };
        assert!(c.signed_area() > 0.0);
        assert!(c.reverse().signed_area() < 0.0);
    }

    #[test]
    fn bezier_flatten_and_eval() {
        let c = Curve2 {
            points: vec![[0.0, 0.0], [0.0, 2.0], [2.0, 2.0], [2.0, 0.0]],
            closed: false,
            kind: SplineKind::Bezier,
        };
        assert_eq!(c.eval(0.0), [0.0, 0.0]);
        assert_eq!(c.eval(1.0), [2.0, 0.0]);
        let f = c.flatten(0.01);
        assert!(f.len() > 3);
        assert!(c.length() > 2.0);
    }

    #[test]
    fn catmull_rom_is_interpolating() {
        let c = Curve3 {
            points: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [2.0, 1.0, 0.0],
                [3.0, 0.0, 0.0],
            ],
            closed: false,
            kind: SplineKind::CatmullRom,
        };
        let third = c.eval(2.0 / 3.0);
        assert!((third[0] - 2.0).abs() < 1.0e-4);
        assert!((third[1] - 1.0).abs() < 1.0e-4);
    }

    #[test]
    fn trim_and_extend() {
        let c = Curve3 {
            points: vec![[0.0, 0.0, 0.0], [0.0, 0.0, 2.0]],
            closed: false,
            kind: SplineKind::Polyline,
        };
        let t = c.trim(0.25, 0.75);
        assert!((t.length() - 1.0).abs() < 1.0e-4);
        assert_eq!(t.points[0], [0.0, 0.0, 0.5]);
        let e = c.extend(1.0);
        assert!((e.length() - 4.0).abs() < 1.0e-4);
        assert_eq!(e.points[0], [0.0, 0.0, -1.0]);
    }

    #[test]
    fn smooth_keeps_endpoints() {
        let c = Curve3 {
            points: vec![[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [2.0, 0.0, 0.0]],
            closed: false,
            kind: SplineKind::Polyline,
        };
        let s = c.smooth(3, 0.5);
        assert_eq!(s.points[0], c.points[0]);
        assert_eq!(s.points[2], c.points[2]);
    }
}
