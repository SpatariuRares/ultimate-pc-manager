//! The dark canvas: the navigation bar
//! (Parent, breadcrumbs, This Folder Only) and the Balls or Block view of
//! the level. It never changes the navigation itself: it returns the
//! [`Move`] asked for, which the add-on applies to its tree.

use egui::{vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Modifiers, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui};

use super::balls::{self, Ball};
use super::block::{self, Tile};
use super::keys::{self, Press};
use super::nav::{Crumb, Key, Move, Piece};
use super::actions::Verb;
use super::{text, Chip, Drawing, View};
use crate::theme::{self, Mode, Theme};
use crate::widgets::{dashed_rect, fit_path, icon_button, shaded_ball, Glyph};

/// The canvas is dark in both themes (spec #123, US36): its fill and
/// border are the theme's always-dark tokens, and what is drawn on it
/// takes the dark theme's words, selection and focus colours.
fn on_canvas() -> Theme {
    Theme::of(Mode::Dark)
}

/// The lowest the canvas gets when little room is left on screen.
const MIN_HEIGHT: f32 = 220.0;

/// The canvas's height for a width: a wide landscape (at most 500 points)
/// that fits in the `room` left in the visible part of the column, so the
/// pieces are not cut by the status strip; never below [`MIN_HEIGHT`].
fn height(width: f32, room: f32) -> f32 {
    (width * 0.56).min(500.0).min(room).max(MIN_HEIGHT)
}

/// What hovering a piece says: its name, then "713 GB · mostly Other"
/// (a single file says its own type).
pub fn hover_text(p: &Piece) -> String {
    let kind = match p.key {
        Key::File(_) => p.category.name().to_string(),
        _ => format!("mostly {}", p.category.name()),
    };
    format!("{}\n{} \u{00b7} {kind}", p.label, text::size(p.bytes))
}

/// One piece laid out: a ball or a block.
#[derive(Clone, Copy)]
enum Shape {
    Ball(Ball),
    Block(Tile),
}

impl Shape {
    fn centre(&self, origin: Pos2) -> Pos2 {
        match self {
            Shape::Ball(b) => origin + vec2(b.x, b.y),
            Shape::Block(t) => origin + vec2(t.x + t.w / 2.0, t.y + t.h / 2.0),
        }
    }

    fn contains(&self, origin: Pos2, p: Pos2) -> bool {
        match self {
            Shape::Ball(b) => p.distance(origin + vec2(b.x, b.y)) <= b.r,
            Shape::Block(t) => t.contains(p.x - origin.x, p.y - origin.y),
        }
    }

    /// The width a label can take and whether it is a big piece, or None
    /// when the piece is too small for one.
    fn room(&self) -> Option<(f32, bool)> {
        match self {
            Shape::Ball(b) => (b.r >= 28.0).then_some((b.r * 1.7, b.r > 70.0)),
            Shape::Block(t) => (t.w >= 60.0 && t.h >= 34.0).then_some((t.w - 10.0, t.w > 180.0 && t.h > 60.0)),
        }
    }
}

/// The empty margin kept between the canvas's edge and its pieces (it
/// also leaves room for a ball's rim and the selection's outline).
const PADDING: f32 = 10.0;

/// The pieces of `sizes` laid out in a `width` x `height` canvas, in its
/// coordinates (0,0 = top left), all inside it minus [`PADDING`].
fn layout(drawing: Drawing, sizes: &[u64], width: f32, height: f32) -> Vec<Shape> {
    let (w, h) = inside(width, height);
    match drawing {
        Drawing::Balls => balls::pack(sizes, w, h).into_iter().map(placed_ball).collect(),
        Drawing::Block => block::split(sizes, w, h).into_iter().map(|t| Shape::Block(Tile { x: t.x + PADDING, y: t.y + PADDING, ..t })).collect(),
    }
}

/// The balls of `sizes` in a `width` x `height` canvas, those with a
/// centre in `kept` (canvas coordinates) left there, the others added
/// around them (#228).
fn keep_balls(kept: &[Option<(f32, f32)>], sizes: &[u64], width: f32, height: f32) -> Vec<Shape> {
    let (w, h) = inside(width, height);
    let kept: Vec<_> = kept.iter().map(|c| c.map(|(x, y)| (x - PADDING, y - PADDING))).collect();
    balls::keep(&kept, sizes, w, h).into_iter().map(placed_ball).collect()
}

/// The area the pieces are laid out in: the canvas less [`PADDING`] all
/// around.
fn inside(width: f32, height: f32) -> (f32, f32) {
    ((width - 2.0 * PADDING).max(1.0), (height - 2.0 * PADDING).max(1.0))
}

/// A ball laid out in [`inside`]'s area, moved back to canvas coordinates.
fn placed_ball(b: Ball) -> Shape {
    Shape::Ball(Ball { x: b.x + PADDING, y: b.y + PADDING, ..b })
}

/// While a scan runs in the Balls view: the level and canvas size the
/// balls were first laid out for, and where each piece's ball is since
/// (#228: the balls keep their place, only their size changes).
struct Kept {
    level: (Vec<Crumb>, bool),
    key: [u32; 2],
    at: Vec<(Key, (f32, f32))>,
}

