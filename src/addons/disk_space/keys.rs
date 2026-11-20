//! The canvas keyboard (pure: no egui): arrows move the selection to the
//! nearest piece in that direction, Backspace is "Parent" (Enter is the
//! chip's: the canvas reveals the selected item). It works on the level's
//! pieces and their centres, in the same order, so any drawing (Balls,
//! Block) can use it.

use super::nav::{Key, Move, Piece};

/// A key the canvas handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Left,
    Right,
    Up,
    Down,
    Back,
}

/// What `press` asks for on a level whose `pieces` are drawn at `centres`
/// (same order), with `selected` highlighted.
pub fn on_key(press: Press, pieces: &[Piece], centres: &[[f32; 2]], selected: Option<&Key>) -> Option<Move> {
    let at = selected.and_then(|k| pieces.iter().position(|p| &p.key == k));
    match (press, at) {
        (Press::Back, _) => Some(Move::Parent),
        // The first arrow (or one after the level changed) picks the largest.
        (_, None) => pieces.first().map(|p| Move::Select(Some(p.key.clone()))),
        (_, Some(i)) => nearest(centres, i, press).and_then(|j| pieces.get(j)).map(|p| Move::Select(Some(p.key.clone()))),
    }
}

/// The piece nearest to `centres[from]` in the direction of `press` (an
/// arrow), or None when nothing lies that way. Screen coordinates: y grows
/// downwards.
pub fn nearest(centres: &[[f32; 2]], from: usize, press: Press) -> Option<usize> {
    let [fx, fy] = *centres.get(from)?;
    let way: fn(f32, f32) -> (f32, f32) = match press {
        Press::Left => |dx, dy| (-dx, dy),
        Press::Right => |dx, dy| (dx, dy),
        Press::Up => |dx, dy| (-dy, dx),
        Press::Down => |dx, dy| (dy, dx),
        Press::Back => return None,
    };
    // Distance along the arrow, with straying sideways counted double, so
    // the piece straight ahead wins over a nearer one off to the side.
    centres
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != from)
        .map(|(i, &[x, y])| (i, way(x - fx, y - fy)))
        .filter(|&(_, (along, _))| along > 0.5)
        .map(|(i, (along, side))| (i, along + 2.0 * side.abs()))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::disk_space::kind::Category;
    use std::path::PathBuf;

    // A plus sign around the centre (0), plus one far to the right.
    //        1
    //    4   0   2      5
    //        3
    const PLUS: [[f32; 2]; 6] = [[50.0, 50.0], [50.0, 10.0], [90.0, 50.0], [50.0, 90.0], [10.0, 50.0], [200.0, 52.0]];

    #[test]
    fn arrows_find_the_nearest_piece_that_way() {
        assert_eq!(nearest(&PLUS, 0, Press::Up), Some(1));
        assert_eq!(nearest(&PLUS, 0, Press::Right), Some(2));
        assert_eq!(nearest(&PLUS, 0, Press::Down), Some(3));
        assert_eq!(nearest(&PLUS, 0, Press::Left), Some(4));
        assert_eq!(nearest(&PLUS, 2, Press::Right), Some(5));
        assert_eq!(nearest(&PLUS, 5, Press::Right), None, "nothing further right");
    }

    fn piece(key: Key) -> Piece {
        Piece { key, label: String::new(), bytes: 1, category: Category::Other }
    }

    fn folder(name: &str) -> Key {
        Key::Folder(PathBuf::from(name))
    }

    #[test]
    fn keys_move_the_selection_and_backspace_goes_up() {
        let level: Vec<Piece> = ["0", "1", "2", "3", "4", "5"].into_iter().map(|n| piece(folder(n))).collect();
        let press = |p, sel: Option<&Key>| on_key(p, &level, &PLUS, sel);
        assert_eq!(press(Press::Right, None), Some(Move::Select(Some(folder("0")))), "the first arrow picks the largest");
        assert_eq!(press(Press::Up, Some(&folder("0"))), Some(Move::Select(Some(folder("1")))));
        assert_eq!(press(Press::Right, Some(&folder("5"))), None, "the edge keeps the selection");
        assert_eq!(press(Press::Left, Some(&Key::Rest)), Some(Move::Select(Some(folder("0")))), "a selection not on the level starts over");
        assert_eq!(press(Press::Back, None), Some(Move::Parent));
        assert_eq!(on_key(Press::Down, &[], &[], None), None, "an empty level");
    }
}
