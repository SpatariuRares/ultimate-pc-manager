//! The Block layout (pure: no UI): a treemap, one rectangle per size,
//! area proportional to the size, tiling the area without gaps. The same
//! sizes in the same area always give the same rectangles, so the drawing
//! does not jump between frames while a scan fills it.

/// A rectangle of the layout, in the area's coordinates (0,0 = top left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tile {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Tile {
    /// Whether the point is inside the rectangle.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// One rectangle per entry of `sizes` (same order), tiling a `width` x
/// `height` area.
pub fn split(sizes: &[u64], width: f32, height: f32) -> Vec<Tile> {
    let mut tiles = vec![Tile { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }; sizes.len()];
    let total: f64 = sizes.iter().map(|&s| s as f64).sum();
    let (width, height) = (width.max(0.0) as f64, height.max(0.0) as f64);
    if total <= 0.0 || width * height <= 0.0 {
        return tiles;
    }
    // Squarified (Bruls, Huizing, van Wijk): largest first, rows laid along
    // the short side of what is left, a row grows while that keeps its
    // worst aspect ratio from getting worse.
    let mut order: Vec<usize> = (0..sizes.len()).filter(|&i| sizes[i] > 0).collect();
    order.sort_by(|&a, &b| sizes[b].cmp(&sizes[a]).then(a.cmp(&b)));
    let per_unit = width * height / total;
    let area = |i: usize| sizes[i] as f64 * per_unit;
    let (mut x, mut y, mut w, mut h) = (0.0f64, 0.0f64, width, height);
    let mut start = 0;
    while start < order.len() {
        let side = w.min(h);
        let worst = |sum: f64, big: f64, small: f64| {
            let s2 = side * side;
            (s2 * big / (sum * sum)).max(sum * sum / (s2 * small))
        };
        let mut end = start + 1;
        let mut sum = area(order[start]);
        while end < order.len() {
            let next = area(order[end]);
            // Sizes are sorted, so the row's largest is its first, smallest its last.
            if worst(sum + next, area(order[start]), next) > worst(sum, area(order[start]), area(order[end - 1])) {
                break;
            }
            sum += next;
            end += 1;
        }
        let thick = sum / side;
        let mut along = 0.0;
        for &i in &order[start..end] {
            let len = area(i) / thick;
            tiles[i] = if w >= h {
                Tile { x: x as f32, y: (y + along) as f32, w: thick as f32, h: len as f32 }
            } else {
                Tile { x: (x + along) as f32, y: y as f32, w: len as f32, h: thick as f32 }
            };
            along += len;
        }
        if w >= h {
            x += thick;
            w -= thick;
        } else {
            y += thick;
            h -= thick;
        }
        start = end;
    }
    tiles
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
    fn no_two_blocks_overlap_and_all_fit_in_the_area() {
        for n in [1, 2, 3, 7, sizes().len()] {
            let s = &sizes()[..n];
            let tiles = split(s, W, H);
            assert_eq!(tiles.len(), n);
            for (i, a) in tiles.iter().enumerate() {
                assert!(a.x >= -0.01 && a.x + a.w <= W + 0.01, "tile {i} of {n} out of the width: {a:?}");
                assert!(a.y >= -0.01 && a.y + a.h <= H + 0.01, "tile {i} of {n} out of the height: {a:?}");
                for b in &tiles[i + 1..] {
                    let across = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
                    let down = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
                    assert!(across <= 0.01 || down <= 0.01, "overlap: {a:?} {b:?}");
                }
            }
        }
    }

    #[test]
    fn the_area_of_each_block_is_proportional_to_its_size() {
        let s = sizes();
        let total: u64 = s.iter().sum();
        for (t, &size) in split(&s, W, H).iter().zip(&s) {
            let want = W * H * size as f32 / total as f32;
            assert!((t.w * t.h - want).abs() <= want * 0.01 + 0.01, "size {size}: area {} want {want}", t.w * t.h);
        }
    }

    #[test]
    fn the_blocks_fill_the_whole_area() {
        let area: f32 = split(&sizes(), W, H).iter().map(|t| t.w * t.h).sum();
        assert!((area - W * H).abs() < W * H * 0.001, "{area}");
    }

    #[test]
    fn the_blocks_are_not_slivers() {
        // Squarified: the big pieces stay readable (no ratio worse than 1:4
        // for the four largest of a typical level).
        let s = sizes();
        let tiles = split(&s, W, H);
        for i in [0, 1, 2, 13] {
            let t = tiles[i];
            let ratio = t.w.max(t.h) / t.w.min(t.h);
            assert!(ratio < 4.0, "size {}: {t:?}", s[i]);
        }
    }

    #[test]
    fn the_same_sizes_give_the_same_layout() {
        assert_eq!(split(&sizes(), W, H), split(&sizes(), W, H));
    }

    #[test]
    fn nothing_to_split_gives_nothing() {
        assert!(split(&[], W, H).is_empty());
        let zero = split(&[5, 0], W, H);
        assert_eq!(zero.len(), 2);
        assert!(zero.iter().all(|t| t.w.is_finite() && t.h.is_finite() && t.w >= 0.0 && t.h >= 0.0));
        assert!(split(&[0, 0], W, H).iter().all(|t| t.w * t.h == 0.0));
    }

    #[test]
    fn a_point_is_inside_its_block_only() {
        let tiles = split(&[3, 1], W, H);
        let hits = |x, y| tiles.iter().filter(|t: &&Tile| t.contains(x, y)).count();
        assert_eq!(hits(10.0, 10.0), 1);
        assert_eq!(hits(W - 10.0, H - 10.0), 1);
        assert_eq!(hits(W + 10.0, 10.0), 0);
    }
}