/// The canvas and its last layout: the same pieces in the same size and
/// view are not laid out again.
#[derive(Default)]
pub struct Canvas {
    last: Option<(Drawing, Vec<u64>, [u32; 2], Vec<Shape>)>,
    kept: Option<Kept>,
    /// The piece whose context menu is open (right-clicked).
    menu_for: Option<Key>,
    /// Where the chip was drawn last (#223).
    chip_rect: Option<Rect>,
    /// The item action asked from the chip (a click, or Enter), until taken.
    acted: Option<(Verb, Key)>,
}

impl Canvas {
    /// Draws the navigation bar and the `drawing` view of `v`'s level,
    /// with `selected` outlined; returns the move the user asked for. A
    /// right click on a piece selects it and opens `menu` for it (#90).
    pub fn show(&mut self, ui: &mut Ui, v: &View, drawing: Drawing, selected: Option<&Key>, menu: &mut dyn FnMut(&mut Ui, &Key)) -> Option<Move> {
        let mut asked = nav_bar(ui, v);
        let (rect, response) = focus_area(ui);
        let painter = ui.painter_at(rect);
        let dark = on_canvas();
        painter.rect(rect, CornerRadius::same(6), dark.canvas_bg, Stroke::new(1.0, dark.canvas_line), StrokeKind::Inside);
        let sizes: Vec<u64> = v.pieces.iter().map(|p| p.bytes).collect();
        // Shapes are laid out in the canvas's own coordinates.
        let area = rect;
        let key = [rect.width() as u32, rect.height() as u32];
        let scanning = drawing == Drawing::Balls && v.running.is_some();
        let level = (v.crumbs.clone(), v.this_folder_only);
        // The end of the scan, another level or another canvas size lays
        // the balls out afresh.
        if !(scanning && matches!(&self.kept, Some(k) if k.level == level && k.key == key)) && self.kept.take().is_some() {
            self.last = None;
        }
        let fresh = !matches!(&self.last, Some((d, s, k, _)) if *d == drawing && *s == sizes && *k == key);
        if fresh {
            let shapes = match &self.kept {
                Some(k) => {
                    let at: Vec<_> = v.pieces.iter().map(|p| k.at.iter().find(|(key, _)| *key == p.key).map(|e| e.1)).collect();
                    keep_balls(&at, &sizes, rect.width(), rect.height())
                }
                None => layout(drawing, &sizes, rect.width(), rect.height()),
            };
            if scanning {
                let at = v.pieces.iter().zip(&shapes).filter_map(|(p, s)| if let Shape::Ball(b) = s { Some((p.key.clone(), (b.x, b.y))) } else { None }).collect();
                self.kept = Some(Kept { level, key, at });
            }
            self.last = Some((drawing, sizes.clone(), key, shapes));
        }
        let laid = self.last.as_ref().map(|l| l.3.as_slice()).unwrap_or_default();
        let pointer = response.hover_pos();
        let mut hovered = None;
        for (piece, shape) in v.pieces.iter().zip(laid) {
            let over = pointer.is_some_and(|p| shape.contains(area.min, p));
            if over {
                hovered = Some(piece);
            }
            let (r, g, bl) = piece.category.colour();
            let fill = Color32::from_rgb(r, g, bl);
            // The piece under the pointer lights up.
            let fill = if over { brighter(fill) } else { fill };
            let chosen = selected == Some(&piece.key);
            match shape {
                Shape::Ball(b) => {
                    let centre = area.min + vec2(b.x, b.y);
                    let rim = if chosen { Stroke::new(3.0, dark.link) } else { Stroke::new((b.r * 0.05).max(1.0), crate::theme::SHADING.ball_line) };
                    shaded_ball(&painter, centre, b.r, fill, rim);
                }
                Shape::Block(t) => {
                    // Two points between neighbours, slightly rounded corners.
                    let r = Rect::from_min_size(area.min + vec2(t.x, t.y), vec2(t.w, t.h)).shrink(1.0);
                    let edge = if chosen { Stroke::new(3.0, dark.link) } else { Stroke::new(1.0, crate::theme::SHADING.block_line) };
                    painter.rect(r, CornerRadius::same(3), fill.gamma_multiply(0.88), edge, StrokeKind::Inside);
                }
            }
            if let Some((width, big)) = shape.room() {
                let centre = shape.centre(area.min);
                let size = if big { 13.0 } else { 11.5 };
                outlined_text(&painter, centre - vec2(0.0, size * 0.55), &short(&piece.label, width, big), FontId::proportional(size), dark.text);
                outlined_text(&painter, centre + vec2(0.0, size * 0.65), &text::size(piece.bytes), FontId::proportional(size - 1.0), dark.text);
            }
        }
        if v.pieces.is_empty() {
            painter.text(rect.center(), Align2::CENTER_CENTER, "Nothing in this folder yet.", FontId::proportional(13.0), dark.label);
        }
        // Enter shows the chip's item in File Explorer; Shift+F10 (the
        // context-menu key) opens the chip's menu.
        let (enter, menu_key) = if response.has_focus() {
            ui.input_mut(|i| (i.consume_key(Modifiers::NONE, egui::Key::Enter), i.consume_key(Modifiers::SHIFT, egui::Key::F10)))
        } else {
            (false, false)
        };
        self.chip_rect = None;
        if let Some((chip, anchor)) = chip_anchor(v, laid, area.min) {
            let r = draw_chip(ui, &painter, rect, anchor, chip);
            self.chip_rect = Some(r);
            let resp = ui.interact(r, focus_id().with("chip"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(Verb::ShowInExplorer.label());
            if resp.clicked() || enter {
                self.acted = Some((Verb::ShowInExplorer, chip.key.clone()));
            }
            let open = if resp.secondary_clicked() || menu_key {
                Some(egui::SetOpenCommand::Bool(true))
            } else if resp.clicked() {
                Some(egui::SetOpenCommand::Bool(false))
            } else {
                None
            };
            egui::Popup::menu(&resp).open_memory(open).show(|ui| menu(ui, &chip.key));
        }
        if response.secondary_clicked() {
            self.menu_for = hovered.map(|p| p.key.clone());
            if let Some(key) = &self.menu_for {
                asked = Some(Move::Select(Some(key.clone())));
            }
        }
        if let Some(key) = self.menu_for.clone() {
            response.context_menu(|ui| menu(ui, &key));
        }
        outline_focus(ui, rect, &response);
        let centres: Vec<[f32; 2]> = laid.iter().map(|s| s.centre(Pos2::ZERO)).map(|c| [c.x, c.y]).collect();
        if let Some(m) = keyboard(ui, &response, &v.pieces, &centres, selected) {
            asked = Some(m);
        }
        // Not `clicked()`: egui also counts Enter on the focused canvas as a click.
        let pointer_click = response.clicked_by(egui::PointerButton::Primary);
        if let Some(p) = hovered {
            let tip = hover_text(p);
            // A click opens the piece's chip where it was clicked; a
            // double click enters a folder (our drill-in, #80).
            if response.double_clicked_by(egui::PointerButton::Primary) {
                asked = Some(Move::Activate(p.key.clone()));
            } else if let (true, Some(at)) = (pointer_click, pointer) {
                asked = Some(Move::Pick(p.key.clone(), [at.x - area.min.x, at.y - area.min.y]));
            }
            response.on_hover_text_at_pointer(tip);
        } else if pointer_click {
            asked = Some(Move::Select(None));
        }
        asked
    }

    /// The item action asked from the chip since the last call: a click
    /// on it, or Enter on the focused canvas (Show in File Explorer).
    pub fn take_action(&mut self) -> Option<(Verb, Key)> {
        self.acted.take()
    }
}

/// The chip of `v` and where it is anchored on the canvas whose top left
/// is `origin`: the click point while it is still on its piece (the balls
/// keep their place during a scan, #228), else the piece's centre.
fn chip_anchor<'a>(v: &'a View, laid: &[Shape], origin: Pos2) -> Option<(&'a Chip, Pos2)> {
    let chip = v.chip.as_ref()?;
    let shape = v.pieces.iter().position(|p| p.key == chip.key).and_then(|i| laid.get(i))?;
    let click = chip.at.map(|[x, y]| origin + vec2(x, y)).filter(|p| shape.contains(origin, *p));
    Some((chip, click.unwrap_or_else(|| shape.centre(origin))))
}

/// The gap between the anchor and the chip, so the anchor (where a
/// double click lands) stays uncovered.
const CHIP_GAP: f32 = 12.0;

/// Draws the chip (a rounded three-line box: name; files or type and
/// size; full path) next to `anchor`, inside `canvas`; gives its rectangle.
fn draw_chip(ui: &Ui, painter: &egui::Painter, canvas: Rect, anchor: Pos2, chip: &Chip) -> Rect {
    let dark = on_canvas();
    let widest = (canvas.width() - 2.0 * PADDING - 2.0 * CHIP_GAP).clamp(80.0, 340.0);
    let (name_font, line_font, path_font) = (FontId::proportional(13.0), FontId::proportional(12.0), FontId::proportional(11.5));
    let name = fit_path(ui, &chip.name, &name_font, widest).0;
    let second = format!("{} \u{00b7} {}", chip.detail, text::size(chip.bytes));
    let path = chip.path.as_deref().map(|p| fit_path(ui, p, &path_font, widest).0);
    let mut lines = vec![(name, name_font, dark.text), (second, line_font, dark.label)];
    lines.extend(path.map(|p| (p, path_font, dark.label)));
    let galleys: Vec<_> = lines.into_iter().map(|(s, f, c)| painter.layout_no_wrap(s, f, c)).collect();
    let inner = vec2(galleys.iter().map(|g| g.size().x).fold(0.0, f32::max), galleys.iter().map(|g| g.size().y + 2.0).sum::<f32>() - 2.0);
    let size = inner + vec2(20.0, 14.0);
    // Below right of the anchor, or flipped to the other side when it
    // would leave the canvas.
    let room = canvas.shrink(4.0);
    let x = if anchor.x + CHIP_GAP + size.x <= room.right() { anchor.x + CHIP_GAP } else { anchor.x - CHIP_GAP - size.x };
    let y = if anchor.y + CHIP_GAP + size.y <= room.bottom() { anchor.y + CHIP_GAP } else { anchor.y - CHIP_GAP - size.y };
    let x = x.clamp(room.left(), (room.right() - size.x).max(room.left()));
    let y = y.clamp(room.top(), (room.bottom() - size.y).max(room.top()));
    let r = Rect::from_min_size(Pos2::new(x, y), size);
    painter.rect_filled(r.translate(vec2(0.0, 2.0)), CornerRadius::same(8), crate::theme::SHADING.chip_shadow);
    painter.rect(r, CornerRadius::same(8), dark.panel, Stroke::new(1.0, dark.control_line), StrokeKind::Inside);
    let mut at = r.min + vec2(10.0, 7.0);
    for g in galleys {
        let h = g.size().y;
        painter.galley(at, g, dark.text);
        at.y += h + 2.0;
    }
    r
}

/// The keyboard focus of the canvas, and of the veil drawn in its place
/// before the first scan (only one of them is on the page at a time).
pub fn focus_id() -> egui::Id {
    egui::Id::new("disk-space-canvas")
}

/// The veil over the empty canvas before the first scan, inviting to
/// "Measure C:\"; true when clicked, or Enter/Space while it has the focus.
pub fn veil(ui: &mut Ui, invite: &str) -> bool {
    let (rect, response) = focus_area(ui);
    let painter = ui.painter_at(rect);
    let dark = on_canvas();
    painter.rect(rect, CornerRadius::same(6), dark.canvas_bg, Stroke::new(1.0, dark.canvas_line), StrokeKind::Inside);
    let ink = dark.text;
    let title = painter.layout_no_wrap(invite.to_string(), FontId::proportional(22.0), ink);
    let hint = painter.layout_no_wrap("Click or press Enter to start".into(), FontId::proportional(13.0), dark.label);
    let size = vec2(title.size().x.max(hint.size().x) + 80.0, title.size().y + hint.size().y + 62.0);
    let card = Rect::from_center_size(rect.center(), size.min(rect.size() - vec2(32.0, 32.0)));
    painter.rect_filled(card, CornerRadius::same(12), dark.panel);
    let lit = response.hovered() || response.has_focus();
    // The disk graph's green when it is about to start.
    let edge = if lit { dark.graphs.disk } else { dark.control_line };
    dashed_rect(&painter, card, Stroke::new(1.5, edge));
    let top = card.center().y - (title.size().y + hint.size().y + 6.0) / 2.0;
    let below = top + title.size().y + 6.0;
    painter.galley(Pos2::new(card.center().x - title.size().x / 2.0, top), title, ink);
    painter.galley(Pos2::new(card.center().x - hint.size().x / 2.0, below), hint, dark.label);
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    // egui counts Enter and Space on the focused veil as a click.
    response.clicked()
}

/// The canvas rectangle, as one widget Tab can reach; a click gives it
/// the focus too.
fn focus_area(ui: &mut Ui) -> (egui::Rect, egui::Response) {
    let width = ui.available_width();
    // The room under the canvas's top in the visible part of the column,
    // measured from the column's top so scrolling does not change it.
    let below_top = ui.cursor().top() - ui.min_rect().top();
    let room = ui.clip_rect().height() - below_top - 6.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height(width, room)), Sense::hover());
    let response = ui.interact(rect, focus_id(), Sense::click());
    if response.clicked() {
        response.request_focus();
    }
    (rect, response)
}

