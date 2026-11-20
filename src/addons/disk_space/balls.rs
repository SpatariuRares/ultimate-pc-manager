//! The Balls layout (pure: no UI): packs one circle per size into an area,
//! area proportional to the size, the largest in the middle. The same
//! sizes in the same area always give the same circles, so the drawing
//! does not jump between frames while a scan fills it.

/// A circle of the layout, in the area's coordinates (0,0 = top left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ball {
    /// Centre, from the left.
    pub x: f32,
    /// Centre, from the top.
    pub y: f32,
    /// Radius.
    pub r: f32,
}

/// One circle per entry of `sizes` (same order), packed in a `width` x
/// `height` area.
pub fn pack(sizes: &[u64], width: f32, height: f32) -> Vec<Ball> {
    if sizes.is_empty() {
        return Vec::new();
    }
    // Radii in "unit" space (area = size), placed largest first, each where
    // it touches two placed balls (or one) closest to the middle. The
    // distance is stretched to the area's shape so a wide area fills wide.
    let radius: Vec<f64> = sizes.iter().map(|&s| (s as f64).sqrt()).collect();
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|&a, &b| sizes[b].cmp(&sizes[a]).then(a.cmp(&b)));
    let aspect = (width.max(1.0) / height.max(1.0)) as f64;
    let far = |(x, y): (f64, f64)| (x / aspect).powi(2) + y * y;
    let mut placed: Vec<(f64, f64, f64)> = Vec::with_capacity(sizes.len());
    let mut at = vec![(0.0, 0.0); sizes.len()];
    for &i in &order {
        let r = radius[i];
        let free = |p: (f64, f64)| placed.iter().all(|&(x, y, pr)| ((p.0 - x).powi(2) + (p.1 - y).powi(2)).sqrt() >= pr + r - 1e-6 * (pr + r));
        let mut best: Option<(f64, (f64, f64))> = if placed.is_empty() { Some((0.0, (0.0, 0.0))) } else { None };
        let consider = |p: (f64, f64), best: &mut Option<(f64, (f64, f64))>| {
            let d = far(p);
            if best.is_none_or(|(bd, _)| d < bd - 1e-9) && free(p) {
                *best = Some((d, p));
            }
        };
        for (a, &(ax, ay, ar)) in placed.iter().enumerate() {
            for k in 0..4 {
                let t = k as f64 * std::f64::consts::FRAC_PI_2;
                consider((ax + (ar + r) * t.cos(), ay + (ar + r) * t.sin()), &mut best);
            }
            for &(bx, by, br) in &placed[a + 1..] {
                for p in touching((ax, ay, ar + r), (bx, by, br + r)) {
                    consider(p, &mut best);
                }
            }
        }
        let p = best.map_or((0.0, 0.0), |(_, p)| p);
        at[i] = p;
        placed.push((p.0, p.1, r));
    }
    // Scale and centre the whole group into the area.
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (&(x, y), &r) in at.iter().zip(&radius) {
        x0 = x0.min(x - r);
        y0 = y0.min(y - r);
        x1 = x1.max(x + r);
        y1 = y1.max(y + r);
    }
    let (w, h) = ((x1 - x0).max(1e-9), (y1 - y0).max(1e-9));
    let scale = (width as f64 / w).min(height as f64 / h);
    let (ox, oy) = ((width as f64 - w * scale) / 2.0, (height as f64 - h * scale) / 2.0);
    at.iter()
        .zip(&radius)
        .map(|(&(x, y), &r)| Ball { x: ((x - x0) * scale + ox) as f32, y: ((y - y0) * scale + oy) as f32, r: (r * scale) as f32 })
        .collect()
}