/// Shows that the keys now go to the canvas: a ring in the focus blue.
fn outline_focus(ui: &Ui, rect: egui::Rect, response: &egui::Response) {
    if response.has_focus() {
        ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(2.0, on_canvas().link), StrokeKind::Inside);
    }
}

/// A colour a fifth brighter (the piece under the pointer).
fn brighter(c: Color32) -> Color32 {
    let up = |v: u8| (v as f32 * 1.2).min(255.0) as u8;
    Color32::from_rgb(up(c.r()), up(c.g()), up(c.b()))
}

/// Light words (`ink`) with a dark edge, readable on any piece's colour.
fn outlined_text(p: &egui::Painter, at: Pos2, words: &str, font: FontId, ink: Color32) {
    let shade = crate::theme::SHADING.halo;
    for d in [vec2(-1.0, 0.0), vec2(1.0, 0.0), vec2(0.0, -1.0), vec2(0.0, 1.0)] {
        p.text(at + d, Align2::CENTER_CENTER, words, font.clone(), shade);
    }
    p.text(at, Align2::CENTER_CENTER, words, font, ink);
}

/// What the keys pressed this frame ask for, while the canvas has the
/// focus: arrows move the selection between the `pieces` drawn at
/// `centres` (any view's layout), Backspace is Parent (Enter is the
/// chip's, see [`Canvas::show`]).
pub fn keyboard(ui: &Ui, response: &egui::Response, pieces: &[Piece], centres: &[[f32; 2]], selected: Option<&Key>) -> Option<Move> {
    if !response.has_focus() {
        return None;
    }
    // Arrows act on the canvas instead of moving the focus to another widget.
    let arrows = egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() };
    ui.memory_mut(|m| m.set_focus_lock_filter(response.id, arrows));
    const KEYS: [(egui::Key, Press); 5] = [
        (egui::Key::ArrowLeft, Press::Left),
        (egui::Key::ArrowRight, Press::Right),
        (egui::Key::ArrowUp, Press::Up),
        (egui::Key::ArrowDown, Press::Down),
        (egui::Key::Backspace, Press::Back),
    ];
    let press = ui.input_mut(|i| KEYS.iter().find(|(k, _)| i.consume_key(Modifiers::NONE, *k)).map(|&(_, p)| p))?;
    keys::on_key(press, pieces, centres, selected)
}

/// "Parent", the breadcrumbs ("C:\ › Users › spata") and "This Folder Only".
/// Past four levels the breadcrumbs show the first and the last two, with
/// "…" between.
fn nav_bar(ui: &mut Ui, v: &View) -> Option<Move> {
    let mut asked = None;
    let pal = theme::of_ui(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let can_go_up = v.crumbs.len() > 1 || v.this_folder_only;
        if ui.add_enabled_ui(can_go_up, |ui| icon_button(ui, Glyph::Up, "Parent", None)).inner.clicked() {
            asked = Some(Move::Parent);
        }
        ui.add_space(6.0);
        let n = v.crumbs.len();
        let shown: Vec<Option<usize>> = if n > 4 { vec![Some(0), None, Some(n - 2), Some(n - 1)] } else { (0..n).map(Some).collect() };
        let full = v.crumbs.last().map(|c| c.path.display().to_string()).unwrap_or_default();
        // What the crumbs may take, leaving room for "This Folder Only".
        let mut room = ui.available_width() - 150.0 - 18.0 * shown.len() as f32;
        let mut left = shown.iter().filter(|i| i.is_some()).count();
        for (j, i) in shown.into_iter().enumerate() {
            if j > 0 {
                crate::widgets::crumb_separator(ui);
            }
            let Some(i) = i else {
                ui.label(RichText::new("\u{2026}").color(pal.label)).on_hover_text(&full);
                continue;
            };
            let c = &v.crumbs[i];
            let (label, _) = fit_path(ui, &c.label, &FontId::proportional(13.0), (room / left.max(1) as f32).max(60.0));
            left = left.saturating_sub(1);
            let last = i + 1 == n && !v.this_folder_only;
            let resp = if last {
                ui.label(RichText::new(label).size(13.0).color(pal.text).strong()).on_hover_text(&full)
            } else {
                let resp = ui.add(egui::Button::new(RichText::new(label).size(13.0).color(pal.link)).frame(false)).on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    asked = Some(Move::Crumb(c.path.clone()));
                }
                resp.on_hover_text(c.path.display().to_string())
            };
            room -= resp.rect.width();
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let mut only = v.this_folder_only;
            if ui.checkbox(&mut only, "This Folder Only").changed() {
                asked = Some(Move::ThisFolderOnly(only));
            }
        });
    });
    ui.add_space(2.0);
    asked
}