/// One circle per entry of `sizes` in a `width` x `height` area, the
/// entries with a centre in `kept` (same order) left there and the others
/// (None) placed around them, closest to the middle. Only the radii follow
/// the sizes: one scale for all, the largest that keeps the balls apart and
/// inside the area (#228: the drawing does not jump while a scan fills it).
pub fn keep(kept: &[Option<(f32, f32)>], sizes: &[u64], width: f32, height: f32) -> Vec<Ball> {
    let root: Vec<f64> = sizes.iter().map(|&s| (s as f64).sqrt()).collect();
    let (w, h) = (width as f64, height as f64);
    let fixed: Vec<(usize, (f64, f64))> = (0..sizes.len()).filter_map(|i| kept.get(i).copied().flatten().map(|(x, y)| (i, (x as f64, y as f64)))).collect();
    let largest = root.iter().copied().fold(0.0, f64::max).max(1e-9);
    let mut scale = w.min(h) / 2.0 / largest;
    for (k, &(i, (x, y))) in fixed.iter().enumerate() {
        if root[i] > 0.0 {
            scale = scale.min(x.min(w - x).min(y).min(h - y).max(0.0) / root[i]);
        }
        for &(j, (x2, y2)) in &fixed[k + 1..] {
            let (sum, d) = (root[i] + root[j], ((x - x2).powi(2) + (y - y2).powi(2)).sqrt());
            if sum > 0.0 && d > 1e-6 {
                scale = scale.min(d / sum);
            }
        }
    }
    // The new balls, largest first; when one finds no room, all shrink.
    let mut new: Vec<usize> = (0..sizes.len()).filter(|&i| kept.get(i).copied().flatten().is_none()).collect();
    new.sort_by(|&a, &b| sizes[b].cmp(&sizes[a]).then(a.cmp(&b)));
    let mid = (w / 2.0, h / 2.0);
    let aspect = w.max(1.0) / h.max(1.0);
    let far = |(x, y): (f64, f64)| ((x - mid.0) / aspect).powi(2) + (y - mid.1).powi(2);
    let mut at: Vec<(f64, f64)> = vec![mid; sizes.len()];
    for &(i, p) in &fixed {
        at[i] = p;
    }
    for attempt in 0..30 {
        let mut placed: Vec<(f64, f64, f64)> = fixed.iter().map(|&(i, (x, y))| (x, y, root[i] * scale)).collect();
        let mut fits = true;
        for &i in &new {
            let r = root[i] * scale;
            let free = |p: (f64, f64)| {
                p.0 - r >= -1e-6 && p.0 + r <= w + 1e-6 && p.1 - r >= -1e-6 && p.1 + r <= h + 1e-6 && placed.iter().all(|&(x, y, pr)| ((p.0 - x).powi(2) + (p.1 - y).powi(2)).sqrt() >= pr + r - 1e-6 * (pr + r).max(1.0))
            };
            let mut best: Option<(f64, (f64, f64))> = None;
            let mut consider = |p: (f64, f64)| {
                let d = far(p);
                if best.is_none_or(|(bd, _)| d < bd - 1e-9) && free(p) {
                    best = Some((d, p));
                }
            };
            consider(mid);
            for (a, &(ax, ay, ar)) in placed.iter().enumerate() {
                for k in 0..8 {
                    let t = k as f64 * std::f64::consts::FRAC_PI_4;
                    consider((ax + (ar + r) * t.cos(), ay + (ar + r) * t.sin()));
                }
                for &(bx, by, br) in &placed[a + 1..] {
                    for p in touching((ax, ay, ar + r), (bx, by, br + r)) {
                        consider(p);
                    }
                }
            }
            match best {
                Some((_, p)) => {
                    at[i] = p;
                    placed.push((p.0, p.1, r));
                }
                None if attempt < 29 => {
                    fits = false;
                    break;
                }
                None => at[i] = mid,
            }
        }
        if fits {
            break;
        }
        scale *= 0.7;
    }
    at.iter().zip(&root).map(|(&(x, y), &r)| Ball { x: x as f32, y: y as f32, r: (r * scale) as f32 }).collect()
}

/// The points at distance `a.2` from (a.0, a.1) and `b.2` from (b.0, b.1).
fn touching(a: (f64, f64, f64), b: (f64, f64, f64)) -> Vec<(f64, f64)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let d = (dx * dx + dy * dy).sqrt();
    if d <= 1e-12 || d > a.2 + b.2 || d < (a.2 - b.2).abs() {
        return Vec::new();
    }
    let along = (a.2 * a.2 - b.2 * b.2 + d * d) / (2.0 * d);
    let side = (a.2 * a.2 - along * along).max(0.0).sqrt();
    let (mx, my) = (a.0 + along * dx / d, a.1 + along * dy / d);
    vec![(mx - side * dy / d, my + side * dx / d), (mx + side * dy / d, my - side * dx / d)]
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 800.0;
    const H: f32 = 400.0;

    fn sizes() -> Vec<u64> {
        vec![713, 300, 300, 120, 80, 64, 50, 20, 9, 5, 4, 1, 1, 400, 2]
    }

    #[test]
    fn no_two_balls_overlap_and_all_fit_in_the_area() {
        for n in [1, 2, 3, 7, sizes().len()] {
            let s = &sizes()[..n];
            let balls = pack(s, W, H);
            assert_eq!(balls.len(), n);
            for (i, a) in balls.iter().enumerate() {
                assert!(a.x - a.r >= -0.01 && a.x + a.r <= W + 0.01, "ball {i} of {n} out of the width: {a:?}");
                assert!(a.y - a.r >= -0.01 && a.y + a.r <= H + 0.01, "ball {i} of {n} out of the height: {a:?}");
                for b in &balls[i + 1..] {
                    let d = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
                    assert!(d >= a.r + b.r - 0.01, "overlap: {a:?} {b:?}");
                }
            }
        }
    }

    #[test]
    fn the_area_of_each_ball_is_proportional_to_its_size() {
        let s = sizes();
        let balls = pack(&s, W, H);
        let per_byte = balls[0].r * balls[0].r / s[0] as f32;
        for (b, &size) in balls.iter().zip(&s) {
            let ratio = b.r * b.r / size as f32 / per_byte;
            assert!((ratio - 1.0).abs() < 0.01, "size {size}: ratio {ratio}");
        }
    }

    #[test]
    fn the_largest_ball_is_in_the_middle() {
        let balls = pack(&sizes(), W, H);
        let big = balls[0];
        assert!((big.x - W / 2.0).abs() < W / 4.0 && (big.y - H / 2.0).abs() < H / 4.0, "{big:?}");
    }

    #[test]
    fn the_same_sizes_give_the_same_layout() {
        assert_eq!(pack(&sizes(), W, H), pack(&sizes(), W, H));
    }

    #[test]
    fn nothing_to_pack_gives_nothing() {
        assert!(pack(&[], W, H).is_empty());
        assert!(pack(&[5, 0], W, H).iter().all(|b| b.r.is_finite()));
    }
}