/// The label cut to what fits in `width` points.
fn short(label: &str, width: f32, big: bool) -> String {
    let fits = (width / if big { 7.4 } else { 6.6 }).max(3.0) as usize;
    if label.chars().count() <= fits {
        return label.to_string();
    }
    let mut s: String = label.chars().take(fits.saturating_sub(1)).collect();
    s.push('\u{2026}');
    s
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::disk_space::kind::Category;
    use std::path::PathBuf;

    fn piece(key: Key, label: &str, bytes: u64, category: Category) -> Piece {
        Piece { key, label: label.into(), bytes, category }
    }

    #[test]
    fn hovering_says_name_size_and_prevalent_type() {
        let gib = 1u64 << 30;
        let folder = piece(Key::Folder(PathBuf::from("C:\\Users")), "Users", 713 * gib, Category::Other);
        assert_eq!(hover_text(&folder), "Users\n713 GB \u{00b7} mostly Other");
        let this = piece(Key::ThisFolder, "this folder", 2048, Category::System);
        assert_eq!(hover_text(&this), "this folder\n2 KB \u{00b7} mostly System");
        let file = piece(Key::File(PathBuf::from("C:\\a.mkv")), "a.mkv", 2048, Category::Media);
        assert_eq!(hover_text(&file), "a.mkv\n2 KB \u{00b7} Media");
    }

    #[test]
    fn every_piece_lies_inside_the_canvas_minus_its_padding_whatever_its_size() {
        // The scan of the screenshot (addons, this folder, sensors) and a
        // long tail of small folders.
        let few = [724_000, 392_000, 320_000];
        let many: Vec<u64> = (1..=40).map(|i| 1_000_000 / i).collect();
        let canvases = [(763.0, 340.0), (1600.0, 260.0), (300.0, 900.0), (120.0, 80.0), (763.0, 427.0)];
        for (w, h) in canvases {
            for sizes in [&few[..], &many[..]] {
                for d in [Drawing::Balls, Drawing::Block] {
                    let shapes = layout(d, sizes, w, h);
                    assert_eq!(shapes.len(), sizes.len());
                    for s in shapes {
                        let (x0, y0, x1, y1) = match s {
                            Shape::Ball(b) => (b.x - b.r, b.y - b.r, b.x + b.r, b.y + b.r),
                            Shape::Block(t) => (t.x, t.y, t.x + t.w, t.y + t.h),
                        };
                        let inside = x0 >= PADDING - 0.01 && y0 >= PADDING - 0.01 && x1 <= w - PADDING + 0.01 && y1 <= h - PADDING + 0.01;
                        assert!(inside, "{d:?} in {w}x{h}: ({x0}, {y0}) to ({x1}, {y1}) leaves the padded canvas");
                    }
                }
            }
        }
    }

    #[test]
    fn the_canvas_fits_in_the_room_left_on_screen() {
        // Wide window, little room under the band: the canvas takes the
        // room instead of running under the status strip.
        assert_eq!(height(763.0, 340.0), 340.0);
        // Plenty of room: a wide landscape, at most 500 points.
        assert_eq!(height(763.0, 2000.0), 763.0 * 0.56);
        assert_eq!(height(2000.0, 2000.0), 500.0);
        // Almost no room left: still a usable canvas (the column scrolls).
        assert_eq!(height(763.0, 40.0), MIN_HEIGHT);
    }

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    /// One frame of the page with `draw` in it, fed `events`.
    fn frame<R>(ctx: &egui::Context, events: Vec<egui::Event>, mut draw: impl FnMut(&mut Ui) -> R) -> Option<R> {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0));
        let input = egui::RawInput { events, screen_rect: Some(screen), ..Default::default() };
        let mut out = None;
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| out = Some(draw(ui)));
        });
        out
    }

    fn level() -> View {
        let pieces = ["a", "b", "c"]
            .iter()
            .enumerate()
            .map(|(i, n)| piece(Key::Folder(PathBuf::from(*n)), n, 1000 / (i as u64 + 1), Category::Other))
            .collect();
        View { pieces, ..Default::default() }
    }

    #[test]
    fn the_focused_canvas_answers_arrows_enter_and_backspace_in_balls_and_block() {
        for d in [Drawing::Balls, Drawing::Block] {
            focused_keys(d);
        }
    }

    fn focused_keys(d: Drawing) {
        let ctx = egui::Context::default();
        let v = level();
        let mut c = Canvas::default();
        let a = Key::Folder(PathBuf::from("a"));
        frame(&ctx, vec![], |ui| c.show(ui, &v, d, None, &mut |_, _| {}));
        ctx.memory_mut(|m| m.request_focus(focus_id()));
        frame(&ctx, vec![], |ui| c.show(ui, &v, d, None, &mut |_, _| {}));
        let first = frame(&ctx, vec![key(egui::Key::ArrowRight)], |ui| c.show(ui, &v, d, None, &mut |_, _| {})).flatten();
        assert_eq!(first, Some(Move::Select(Some(a.clone()))), "the first arrow picks the largest");
        let arrows = [egui::Key::ArrowLeft, egui::Key::ArrowRight, egui::Key::ArrowUp, egui::Key::ArrowDown];
        let moves: Vec<_> = arrows.iter().map(|&k| frame(&ctx, vec![key(k)], |ui| c.show(ui, &v, d, Some(&a), &mut |_, _| {})).flatten()).collect();
        assert!(moves.iter().any(|m| matches!(m, Some(Move::Select(Some(k))) if *k != a)), "{d:?}: some arrow leaves the largest piece: {moves:?}");
        assert!(ctx.memory(|m| m.has_focus(focus_id())), "arrows keep the focus on the canvas");
        let chosen = View { chip: Some(chip_of(&a, None)), ..v.clone() };
        let enter = frame(&ctx, vec![key(egui::Key::Enter)], |ui| c.show(ui, &chosen, d, Some(&a), &mut |_, _| {})).flatten();
        assert_eq!(enter, None, "Enter does not enter the folder");
        assert_eq!(c.take_action(), Some((Verb::ShowInExplorer, a.clone())), "{d:?}: Enter shows the chip's item in File Explorer");
        let mut menu_for = None;
        let shift_f10 = egui::Event::Key { key: egui::Key::F10, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::SHIFT };
        frame(&ctx, vec![shift_f10], |ui| c.show(ui, &chosen, d, Some(&a), &mut |_, k| menu_for = Some(k.clone())));
        assert_eq!(menu_for, Some(a.clone()), "{d:?}: the context-menu key opens the chip's menu");
        let back = frame(&ctx, vec![key(egui::Key::Backspace)], |ui| c.show(ui, &v, d, None, &mut |_, _| {})).flatten();
        assert_eq!(back, Some(Move::Parent));
    }

    #[test]
    fn keys_do_nothing_while_the_canvas_has_no_focus() {
        let ctx = egui::Context::default();
        let v = level();
        let mut c = Canvas::default();
        frame(&ctx, vec![], |ui| c.show(ui, &v, Drawing::Balls, None, &mut |_, _| {}));
        let moved = frame(&ctx, vec![key(egui::Key::ArrowRight), key(egui::Key::Backspace)], |ui| c.show(ui, &v, Drawing::Balls, None, &mut |_, _| {})).flatten();
        assert_eq!(moved, None);
    }

    #[test]
    fn the_focused_veil_starts_the_scan_on_enter() {
        let ctx = egui::Context::default();
        frame(&ctx, vec![], |ui| veil(ui, "Measure C:\\"));
        assert_eq!(frame(&ctx, vec![key(egui::Key::Enter)], |ui| veil(ui, "Measure C:\\")), Some(false), "not focused yet");
        ctx.memory_mut(|m| m.request_focus(focus_id()));
        frame(&ctx, vec![], |ui| veil(ui, "Measure C:\\"));
        assert_eq!(frame(&ctx, vec![key(egui::Key::Enter)], |ui| veil(ui, "Measure C:\\")), Some(true));
    }

    /// The rectangles' fills and outlines drawn in one frame in `mode`.
    fn rects_drawn(mode: Mode, mut draw: impl FnMut(&mut Ui)) -> Vec<(egui::Color32, egui::Color32)> {
        let ctx = egui::Context::default();
        theme::apply(&ctx, mode);
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 700.0))), ..Default::default() };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| draw(ui));
        });
        fn flat(s: &egui::Shape, out: &mut Vec<(egui::Color32, egui::Color32)>) {
            match s {
                egui::Shape::Vec(v) => v.iter().for_each(|s| flat(s, out)),
                egui::Shape::Rect(r) => out.push((r.fill, r.stroke.color)),
                _ => {}
            }
        }
        let mut all = Vec::new();
        out.shapes.iter().for_each(|c| flat(&c.shape, &mut all));
        all
    }

    #[test]
    fn the_canvas_and_its_veil_stay_dark_in_the_light_theme() {
        let dark = Theme::of(Mode::Dark);
        let surface = (dark.canvas_bg, dark.canvas_line);
        for mode in [Mode::Dark, Mode::Light] {
            let veil_rects = rects_drawn(mode, |ui| {
                veil(ui, "Measure C:\\");
            });
            assert!(veil_rects.contains(&surface), "veil, {mode:?}");
            let v = level();
            let mut c = Canvas::default();
            let canvas_rects = rects_drawn(mode, |ui| {
                c.show(ui, &v, Drawing::Balls, None, &mut |_, _| {});
            });
            assert!(canvas_rects.contains(&surface), "canvas, {mode:?}");
        }
    }

    /// A level of `named` folders (largest first, as the add-on lists
    /// them) under `crumb`, while a scan runs or once it is over.
    fn scan_level(named: &[(&str, u64)], crumb: &str, running: bool) -> View {
        let mut named = named.to_vec();
        named.sort_by(|a, b| b.1.cmp(&a.1));
        let pieces = named.iter().map(|&(n, b)| piece(Key::Folder(PathBuf::from(n)), n, b, Category::Other)).collect();
        let crumbs = vec![crate::addons::disk_space::nav::Crumb { label: crumb.into(), path: PathBuf::from(crumb) }];
        View { pieces, crumbs, running: running.then(|| "C:\\".to_string()), ..Default::default() }
    }

    /// The balls drawn for `v`, by folder name.
    fn balls_of(ctx: &egui::Context, c: &mut Canvas, v: &View) -> Vec<(String, Ball)> {
        frame(ctx, vec![], |ui| c.show(ui, v, Drawing::Balls, None, &mut |_, _| {}));
        let laid = c.last.as_ref().map(|l| l.3.clone()).unwrap_or_default();
        v.pieces
            .iter()
            .zip(laid)
            .map(|(p, s)| match s {
                Shape::Ball(b) => (p.label.clone(), b),
                Shape::Block(_) => panic!("a block in the Balls view"),
            })
            .collect()
    }

    fn ball<'a>(balls: &'a [(String, Ball)], name: &str) -> &'a Ball {
        &balls.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("no ball for {name}")).1
    }

    fn no_overlap(balls: &[(String, Ball)]) {
        for (i, (na, a)) in balls.iter().enumerate() {
            for (nb, b) in &balls[i + 1..] {
                let d = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
                assert!(d >= a.r + b.r - 0.05, "{na} and {nb} overlap: {a:?} {b:?}");
            }
        }
    }

    #[test]
    fn during_a_scan_the_balls_keep_their_place_and_only_grow_or_shrink() {
        let ctx = egui::Context::default();
        let mut c = Canvas::default();
        let first = balls_of(&ctx, &mut c, &scan_level(&[("a", 1000), ("b", 500), ("c", 300), ("d", 100)], "C:\\", true));
        // More data: "c" overtakes "a", "b" grows, and two new folders appear.
        let more = scan_level(&[("a", 1100), ("b", 900), ("c", 3000), ("d", 100), ("e", 700), ("f", 40)], "C:\\", true);
        let later = balls_of(&ctx, &mut c, &more);
        for (name, before) in &first {
            let after = ball(&later, name);
            assert!((after.x - before.x).abs() < 0.01 && (after.y - before.y).abs() < 0.01, "{name} moved: {before:?} -> {after:?}");
        }
        // The areas still follow the sizes, the new balls are drawn and nothing overlaps.
        let (a, c3) = (ball(&later, "a"), ball(&later, "c"));
        assert!(((c3.r / a.r).powi(2) - 3000.0 / 1100.0).abs() < 0.01, "areas: {a:?} {c3:?}");
        assert!(ball(&later, "e").r > 0.0 && ball(&later, "f").r > 0.0);
        no_overlap(&later);
        // Yet more data: the balls appended before keep their place too.
        let again = balls_of(&ctx, &mut c, &scan_level(&[("a", 1200), ("b", 900), ("c", 3100), ("d", 150), ("e", 800), ("f", 60)], "C:\\", true));
        for (name, before) in &later {
            let after = ball(&again, name);
            assert!((after.x - before.x).abs() < 0.01 && (after.y - before.y).abs() < 0.01, "{name} moved: {before:?} -> {after:?}");
        }
        no_overlap(&again);
    }

    #[test]
    fn the_balls_are_laid_out_afresh_when_the_scan_ends_or_the_level_changes() {
        let named = [("a", 1100), ("b", 900), ("c", 3000), ("d", 100), ("e", 700)];
        let fresh = |v: &View| {
            let ctx = egui::Context::default();
            balls_of(&ctx, &mut Canvas::default(), v)
        };
        for next in [scan_level(&named, "C:\\", false), scan_level(&named, "Users", true)] {
            let ctx = egui::Context::default();
            let mut c = Canvas::default();
            balls_of(&ctx, &mut c, &scan_level(&[("a", 1000), ("b", 500), ("c", 300), ("d", 100)], "C:\\", true));
            assert_eq!(balls_of(&ctx, &mut c, &next), fresh(&next), "running: {:?}", next.running);
        }
    }

    /// The chip of folder `key`, clicked `at` (canvas coordinates).
    fn chip_of(key: &Key, at: Option<[f32; 2]>) -> Chip {
        Chip { key: key.clone(), name: "a".into(), detail: "2 files".into(), bytes: 1000, path: Some("C:\\a".into()), at }
    }

    /// One press and release of `button` at `at`; the moves asked meanwhile.
    fn click(ctx: &egui::Context, c: &mut Canvas, v: &View, at: Pos2, button: egui::PointerButton) -> Vec<Move> {
        let press = |pressed| egui::Event::PointerButton { pos: at, button, pressed, modifiers: egui::Modifiers::NONE };
        [vec![egui::Event::PointerMoved(at)], vec![press(true)], vec![press(false)], vec![]]
            .into_iter()
            .filter_map(|events| frame(ctx, events, |ui| c.show(ui, v, Drawing::Balls, None, &mut |_, _| {})).flatten())
            .collect()
    }

    /// The canvas of one big ball "a": its centre, and a point of the canvas off the ball.
    fn one_ball(ctx: &egui::Context, c: &mut Canvas, v: &View) -> (Pos2, Pos2) {
        frame(ctx, vec![], |ui| c.show(ui, v, Drawing::Balls, None, &mut |_, _| {}));
        let Some((_, _, _, shapes)) = &c.last else { panic!("nothing laid out") };
        let Shape::Ball(b) = shapes[0] else { panic!("not a ball") };
        let origin = ctx.read_response(focus_id()).expect("the canvas").rect.min;
        (origin + vec2(b.x, b.y), origin + vec2(4.0, 4.0))
    }

    fn big_ball() -> View {
        View { pieces: vec![piece(Key::Folder(PathBuf::from("a")), "a", 1000, Category::Other)], ..Default::default() }
    }

    #[test]
    fn a_click_on_a_piece_opens_its_chip_there_a_double_click_enters_and_empty_canvas_clears() {
        let a = Key::Folder(PathBuf::from("a"));
        let ctx = egui::Context::default();
        let (mut c, v) = (Canvas::default(), big_ball());
        let (centre, off) = one_ball(&ctx, &mut c, &v);
        let origin = ctx.read_response(focus_id()).unwrap().rect.min;
        let at = centre + vec2(5.0, 3.0);
        assert_eq!(click(&ctx, &mut c, &v, at, egui::PointerButton::Primary), [Move::Pick(a.clone(), [at.x - origin.x, at.y - origin.y])]);
        let twice = click(&ctx, &mut c, &v, at, egui::PointerButton::Primary);
        assert_eq!(twice, [Move::Activate(a.clone())], "the second click of a double click enters");

        let ctx = egui::Context::default();
        let mut c = Canvas::default();
        one_ball(&ctx, &mut c, &v);
        assert_eq!(click(&ctx, &mut c, &v, off, egui::PointerButton::Primary), [Move::Select(None)], "empty canvas clears the chip");
    }

    #[test]
    fn the_chip_shows_its_item_on_a_click_and_offers_its_menu_on_a_right_click() {
        let a = Key::Folder(PathBuf::from("a"));
        let ctx = egui::Context::default();
        let mut c = Canvas::default();
        let (centre, _) = one_ball(&ctx, &mut c, &big_ball());
        let origin = ctx.read_response(focus_id()).unwrap().rect.min;
        let at = centre - origin;
        let v = View { chip: Some(chip_of(&a, Some([at.x, at.y]))), ..big_ball() };
        frame(&ctx, vec![], |ui| c.show(ui, &v, Drawing::Balls, Some(&a), &mut |_, _| {}));
        let chip = c.chip_rect.expect("the chip is drawn");
        assert!(chip.min.x >= centre.x && chip.min.y >= centre.y, "next to the click point, which stays uncovered: {chip:?}");
        assert!(click(&ctx, &mut c, &v, chip.center(), egui::PointerButton::Primary).is_empty(), "a click on the chip is no click on the canvas");
        assert_eq!(c.take_action(), Some((Verb::ShowInExplorer, a.clone())));
        let mut menu_for = None;
        let at = chip.center();
        let right = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE };
        for events in [vec![egui::Event::PointerMoved(at)], vec![right(true)], vec![right(false)]] {
            frame(&ctx, events, |ui| c.show(ui, &v, Drawing::Balls, Some(&a), &mut |_, k| menu_for = Some(k.clone())));
        }
        assert_eq!(menu_for, Some(a.clone()), "the right click opens the item's menu");
        assert_eq!(c.take_action(), None);
    }

    #[test]
    fn the_chip_stays_at_its_click_point_while_it_is_on_its_piece_else_moves_to_the_piece() {
        let a = Key::Folder(PathBuf::from("a"));
        let ctx = egui::Context::default();
        let mut c = Canvas::default();
        let (centre, off) = one_ball(&ctx, &mut c, &big_ball());
        let origin = ctx.read_response(focus_id()).unwrap().rect.min;
        let drawn_at = |c: &mut Canvas, at: Pos2| {
            let v = View { chip: Some(chip_of(&a, Some([at.x - origin.x, at.y - origin.y]))), ..big_ball() };
            frame(&ctx, vec![], |ui| c.show(ui, &v, Drawing::Balls, Some(&a), &mut |_, _| {}));
            c.chip_rect.expect("the chip is drawn").min
        };
        let on = centre + vec2(-20.0, 10.0);
        assert_eq!(drawn_at(&mut c, on), on + vec2(CHIP_GAP, CHIP_GAP), "on its ball: at the click point");
        assert_eq!(drawn_at(&mut c, off), centre + vec2(CHIP_GAP, CHIP_GAP), "off its ball (laid out again): on the ball's centre");
        let keyboard = View { chip: Some(chip_of(&a, None)), ..big_ball() };
        frame(&ctx, vec![], |ui| c.show(ui, &keyboard, Drawing::Balls, Some(&a), &mut |_, _| {}));
        assert_eq!(c.chip_rect.map(|r| r.min), Some(centre + vec2(CHIP_GAP, CHIP_GAP)), "chosen with the keyboard: on its piece");
    }
}
