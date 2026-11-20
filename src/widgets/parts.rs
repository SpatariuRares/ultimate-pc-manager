//! Design A's shared parts, built only on the theme's tokens: the buttons
//! (primary, administrator, outlined, destructive), the scope group with
//! counts, the status band, the warm and neutral notes, the row fill with
//! its explaining line's colour and the side panel.

use egui::{vec2, Color32, CornerRadius, FontId, Painter, Pos2, Response, RichText, Stroke, Ui};

use crate::theme::{self, Theme};

/// Heights of the buttons, from Design A's "Parts".
pub const PRIMARY_HEIGHT: f32 = 38.0;
pub const ADMIN_HEIGHT: f32 = 36.0;
pub const OUTLINE_HEIGHT: f32 = 34.0;

/// The face of button words: Segoe UI 13, semibold for the main action.
fn button_font(strong: bool) -> FontId {
    if strong { FontId::new(13.0, theme::semibold()) } else { FontId::proportional(13.0) }
}

/// Adds a button of exactly `height` whose words are `text`.
fn sized_button(ui: &mut Ui, text: RichText, fill: Color32, line: Stroke, height: f32, pad: f32, enabled: bool) -> Response {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = vec2(pad, 0.0);
        ui.spacing_mut().interact_size.y = height;
        let b = egui::Button::new(text).fill(fill).stroke(line).corner_radius(CornerRadius::same(4)).min_size(vec2(0.0, height));
        ui.add_enabled(enabled, b)
    })
    .inner
}

/// The page's main action: semibold words on the primary blue, 38 high.
pub fn primary_button(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    let t = theme::of_ui(ui);
    let text = RichText::new(label).font(button_font(true)).color(t.on_primary);
    sized_button(ui, text, t.primary, Stroke::NONE, PRIMARY_HEIGHT, 16.0, enabled)
}

/// The outlined buttons, 34 high: they differ only in ink, outline, fill
/// and weight.
#[derive(Clone, Copy)]
enum Outlined {
    /// Words in a control-line outline.
    Plain,
    /// Strong words on the control fill.
    Secondary,
    /// Link words in a dim link outline.
    Link,
    /// Red words in a red outline.
    Destructive,
}

fn outlined_button(ui: &mut Ui, label: &str, kind: Outlined, enabled: bool) -> Response {
    let t = theme::of_ui(ui);
    let (ink, line, fill) = match kind {
        Outlined::Plain => (t.text, t.control_line, Color32::TRANSPARENT),
        Outlined::Secondary => (t.text, t.control_line, ui.visuals().widgets.inactive.weak_bg_fill),
        Outlined::Link => (t.link, t.link.gamma_multiply(0.45), Color32::TRANSPARENT),
        Outlined::Destructive => (t.red, t.destructive_line, Color32::TRANSPARENT),
    };
    let mut text = RichText::new(label).font(button_font(false)).color(if enabled { ink } else { t.faint });
    if matches!(kind, Outlined::Secondary) {
        text = text.strong();
    }
    sized_button(ui, text, fill, Stroke::new(1.0, line), OUTLINE_HEIGHT, 14.0, enabled)
}

/// A secondary action: words in a control-line outline, 34 high.
pub fn outline_button(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    outlined_button(ui, label, Outlined::Plain, enabled)
}

/// A destructive action: red words in a red outline, 34 high. The page
/// asks for confirmation before acting.
pub fn destructive_button(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    outlined_button(ui, label, Outlined::Destructive, enabled)
}

/// An action Windows will ask administrator permission for, with the UAC
/// shield before its words so the prompt is no surprise: the main action
/// (white shield on the primary blue, 36 high) or an outlined one (amber
/// shield, 34 high).
pub fn admin_button(ui: &mut Ui, label: &str, primary: bool, enabled: bool) -> Response {
    let t = theme::of_ui(ui);
    // Room for the shield before the words.
    let words = format!("      {label}");
    let resp = if primary {
        let text = RichText::new(words).font(button_font(true)).color(t.on_primary);
        sized_button(ui, text, t.primary, Stroke::NONE, ADMIN_HEIGHT, 14.0, enabled)
    } else {
        let text = RichText::new(words).font(button_font(false)).color(if enabled { t.text } else { t.faint });
        sized_button(ui, text, Color32::TRANSPARENT, Stroke::new(1.0, t.control_line), OUTLINE_HEIGHT, 14.0, enabled)
    };
    let ink = match (primary, enabled) {
        (true, true) => crate::widgets::icons::Ink::On(t.on_primary),
        (true, false) => crate::widgets::icons::Ink::On(t.on_primary.gamma_multiply(0.5)),
        (false, true) => crate::widgets::icons::Ink::Theme,
        (false, false) => crate::widgets::icons::Ink::Faded,
    };
    shield(ui.painter(), Pos2::new(resp.rect.left() + 18.0, resp.rect.center().y), ink);
    resp
}

/// What a scope group did this frame.
pub struct Scopes<T> {
    /// The scope clicked, if any (it may be the one already chosen).
    pub clicked: Option<T>,
    /// The whole group (for its screen-reader description).
    pub group: Response,
}

/// A group of scopes, each with its count ("Updates 42", "All 175"), the
/// chosen one (`selected`) filled in the selection blue. The page keeps the
/// choice: it changes it when `clicked` is set.
pub fn scope_group<T: PartialEq + Copy>(ui: &mut Ui, selected: T, items: &[(T, &str, usize)]) -> Scopes<T> {
    ScopeGroup::new(selected, items).show(ui)
}

/// A [`scope_group`] with more to it: one scope whose count is amber while
/// it is not chosen (it needs attention), and a scope that cannot be chosen
/// on this system, in faint words in a dashed outline. Without counts
/// ([`ScopeGroup::choice`]) it is the compact framed choice of a setting.
pub struct ScopeGroup<'a, T> {
    selected: T,
    /// Each scope's value, words and count (none for a plain choice).
    items: Vec<(T, &'a str, Option<usize>)>,
    alert: Option<T>,
    /// Its words, the note after them and its hover text.
    unavailable: Option<(&'a str, &'a str, &'a str)>,
}

impl<'a, T: PartialEq + Copy> ScopeGroup<'a, T> {
    pub fn new(selected: T, items: &[(T, &'a str, usize)]) -> Self {
        let items = items.iter().map(|&(v, l, n)| (v, l, Some(n))).collect();
        ScopeGroup { selected, items, alert: None, unavailable: None }
    }

    /// A choice between a few options without counts ("Light", "Dark"):
    /// smaller words in a tighter frame.
    pub fn choice(selected: T, items: &[(T, &'a str)]) -> Self {
        let items = items.iter().map(|&(v, l)| (v, l, None)).collect();
        ScopeGroup { selected, items, alert: None, unavailable: None }
    }

    /// Whether the scopes carry counts (else it is a plain choice).
    fn counted(&self) -> bool {
        self.items.iter().any(|i| i.2.is_some())
    }

    /// The scope whose count is amber while it is not chosen.
    pub fn alert(mut self, scope: T) -> Self {
        self.alert = Some(scope);
        self
    }

    /// A scope that cannot be chosen: its `label`, a `note` after it and
    /// the `hover` text that says why.
    pub fn unavailable(mut self, label: &'a str, note: &'a str, hover: &'a str) -> Self {
        self.unavailable = Some((label, note, hover));
        self
    }

    pub fn show(self, ui: &mut Ui) -> Scopes<T> {
        let t = theme::of_ui(ui);
        let counted = self.counted();
        // Scopes with counts: row words, 30 high; a plain choice: label words, 24 high.
        let (margin, gap, height, font) = if counted { (3, 4.0, 30.0, t.fonts.row.clone()) } else { (2, 2.0, 24.0, t.fonts.label.clone()) };
        let mut clicked = None;
        let frame = egui::Frame::new().fill(t.panel).stroke(Stroke::new(1.0, t.line)).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::same(margin)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                if !counted {
                    ui.spacing_mut().button_padding = vec2(8.0, 0.0);
                }
                // Inside a right-to-left row the scopes are added from the
                // last, so they still read left to right.
                let rtl = ui.layout().main_dir() == egui::Direction::RightToLeft;
                let order: Vec<usize> = if rtl { (0..self.items.len()).rev().collect() } else { (0..self.items.len()).collect() };
                for i in order {
                    let (value, label, count) = self.items[i];
                    let on = value == self.selected;
                    let count_ink = match (on, self.alert == Some(value)) {
                        (true, _) => t.selected_sub,
                        (false, true) => t.amber,
                        (false, false) => t.faint,
                    };
                    let mut job = egui::text::LayoutJob::default();
                    let format = |color| egui::TextFormat { font_id: font.clone(), color, ..Default::default() };
                    job.append(label, 0.0, format(if on { t.on_selected } else { t.soft }));
                    if let Some(count) = count {
                        job.append(&format!("  {count}"), 0.0, format(count_ink));
                    }
                    let fill = if on { t.selected } else { Color32::TRANSPARENT };
                    let b = ui.add(egui::Button::new(job).fill(fill).stroke(Stroke::NONE).corner_radius(CornerRadius::same(4)).min_size(vec2(0.0, height)));
                    if b.clicked() {
                        clicked = Some(value);
                    }
                }
                if let Some((label, note, hover)) = self.unavailable {
                    let mut job = egui::text::LayoutJob::default();
                    job.append(label, 0.0, egui::TextFormat { font_id: t.fonts.row.clone(), color: t.faint, ..Default::default() });
                    job.append(note, 6.0, egui::TextFormat { font_id: FontId::proportional(11.0), color: t.faint, ..Default::default() });
                    let b = egui::Button::new(job).fill(Color32::TRANSPARENT).stroke(Stroke::NONE).min_size(vec2(0.0, 30.0));
                    let r = ui.add_enabled(false, b).on_disabled_hover_text(hover);
                    crate::widgets::dashed_rect(ui.painter(), r.rect.shrink(0.5), Stroke::new(1.0, t.line));
                }
            });
        });
        Scopes { clicked, group: frame.response }
    }
}

/// What stands before a status band's headline.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mark {
    None,
    /// Work is running: a spinner.
    Busy,
    /// A coloured dot: amber when a decision waits, red for a problem.
    Dot(Color32),
    /// A green tick: nothing to fix.
    Tick,
}

/// The parts of a status band a page fills.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BandArea {
    /// Under the headline and its sub-line, beside the actions.
    Body,
    /// The right-hand column: the main action and its captions.
    Actions,
    /// Across the band, under the rest (cards, tiles).
    Below,
}

/// The status band at the top of a page that answers a question: a mark,
/// a one-sentence headline (Segoe UI Semibold 20), an optional note after
/// it and a sub-line under it, then what the page adds in each
/// [`BandArea`].
pub struct StatusBand<'a> {
    pub mark: Mark,
    pub headline: &'a str,
    pub note: Option<&'a str>,
    pub sub: Option<&'a str>,
    /// Width of the actions column.
    pub actions_width: f32,
    /// What screen readers hear for the headline, when not its words.
    pub spoken: Option<&'a str>,
}

impl<'a> StatusBand<'a> {
    pub fn new(mark: Mark, headline: &'a str) -> Self {
        StatusBand { mark, headline, note: None, sub: None, actions_width: 220.0, spoken: None }
    }

    /// What screen readers hear for the headline (a fuller summary).
    pub fn spoken(mut self, spoken: &'a str) -> Self {
        self.spoken = Some(spoken);
        self
    }

    /// Small words after the headline ("Checked today at 21:03").
    pub fn note(mut self, note: &'a str) -> Self {
        self.note = Some(note);
        self
    }

    /// The line under the headline.
    pub fn sub(mut self, sub: &'a str) -> Self {
        self.sub = Some(sub);
        self
    }

    pub fn actions_width(mut self, width: f32) -> Self {
        self.actions_width = width;
        self
    }

    /// Draws the band; `add` is called for each area in turn (body,
    /// actions, below).
    pub fn show(self, ui: &mut Ui, mut add: impl FnMut(&mut Ui, BandArea)) {
        let t = theme::of_ui(ui);
        egui::Frame::new()
            .fill(t.band)
            .stroke(Stroke::new(1.0, t.line))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(22, 18))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.horizontal_top(|ui| {
                    let left_w = (ui.available_width() - self.actions_width - 24.0).max(200.0);
                    ui.vertical(|ui| {
                        ui.set_width(left_w);
                        ui.spacing_mut().item_spacing.y = 8.0;
                        ui.horizontal_wrapped(|ui| {
                            match self.mark {
                                Mark::None => {}
                                Mark::Busy => {
                                    ui.spinner();
                                }
                                Mark::Dot(c) => {
                                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 24.0), egui::Sense::hover());
                                    ui.painter().circle_filled(r.center(), 6.0, c);
                                }
                                Mark::Tick => {
                                    let (r, _) = ui.allocate_exact_size(vec2(18.0, 24.0), egui::Sense::hover());
                                    crate::widgets::tick(ui.painter(), r.center());
                                }
                            }
                            let line = ui.label(RichText::new(self.headline).font(t.fonts.band_title.clone()).color(t.text));
                            if let Some(spoken) = self.spoken {
                                line.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, spoken));
                            }
                            if let Some(note) = self.note {
                                ui.add_space(6.0);
                                ui.label(RichText::new(note).font(t.fonts.label.clone()).color(t.label));
                            }
                        });
                        if let Some(sub) = self.sub {
                            ui.add(egui::Label::new(RichText::new(sub).size(12.5).color(t.label)).wrap());
                        }
                        add(ui, BandArea::Body);
                    });
                    ui.vertical(|ui| {
                        ui.set_width(self.actions_width);
                        ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| add(ui, BandArea::Actions));
                    });
                });
                add(ui, BandArea::Below);
            });
    }
}

/// A note on a fill, for a decision to weigh (`warm`) or for what Windows
/// can't tell (neutral).
fn note(ui: &mut Ui, text: &str, fill: Color32, ink: Color32) {
    egui::Frame::new().fill(fill).corner_radius(CornerRadius::same(5)).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(egui::Label::new(RichText::new(text).color(ink)).wrap());
    });
}

/// The warm note: something to decide, with the number that matters.
pub fn warm_note(ui: &mut Ui, text: &str) {
    let t = theme::of_ui(ui);
    note(ui, text, t.warm_note, t.warm_note_text);
}

/// The neutral note: what Windows can't tell us.
pub fn neutral_note(ui: &mut Ui, text: &str) {
    let t = theme::of_ui(ui);
    note(ui, text, t.neutral_note, t.neutral_note_text);
}

/// Paints a list row's background: the selection blue when `selected`,
/// the hover tone under the pointer, nothing otherwise.
pub fn row_fill(p: &Painter, rect: egui::Rect, selected: bool, hovered: bool) {
    let t = theme::current(p.ctx());
    if selected {
        p.rect_filled(rect, CornerRadius::same(4), t.selected);
    } else if hovered {
        p.rect_filled(rect, CornerRadius::same(4), t.hover);
    }
}

/// The colour of a row's explaining line: light blue on the selection,
/// faint otherwise.
pub fn sub_colour(t: &Theme, selected: bool) -> Color32 {
    if selected { t.selected_sub } else { t.faint }
}

/// A key-figure tile `width` wide on the panel: its label, the figure in the
/// display face (or, when `missing`, why it is missing, faint) and an
/// optional note under it.
pub fn kpi_tile(ui: &mut Ui, width: f32, label: &str, value: &str, note: Option<&str>, missing: bool) -> Response {
    tile(ui, width, label, value, note, missing, None)
}

/// A [`kpi_tile`] whose figure needs attention: the figure in `alert`
/// (amber, red) and the outline in a softer tone of it.
pub fn alert_tile(ui: &mut Ui, width: f32, label: &str, value: &str, note: Option<&str>, alert: Color32) -> Response {
    tile(ui, width, label, value, note, false, Some(alert))
}

fn tile(ui: &mut Ui, width: f32, label: &str, value: &str, note: Option<&str>, missing: bool, alert: Option<Color32>) -> Response {
    let t = theme::of_ui(ui);
    egui::Frame::new()
        .fill(t.panel)
        .stroke(Stroke::new(1.0, alert.map_or(t.line, |a| a.gamma_multiply(0.45))))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(width - 26.0);
            ui.set_min_height(58.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.add(egui::Label::new(RichText::new(label).font(t.fonts.label.clone()).color(t.label)).truncate());
                if missing {
                    ui.add(egui::Label::new(RichText::new(value).size(13.0).italics().color(t.faint)).wrap());
                } else {
                    let face = crate::widgets::display_font(ui, 17.0);
                    ui.add(egui::Label::new(RichText::new(value).font(face).color(alert.unwrap_or(t.text))).truncate());
                }
                if let Some(n) = note {
                    ui.add(egui::Label::new(RichText::new(n).size(11.5).color(t.faint)).truncate());
                }
            });
        })
        .response
}

/// Words that act like a link ("See up to 8 examples", "Dismiss"): the link
/// colour and a pointing hand under the pointer.
pub fn link_button(ui: &mut Ui, label: &str) -> Response {
    let t = theme::of_ui(ui);
    ui.add(egui::Button::new(RichText::new(label).font(t.fonts.row.clone()).color(t.link)).frame(false))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A section's title in a side panel ("File Type", "History"): Michroma 14
/// in the text colour.
pub fn section_title(ui: &mut Ui, text: &str) -> Response {
    let t = theme::of_ui(ui);
    ui.label(RichText::new(text).font(t.fonts.section_title.clone()).color(t.text))
}

/// What the side panel says while no single row is chosen.
pub const EMPTY_DETAIL: &str = "Select a single entry for detail information.";

/// The side panel beside a list: the chosen row's details and actions, or
/// its empty text.
pub struct SidePanel<'a> {
    id: &'a str,
}

impl<'a> SidePanel<'a> {
    /// `id` keeps the panel's scroll position apart from other panels'.
    pub fn new(id: &'a str) -> Self {
        SidePanel { id }
    }

    /// Fills the space given with the panel; `add` draws `item`'s details,
    /// or the empty text shows when there is no item.
    pub fn show<T>(self, ui: &mut Ui, item: Option<T>, add: impl FnOnce(&mut Ui, T)) {
        let t = theme::of_ui(ui);
        egui::Frame::new()
            .fill(t.panel)
            .stroke(Stroke::new(1.0, t.line))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(20, 18))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(ui.available_height() - 4.0);
                // Room for the scroll bar; nothing may widen the panel.
                let w = ui.available_width() - 12.0;
                egui::ScrollArea::vertical().id_salt(self.id).auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_max_width(w);
                    ui.spacing_mut().item_spacing.y = 7.0;
                    match item {
                        Some(item) => add(ui, item),
                        None => {
                            ui.add(egui::Label::new(RichText::new(EMPTY_DETAIL).color(t.label)).wrap());
                        }
                    }
                });
            });
    }
}

/// The UAC shield (the "admin-shield" glyph), centred on `c`.
pub fn shield(p: &Painter, c: Pos2, ink: crate::widgets::icons::Ink) {
    crate::widgets::icons::draw_glyph(p, c, 15.0, "admin-shield", ink);
}

/// A page header's button ("Start", "+ Run new task"): 32 high, as tall as
/// Refresh, on the control fill.
pub fn header_button(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    ui.add_enabled(enabled, egui::Button::new(label).min_size(vec2(0.0, HEADER_BUTTON_HEIGHT)))
}

pub const HEADER_BUTTON_HEIGHT: f32 = 32.0;

/// One entry of a header dropdown such as the Processes page's "Process
/// View ⌄": an item the user clicks (a toggle when `checked` is `Some`,
/// greyed when not `enabled`), or a separator line.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DropItem<T> {
    Item { id: T, label: &'static str, checked: Option<bool>, enabled: bool },
    Separator,
}

/// A page header's dropdown: a header button reading `label` with a chevron
/// after it, whose menu lists `items` in order. Returns the id of the item
/// clicked this frame. With no items the button opens nothing.
pub fn dropdown_button<T: Copy>(ui: &mut Ui, label: &str, items: &[DropItem<T>]) -> Option<T> {
    // Room after the words for the chevron glyph, as on "More actions".
    let resp = header_button(ui, &format!("{label}     "), true);
    chevron_down(ui.painter(), Pos2::new(resp.rect.right() - 16.0, resp.rect.center().y));
    if items.is_empty() {
        return None;
    }
    let mut picked = None;
    egui::Popup::menu(&resp).show(|ui| {
        for item in items {
            match *item {
                DropItem::Separator => {
                    ui.separator();
                }
                DropItem::Item { id, label, checked: Some(mut on), enabled } => {
                    if ui.add_enabled(enabled, egui::Checkbox::new(&mut on, label)).clicked() {
                        picked = Some(id);
                    }
                }
                DropItem::Item { id, label, checked: None, enabled } => {
                    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                        picked = Some(id);
                    }
                }
            }
        }
    });
    picked
}

/// The side panel's width beside a list.
pub const SIDE_PANEL_WIDTH: f32 = 340.0;

/// A list with its side panel on the right, filling the space left: `list`
/// gets the left column and its width, then `panel` the right column and
/// what `list` returned (e.g. the row chosen this frame).
pub fn list_with_panel<R>(ui: &mut Ui, list: impl FnOnce(&mut Ui, f32) -> R, panel: impl FnOnce(&mut Ui, &R)) -> R {
    const GAP: f32 = 16.0;
    // The panel's frame (margins and line) adds 42 to its content width.
    let total = ui.available_width();
    let list_w = (total - SIDE_PANEL_WIDTH - GAP - 42.0).max(320.0);
    // The panel takes the rest of the row, so it ends with the band above.
    let panel_w = (total - list_w - GAP - 2.0 * ui.spacing().item_spacing.x).max(SIDE_PANEL_WIDTH);
    let height = ui.available_height();
    ui.horizontal_top(|ui| {
        let r = ui
            .allocate_ui_with_layout(vec2(list_w, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(list_w);
                list(ui, list_w)
            })
            .inner;
        ui.add_space(GAP);
        ui.allocate_ui_with_layout(vec2(panel_w, height), egui::Layout::top_down(egui::Align::Min), |ui| panel(ui, &r));
        r
    })
    .inner
}

// ---- Design A tables (#132) ----
//
// The look of Design A's lists for an `egui_extras::TableBuilder`: no
// stripes, the hover tone under the pointer, the selection blue with light
// words, headings in the label colour over a line.

/// Height of a table row and of its heading row.
pub const TABLE_ROW_HEIGHT: f32 = 28.0;
pub const TABLE_HEADER_HEIGHT: f32 = 26.0;

/// Runs `add`, which builds a `TableBuilder` on the `Ui` it is given, with
/// Design A's list look. Leave the builder's `striped` unset.
pub fn design_table<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = theme::of_ui(ui);
    ui.scope(|ui| {
        let v = &mut ui.style_mut().visuals;
        v.striped = false;
        v.widgets.hovered.bg_fill = t.hover;
        v.selection.bg_fill = t.selected;
        v.selection.stroke.color = t.on_selected;
        // A click on a cell's words picks the row, as in the other lists
        // (selectable words would take the click for text selection).
        ui.style_mut().interaction.selectable_labels = false;
        add(ui)
    })
    .inner
}

/// A column's heading: label-coloured words over the line that closes the
/// heading row.
pub fn column_heading(ui: &mut Ui, text: &str) {
    let t = theme::of_ui(ui);
    let cell = ui.max_rect();
    let half_gap = ui.spacing().item_spacing.x / 2.0;
    ui.painter().hline((cell.left() - half_gap)..=(cell.right() + half_gap), cell.bottom() - 0.5, Stroke::new(1.0, t.line));
    ui.label(RichText::new(text).font(t.fonts.label.clone()).color(t.label));
}

/// The role of a cell's words, which sets their colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CellInk {
    /// The row's name and main values.
    Main,
    /// Secondary words (descriptions, command lines), a size smaller.
    Soft,
    /// Words that matter little (an empty or "off" state), a size smaller.
    Faint,
    /// A good state ("Running").
    Good,
}

/// A cell's words in the colour of their `ink`; in the `selected` row,
/// light on the selection blue.
pub fn cell_text(ui: &mut Ui, text: &str, ink: CellInk, selected: bool) -> Response {
    let t = theme::of_ui(ui);
    let (font, colour) = match ink {
        CellInk::Main => (t.fonts.row.clone(), if selected { t.on_selected } else { t.text }),
        CellInk::Good => (t.fonts.row.clone(), if selected { t.on_selected } else { t.green }),
        CellInk::Soft => (t.fonts.label.clone(), if selected { t.selected_sub } else { t.soft }),
        CellInk::Faint => (t.fonts.label.clone(), sub_colour(&t, selected)),
    };
    ui.add(egui::Label::new(RichText::new(text).font(font).color(colour)).truncate())
}

// ---- Design A tables, sorting and load (#131) ----
//
// Extensions of the #132 tables for a list sorted by its columns and
// lit by load (Processes), and the field cards of System Info.

/// A column's heading that sorts the list when clicked: like
/// [`column_heading`], but the sorted column (`sort` = Some(descending))
/// stands out in the text colour, semibold, with an arrow for its
/// direction. The page sorts when the response is clicked.
pub fn sort_heading(ui: &mut Ui, text: &str, sort: Option<bool>) -> Response {
    let t = theme::of_ui(ui);
    let cell = ui.max_rect();
    let half_gap = ui.spacing().item_spacing.x / 2.0;
    ui.painter().hline((cell.left() - half_gap)..=(cell.right() + half_gap), cell.bottom() - 0.5, Stroke::new(1.0, t.line));
    let words = match sort {
        Some(_) => RichText::new(text).font(FontId::new(12.0, theme::semibold())).color(t.text),
        None => RichText::new(text).font(t.fonts.label.clone()).color(t.label),
    };
    let r = ui.add(egui::Label::new(words).sense(egui::Sense::click()));
    if let Some(desc) = sort {
        sort_mark(ui.painter(), Pos2::new(r.rect.right() + 8.0, r.rect.center().y), desc);
    }
    r
}

/// The sort direction mark (the "sort-down" or "sort-up" glyph), the one
/// every sortable column heading uses.
pub fn sort_mark(p: &Painter, c: Pos2, descending: bool) {
    let name = if descending { "sort-down" } else { "sort-up" };
    crate::widgets::icons::draw_glyph(p, c, 11.0, name, crate::widgets::icons::Ink::Theme);
}

/// A dropdown's chevron (the "chevron-down" glyph), after a menu button's words.
pub fn chevron_down(p: &Painter, c: Pos2) {
    crate::widgets::icons::draw_glyph(p, c, 11.0, "chevron-down", crate::widgets::icons::Ink::Theme);
}

/// The tint of a busy cell: amber, stronger as `level` (0..=1) rises;
/// `None` while it is idle.
pub fn heat_fill(t: &Theme, level: f32) -> Option<Color32> {
    let level = level.clamp(0.0, 1.0);
    (level >= 0.02).then(|| t.amber.gamma_multiply(0.06 + 0.12 * level))
}

/// The tint of a process's CPU cell while its figures have just changed
/// (Highlight Process Updates).
pub fn lit_fill(t: &Theme) -> Color32 {
    t.green.gamma_multiply(0.22)
}

/// Lights the cell being drawn green: its row's figures just changed
/// (see [`lit_fill`]).
pub fn lit_cell(ui: &mut Ui) {
    let r = ui.max_rect().expand2(vec2(4.0, 1.0));
    ui.painter().rect_filled(r, CornerRadius::same(3), lit_fill(&theme::of_ui(ui)));
}

/// Lights the cell being drawn by how busy its figure is (see [`heat_fill`]).
pub fn heat_cell(ui: &mut Ui, level: f32) {
    if let Some(fill) = heat_fill(&theme::of_ui(ui), level) {
        let r = ui.max_rect().expand2(vec2(4.0, 1.0));
        ui.painter().rect_filled(r, CornerRadius::same(3), fill);
    }
}

/// The width a tree chevron takes in a table cell.
pub const CHEVRON_WIDTH: f32 = 14.0;

/// A tree chevron in a table cell: pointing down while `open`, right
/// otherwise. A click on it opens or closes the row.
pub fn tree_chevron(ui: &mut Ui, open: bool, selected: bool) -> Response {
    let t = theme::of_ui(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(CHEVRON_WIDTH, CHEVRON_WIDTH), egui::Sense::click());
    let c = rect.center();
    let pts = if open {
        vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)]
    } else {
        vec![c + vec2(-2.0, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.0, 4.0)]
    };
    ui.painter().add(egui::Shape::convex_polygon(pts, sub_colour(&t, selected), Stroke::NONE));
    resp
}

/// A card of named values (System Info's sections): its group in faint
/// words, its title, then each field's label over its value, in as many
/// columns of at least 190 as fit.
pub fn field_card(ui: &mut Ui, group: &str, title: &str, fields: &[(&str, String)]) {
    let t = theme::of_ui(ui);
    egui::Frame::new()
        .fill(t.panel)
        .stroke(Stroke::new(1.0, t.line))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(group).size(11.0).color(t.faint));
                ui.add_space(4.0);
                ui.label(RichText::new(title).font(FontId::new(15.0, theme::semibold())).color(t.text));
            });
            ui.add_space(4.0);
            let columns = ((ui.available_width() + 18.0) / (190.0 + 18.0)).floor().max(1.0) as usize;
            let width = (ui.available_width() - 18.0 * (columns - 1) as f32) / columns as f32;
            for chunk in fields.chunks(columns) {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 18.0;
                    for (label, value) in chunk {
                        ui.vertical(|ui| {
                            ui.set_width(width);
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.label(RichText::new(*label).font(t.fonts.label.clone()).color(t.label));
                            ui.add(egui::Label::new(RichText::new(value.as_str()).font(t.fonts.row.clone()).color(t.text)).wrap());
                        });
                    }
                });
                ui.add_space(6.0);
            }
        });
}

// ---- Design A settings parts (#133) ----
//
// The controls of Settings, Settings › Add-ons and Colors: the switch, the
// segmented choice, a setting's row, a small tag and a plain panel.

/// Size of a switch.
pub const SWITCH_SIZE: egui::Vec2 = egui::Vec2::new(40.0, 20.0);

/// An on/off switch: the primary blue with a light knob on the right when
/// on, an outline with a grey knob on the left when off. The caller flips
/// its state on `clicked()`; a disabled switch only shows it.
pub fn switch(ui: &mut Ui, on: bool, enabled: bool) -> Response {
    let t = theme::of_ui(ui);
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(SWITCH_SIZE, sense);
    let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.5) };
    let (fill, line, knob) = if on { (t.primary, t.primary, t.on_primary) } else { (Color32::TRANSPARENT, t.label, t.label) };
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(10), fade(fill), Stroke::new(1.0, fade(line)), egui::StrokeKind::Inside);
    let x = if on { rect.right() - 10.0 } else { rect.left() + 10.0 };
    p.circle_filled(Pos2::new(x, rect.center().y), 7.0, fade(knob));
    if resp.has_focus() {
        p.rect_stroke(rect.expand(2.0), CornerRadius::same(12), Stroke::new(1.5, t.link), egui::StrokeKind::Outside);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, on, if on { "On" } else { "Off" }));
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// A choice between a few options in one framed row ("Dark", "Light"),
/// the chosen one (`selected`) in the selection blue: a
/// [`ScopeGroup::choice`]. The page keeps the choice: it changes it when
/// `clicked` is set.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, selected: T, items: &[(T, &str)]) -> Scopes<T> {
    ScopeGroup::choice(selected, items).show(ui)
}

// ---- The Settings search ----
//
// Settings › General keeps what the user typed in "Find a setting" here, so
// every setting row (the app's and the add-ons') can hide itself when it
// does not match, and a card can tell how many of its rows it showed.

/// The Settings search: the lowercased query, the rows a card showed so
/// far, and whether the last row was shown (its note follows it).
#[derive(Clone, Default)]
struct SettingsFilter {
    query: String,
    shown: usize,
    last_shown: bool,
}

fn filter_id() -> egui::Id {
    egui::Id::new("settings-filter")
}

fn settings_filter(ui: &Ui) -> SettingsFilter {
    ui.ctx().data(|d| d.get_temp::<SettingsFilter>(filter_id())).unwrap_or_default()
}

fn edit_filter(ui: &Ui, edit: impl FnOnce(&mut SettingsFilter)) {
    ui.ctx().data_mut(|d| edit(d.get_temp_mut_or_default::<SettingsFilter>(filter_id())));
}

/// Sets what "Find a setting" holds; an empty `query` shows every row.
/// Settings sets it each frame, before drawing its cards.
pub fn set_settings_query(ui: &Ui, query: &str) {
    let query = query.trim().to_lowercase();
    edit_filter(ui, |f| f.query = query);
}

/// Whether a search is on (rows hide, group headings hide).
pub fn settings_searching(ui: &Ui) -> bool {
    !settings_filter(ui).query.is_empty()
}

/// Whether a setting named `name` (explained by `sub`) matches `query`
/// (lowercase): its name or its explaining line holds it.
pub fn setting_matches(query: &str, name: &str, sub: Option<&str>) -> bool {
    query.is_empty() || name.to_lowercase().contains(query) || sub.is_some_and(|s| s.to_lowercase().contains(query))
}

/// Notes the row `name` and says whether it is shown.
fn note_row(ui: &Ui, name: &str, sub: Option<&str>) -> bool {
    let mut shown = false;
    edit_filter(ui, |f| {
        shown = setting_matches(&f.query, name, sub);
        f.last_shown = shown;
        if shown {
            f.shown += 1;
        }
    });
    shown
}

// ---- Settings rows ----

/// Room left of a row that depends on the one above it.
const SUB_SETTING_INDENT: f32 = 18.0;

/// One setting: a line above it, its name and, under it, its explaining
/// line on the left, its control (`add`, laid out right to left) on the
/// right. `None` when the Settings search hides it.
pub fn setting_row<R>(ui: &mut Ui, name: &str, sub: Option<&str>, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    setting_row_at(ui, 0.0, name, sub, add)
}

/// A setting that depends on the one above it (drawn indented).
pub fn sub_setting_row<R>(ui: &mut Ui, name: &str, sub: Option<&str>, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    setting_row_at(ui, SUB_SETTING_INDENT, name, sub, add)
}

fn setting_row_at<R>(ui: &mut Ui, indent: f32, name: &str, sub: Option<&str>, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    if !note_row(ui, name, sub) {
        return None;
    }
    let t = theme::of_ui(ui);
    let top = ui.cursor().top();
    ui.painter().hline(ui.max_rect().x_range(), top, Stroke::new(1.0, t.hover));
    let inner = egui::Frame::new()
        .inner_margin(egui::Margin { left: indent as i8, right: 0, top: 9, bottom: 9 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.set_min_height(28.0);
                let control_w = 300.0_f32.min(ui.available_width() * 0.5);
                let words_w = (ui.available_width() - control_w - 16.0).max(80.0);
                ui.vertical(|ui| {
                    ui.set_width(words_w);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(egui::Label::new(RichText::new(name).font(t.fonts.row.clone()).color(t.text)).wrap());
                    if let Some(sub) = sub {
                        ui.add(egui::Label::new(RichText::new(sub).font(t.fonts.label.clone()).color(t.faint)).wrap());
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add).inner
            })
            .inner
        })
        .inner;
    Some(inner)
}

/// A setting with an on/off switch ("On" or "Off" beside it). Returns
/// whether the user flipped `on`.
pub fn setting_switch(ui: &mut Ui, name: &str, sub: Option<&str>, on: &mut bool) -> bool {
    setting_row(ui, name, sub, |ui| switch_with_state(ui, on)).unwrap_or(false)
}

/// [`setting_switch`] for a setting that depends on the one above it.
pub fn sub_setting_switch(ui: &mut Ui, name: &str, sub: Option<&str>, on: &mut bool) -> bool {
    sub_setting_row(ui, name, sub, |ui| switch_with_state(ui, on)).unwrap_or(false)
}

fn switch_with_state(ui: &mut Ui, on: &mut bool) -> bool {
    let t = theme::of_ui(ui);
    let flipped = switch(ui, *on, ui.is_enabled()).clicked();
    if flipped {
        *on = !*on;
    }
    ui.label(RichText::new(if *on { "On" } else { "Off" }).font(t.fonts.label.clone()).color(t.soft));
    flipped
}

/// A setting with a number: "−", the value (drag or type it), "+" and
/// its unit. Returns whether `value` changed; it stays in `range`.
pub fn setting_number<N: egui::emath::Numeric>(ui: &mut Ui, name: &str, sub: Option<&str>, value: &mut N, range: std::ops::RangeInclusive<N>, unit: &str) -> bool {
    setting_row(ui, name, sub, |ui| number_stepper(ui, value, range, unit)).unwrap_or(false)
}

/// [`setting_number`] for a setting that depends on the one above it.
pub fn sub_setting_number<N: egui::emath::Numeric>(ui: &mut Ui, name: &str, sub: Option<&str>, value: &mut N, range: std::ops::RangeInclusive<N>, unit: &str) -> bool {
    sub_setting_row(ui, name, sub, |ui| number_stepper(ui, value, range, unit)).unwrap_or(false)
}

/// Laid out right to left: the unit, then "+", the value and "−".
fn number_stepper<N: egui::emath::Numeric>(ui: &mut Ui, value: &mut N, range: std::ops::RangeInclusive<N>, unit: &str) -> bool {
    let t = theme::of_ui(ui);
    let (lo, hi) = (range.start().to_f64(), range.end().to_f64());
    let before = value.to_f64();
    ui.add_sized(vec2(44.0, 20.0), egui::Label::new(RichText::new(unit).font(t.fonts.label.clone()).color(t.label)));
    // A square button showing the "plus" or "minus" glyph (faded at a limit).
    let step = |ui: &mut Ui, glyph: &str, hint: &str, enabled: bool| {
        let resp = ui.add_enabled(enabled, egui::Button::new("").min_size(vec2(28.0, 28.0))).on_hover_text(hint);
        let ink = if enabled { crate::widgets::icons::Ink::Theme } else { crate::widgets::icons::Ink::Faded };
        crate::widgets::icons::draw_glyph(ui.painter(), resp.rect.center(), 12.0, glyph, ink);
        resp.clicked()
    };
    let mut now = before;
    ui.spacing_mut().item_spacing.x = 2.0;
    if step(ui, "plus", "More", now < hi) {
        now = (now + 1.0).min(hi);
    }
    let mut shown = N::from_f64(now);
    ui.add_sized(vec2(48.0, 28.0), egui::DragValue::new(&mut shown).range(range.clone()));
    now = shown.to_f64();
    if step(ui, "minus", "Less", now > lo) {
        now = (now - 1.0).max(lo);
    }
    *value = N::from_f64(now);
    now != before
}

/// What the user asked of a folder setting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FolderAct {
    None,
    Choose,
    Reset,
}

/// A folder setting: "Choose…" and, when given, its reset button
/// (`reset`: its label and whether a folder of the user's is set, else it
/// is greyed), with the folder shown in a box under the row. `enabled`
/// greys both buttons (e.g. while a test runs).
pub fn setting_folder(ui: &mut Ui, name: &str, sub: Option<&str>, folder: &str, reset: Option<(&str, bool)>, enabled: bool) -> FolderAct {
    let Some(act) = setting_row(ui, name, sub, |ui| {
        let mut act = FolderAct::None;
        if let Some((label, custom)) = reset {
            if outline_button(ui, label, enabled && custom).clicked() {
                act = FolderAct::Reset;
            }
        }
        if outline_button(ui, "Choose\u{2026}", enabled).clicked() {
            act = FolderAct::Choose;
        }
        act
    }) else {
        return FolderAct::None;
    };
    let t = theme::of_ui(ui);
    egui::Frame::new()
        .fill(t.page)
        .stroke(Stroke::new(1.0, t.line))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(folder).font(FontId::monospace(12.0)).color(t.soft)).truncate()).on_hover_text(folder);
        });
    ui.add_space(9.0);
    act
}

/// A setting with a drop-down list: `current` shown, `add` fills it.
pub fn setting_combo(ui: &mut Ui, id: &str, name: &str, sub: Option<&str>, current: &str, add: impl FnOnce(&mut Ui)) {
    setting_row(ui, name, sub, |ui| {
        egui::ComboBox::from_id_salt(id).selected_text(current).width(220.0).show_ui(ui, add);
    });
}

/// A warm note under the setting drawn just before it (hidden with it).
pub fn setting_note(ui: &mut Ui, text: &str) {
    if settings_filter(ui).last_shown {
        warm_note(ui, text);
        ui.add_space(9.0);
    }
}

/// A group of settings' heading ("UNITS"), small and in capitals; hidden
/// while searching, where rows show without their groups.
pub fn group_heading(ui: &mut Ui, title: &str) {
    if settings_searching(ui) {
        return;
    }
    let t = theme::of_ui(ui);
    ui.add_space(12.0);
    ui.label(RichText::new(title.to_uppercase()).font(FontId::new(11.5, theme::semibold())).color(t.label).extra_letter_spacing(0.6));
    ui.add_space(2.0);
}

/// What a Settings card reports.
#[derive(Clone, Copy, Default, Debug)]
pub struct CardShown {
    /// The rows it showed (all of them unless a search is on).
    pub rows: usize,
    /// Its "Open …" link was clicked.
    pub open: bool,
}

/// A Settings card: `title` in the display face, `blurb` under it, an
/// "Open …" link (`open`) on the right, then `add`'s rows. While searching
/// the card shows only its title and the rows that match, and nothing at
/// all when none does.
pub fn settings_card(ui: &mut Ui, title: &str, blurb: &str, open: Option<&str>, add: impl FnOnce(&mut Ui)) -> CardShown {
    let t = theme::of_ui(ui);
    let searching = settings_searching(ui);
    edit_filter(ui, |f| {
        f.shown = 0;
        f.last_shown = false;
    });
    let frame = egui::Frame::new()
        .fill(t.panel)
        .stroke(Stroke::new(1.0, t.line))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin { left: 20, right: 20, top: 18, bottom: 10 });
    let mut prepared = frame.begin(ui);
    let mut clicked = false;
    {
        let ui = &mut prepared.content_ui;
        ui.set_width(ui.available_width());
        let face = FontId::new(17.0, theme::display());
        if searching {
            // Painted only when a row matches: a placeholder until then.
            let galley = ui.painter().layout_no_wrap(title.to_string(), face, t.text);
            let (rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
            let slot = ui.painter().add(egui::Shape::Noop);
            add(ui);
            if settings_filter(ui).shown > 0 {
                ui.painter().set(slot, egui::Shape::galley(rect.min, galley, t.text));
            }
        } else {
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).font(face).color(t.text));
                if let Some(label) = open {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        clicked = link_button(ui, label).clicked();
                    });
                }
            });
            ui.add(egui::Label::new(RichText::new(blurb).font(t.fonts.label.clone()).color(t.label)).wrap());
            add(ui);
        }
    }
    let rows = settings_filter(ui).shown;
    if !searching || rows > 0 {
        prepared.end(ui);
        ui.add_space(14.0);
    }
    CardShown { rows, open: clicked }
}

/// One entry of the Settings category list: its name, a count of matching
/// settings on the right while searching (`count`), in the selection
/// colour when `selected`, faded when `dim`.
pub fn settings_category(ui: &mut Ui, name: &str, count: Option<usize>, selected: bool, dim: bool) -> Response {
    let t = theme::of_ui(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), egui::Sense::click());
    let p = ui.painter();
    let fill = if selected {
        t.selected
    } else if resp.hovered() {
        t.hover
    } else {
        Color32::TRANSPARENT
    };
    p.rect_filled(rect, CornerRadius::same(5), fill);
    let fade = |c: Color32| if dim { c.gamma_multiply(0.45) } else { c };
    let ink = if selected { t.on_selected } else { t.soft };
    p.text(Pos2::new(rect.left() + 12.0, rect.center().y), egui::Align2::LEFT_CENTER, name, t.fonts.row.clone(), fade(ink));
    if let Some(n) = count {
        let ink = if selected { t.selected_sub } else { t.faint };
        p.text(Pos2::new(rect.right() - 12.0, rect.center().y), egui::Align2::RIGHT_CENTER, n.to_string(), t.fonts.label.clone(), fade(ink));
    }
    if resp.has_focus() {
        p.rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.5, t.link), egui::StrokeKind::Inside);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, name));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A small heading of the Settings category list ("App", "Add-ons").
pub fn category_heading(ui: &mut Ui, text: &str) {
    let t = theme::of_ui(ui);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(RichText::new(text).font(FontId::proportional(11.5)).color(t.faint));
    });
}

/// A small rounded tag ("Free", "Paid") in `ink` on `fill`.
pub fn tag(ui: &mut Ui, text: &str, fill: Color32, ink: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(11.5), ink);
    let pad = vec2(8.0, 3.0);
    let (r, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, egui::Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same(10), fill);
    ui.painter().galley(r.min + pad, galley, ink);
    resp
}

/// A plain panel (panel fill, line, rounded) around `add`, as wide as the
/// space given.
pub fn panel<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = theme::of_ui(ui);
    egui::Frame::new()
        .fill(t.panel)
        .stroke(Stroke::new(1.0, t.line))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A two-column list of facts: label-coloured names, then their values
/// (`add` draws each value).
pub fn facts(ui: &mut Ui, names: &[&str], mut add: impl FnMut(&mut Ui, usize)) {
    let t = theme::of_ui(ui);
    ui.spacing_mut().item_spacing.y = 7.0;
    for (i, name) in names.iter().enumerate() {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.allocate_ui_with_layout(vec2(100.0, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(100.0);
                ui.label(RichText::new(*name).color(t.label));
            });
            ui.vertical(|ui| {
                ui.horizontal_wrapped(|ui| add(ui, i));
            });
        });
    }
}

// ---- Design A figure cards and accent rail (#136) ----
// Benchmarks' parts (board P15): the secondary button, the progress line,
// the bullet bar and its legend, the run sparkline, the figure card, the
// accent rail and the alert banner.

/// A secondary action: strong words on the control fill in a control-line
/// outline, 34 high ("Run All Benchmarks", "Cancel").
pub fn secondary_button(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    outlined_button(ui, label, Outlined::Secondary, enabled)
}

/// A thin (4 px) progress line across the width: the `hover` track, filled
/// to `fraction` in `color`.
pub fn progress_line(ui: &mut Ui, fraction: f32, color: Color32) {
    let t = theme::of_ui(ui);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same(2), t.hover);
    let fill = egui::Rect::from_min_max(r.min, Pos2::new(r.left() + r.width() * fraction.clamp(0.0, 1.0), r.bottom()));
    p.rect_filled(fill, CornerRadius::same(2), color);
}

/// A bullet bar: a 10 px `hover` track filled to `fill` (0..1) in `color`,
/// a solid `text` tick at `reference` and a dashed `soft` one at `best`
/// (both 0..1).
pub fn bullet_bar(ui: &mut Ui, width: f32, fill: f32, color: Color32, reference: Option<f32>, best: Option<f32>) {
    let t = theme::of_ui(ui);
    let (r, _) = ui.allocate_exact_size(vec2(width, 18.0), egui::Sense::hover());
    let track = egui::Rect::from_center_size(r.center(), vec2(width, 10.0));
    let p = ui.painter();
    p.rect_filled(track, CornerRadius::same(2), t.hover);
    let bar = egui::Rect::from_min_max(track.min, Pos2::new(track.left() + track.width() * fill.clamp(0.0, 1.0), track.bottom()));
    p.rect_filled(bar, CornerRadius::same(2), color);
    let x = |f: f32| track.left() + track.width() * f.clamp(0.0, 1.0);
    if let Some(b) = best {
        let mut y = track.top() - 3.0;
        while y < track.bottom() + 3.0 {
            let dash = egui::Rect::from_min_max(Pos2::new(x(b) - 1.0, y), Pos2::new(x(b) + 1.0, (y + 3.0).min(track.bottom() + 3.0)));
            p.rect_filled(dash, CornerRadius::ZERO, t.soft);
            y += 5.0;
        }
    }
    if let Some(m) = reference {
        p.rect_filled(egui::Rect::from_min_max(Pos2::new(x(m) - 1.0, r.top()), Pos2::new(x(m) + 1.0, r.bottom())), CornerRadius::ZERO, t.text);
    }
}

/// One entry of a bar's legend: a short swatch in `color`, then `text` in
/// the label grey.
pub fn legend_item(ui: &mut Ui, text: &str, color: Color32) {
    let t = theme::of_ui(ui);
    let (r, _) = ui.allocate_exact_size(vec2(8.0, 12.0), egui::Sense::hover());
    ui.painter().rect_filled(egui::Rect::from_center_size(r.center(), vec2(8.0, 2.0)), CornerRadius::ZERO, color);
    ui.label(RichText::new(text).size(11.5).color(t.label));
}

/// Where a run sparkline's points sit in `rect` (4 px inset): oldest on the
/// left, the lowest value at the bottom and the highest at the top; a flat
/// run sits in the middle; fewer than two values draw nothing.
pub fn spark_points(values: &[f64], rect: egui::Rect) -> Vec<Pos2> {
    if values.len() < 2 {
        return Vec::new();
    }
    let inner = rect.shrink(4.0);
    let (lo, hi) = values.iter().fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let step = inner.width() / (values.len() - 1) as f32;
    values
        .iter()
        .enumerate()
        .map(|(k, &v)| {
            let f = if hi > lo { ((v - lo) / (hi - lo)) as f32 } else { 0.5 };
            Pos2::new(inner.left() + k as f32 * step, inner.bottom() - f * inner.height())
        })
        .collect()
}

/// A small line of the last runs (`values`, oldest first) with a dot on the newest.
pub fn run_sparkline(ui: &mut Ui, size: egui::Vec2, values: &[f64], color: Color32) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let pts = spark_points(values, rect);
    let Some(&last) = pts.last() else { return };
    let p = ui.painter();
    p.add(egui::Shape::line(pts, Stroke::new(1.6, color)));
    p.circle_filled(last, 2.8, color);
}

/// What a [`figure_card`] shows at its top.
pub struct FigureCard<'a> {
    /// "Single core", after a dot in `color`.
    pub label: &'a str,
    pub color: Color32,
    /// "Score 102" (and its colour) on the right.
    pub score: Option<(String, Color32)>,
    /// The big number, in the display face, and its unit after it.
    pub value: String,
    pub unit: &'a str,
    pub value_color: Color32,
}

/// Design A's big-number card (panel fill, line border): the label in
/// `soft` with a coloured dot and the score on the right, the value large
/// in the display face with its unit in `label` grey, then `add` (bar,
/// legend, sparkline).
pub fn figure_card(ui: &mut Ui, width: f32, card: &FigureCard, add: impl FnOnce(&mut Ui)) {
    let t = theme::of_ui(ui);
    egui::Frame::new()
        .fill(t.panel)
        .stroke(Stroke::new(1.0, t.line))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin { left: 20, right: 20, top: 18, bottom: 16 })
        .show(ui, |ui| {
            ui.set_width(width - 42.0);
            ui.vertical(|ui| figure_card_body(ui, &t, card, add));
        });
}

/// The inside of a [`figure_card`], top to bottom.
fn figure_card_body(ui: &mut Ui, t: &Theme, card: &FigureCard, add: impl FnOnce(&mut Ui)) {
    ui.spacing_mut().item_spacing.y = 10.0;
    ui.horizontal(|ui| {
        crate::widgets::dot(ui, card.color);
        ui.label(RichText::new(card.label).size(13.0).color(t.soft));
        if let Some((text, color)) = &card.score {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(text).size(12.0).color(*color));
            });
        }
    });
    // The value and its unit on one baseline; a missing value ("—") in the
    // body face, which draws the dash stronger than the display face.
    let font = if card.value.chars().any(|c| c.is_ascii_digit()) { crate::widgets::display_font(ui, 38.0) } else { FontId::proportional(38.0) };
    let value = ui.painter().layout_no_wrap(card.value.clone(), font, card.value_color);
    let unit = ui.painter().layout_no_wrap(card.unit.to_string(), FontId::proportional(14.0), t.label);
    let baseline = |g: &egui::Galley| g.rows.first().and_then(|r| r.glyphs.first()).map_or(g.size().y, |gl| gl.pos.y);
    let drop = baseline(&value) - baseline(&unit);
    let (rect, _) = ui.allocate_exact_size(vec2(value.size().x + 8.0 + unit.size().x, value.size().y.max(drop + unit.size().y)), egui::Sense::hover());
    let unit_at = Pos2::new(rect.left() + value.size().x + 8.0, rect.top() + drop);
    ui.painter().galley(rect.min, value, card.value_color);
    ui.painter().galley(unit_at, unit, t.label);
    add(ui);
}

/// One entry of Design A's accent rail: its name (and a small `tag`), a
/// score on the right in the display face, and a status line under them;
/// the selected one has the [`rail_fill`] and a 3 px `link` bar on its left.
pub struct RailItem<'a> {
    pub name: &'a str,
    pub tag: Option<&'a str>,
    pub score: Option<(String, Color32)>,
    pub status: &'a str,
    pub status_color: Color32,
}

/// The fill of the selected rail entry: dark, the hover colour tinted 16 %
/// towards the selection green (#282f2b, quieter than a selected row);
/// light, the selection green itself.
pub fn rail_fill(t: &Theme) -> Color32 {
    if t.mode == theme::Mode::Light {
        return t.selected;
    }
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * 0.16).round() as u8;
    let (a, b) = (t.hover, t.selected);
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// Draws a [`RailItem`] across the width; click or keyboard selects it.
pub fn rail_item(ui: &mut Ui, item: &RailItem, selected: bool) -> Response {
    let t = theme::of_ui(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 54.0), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, item.name));
    let p = ui.painter();
    let fill = if selected {
        rail_fill(&t)
    } else if resp.hovered() {
        t.hover
    } else {
        Color32::TRANSPARENT
    };
    p.rect_filled(rect, CornerRadius::same(6), fill);
    if selected {
        let bar = egui::Rect::from_min_max(Pos2::new(rect.left(), rect.top() + 10.0), Pos2::new(rect.left() + 3.0, rect.bottom() - 10.0));
        p.rect_filled(bar, CornerRadius::same(2), t.link);
    }
    if resp.has_focus() {
        p.rect_stroke(rect.shrink(1.0), CornerRadius::same(6), Stroke::new(2.0, t.link), egui::StrokeKind::Inside);
    }
    let left = rect.left() + 16.0;
    let name = p.layout_no_wrap(item.name.to_string(), FontId::proportional(13.5), t.text);
    let name_w = name.size().x;
    p.galley(Pos2::new(left, rect.top() + 9.0), name, t.text);
    if let Some(tag) = item.tag {
        p.text(Pos2::new(left + name_w + 6.0, rect.top() + 11.0), egui::Align2::LEFT_TOP, tag, FontId::proportional(11.0), t.label);
    }
    if let Some((score, color)) = &item.score {
        p.text(Pos2::new(rect.right() - 14.0, rect.top() + 10.0), egui::Align2::RIGHT_TOP, score, crate::widgets::display_font(ui, 14.0), *color);
    }
    let status = crate::widgets::fitted(ui, item.status, 12.0, item.status_color, false, rect.width() - 30.0);
    ui.painter().galley(Pos2::new(left, rect.top() + 31.0), status, item.status_color);
    resp
}

/// The alert banner across the page: something is wrong with every result
/// ("!" and the words in `alert_note_text` on the `alert_note`, a
/// `destructive_line` border).
pub fn alert_banner(ui: &mut Ui, text: &str) {
    let t = theme::of_ui(ui);
    egui::Frame::new()
        .fill(t.alert_note)
        .stroke(Stroke::new(1.0, t.destructive_line))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(14, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
                crate::widgets::icons::draw_glyph(ui.painter(), r.center(), 15.0, "warning", crate::widgets::icons::Ink::Theme);
                ui.add(egui::Label::new(RichText::new(text).color(t.alert_note_text)).wrap());
            });
        });
}

// ---- Design A telemetry (#135) ----
// Flight Recorder's parts (its approved prototype): the marked button, the
// outlined tag and card, the ruled heading, the warm note holding controls,
// and the telemetry panel with its line chart and cell grid, both on the
// always-dark graph surface.

/// A mark drawn before a [`marked_button`]'s words.
#[derive(Clone, Copy, Debug)]
pub enum ButtonMark {
    /// The "record" glyph (a dot in a ring).
    Record,
    /// The "stop" glyph (a square in a frame).
    Stop,
}

/// A main action (38 high, semibold `on_primary` words) with a mark before
/// its words in the same ink: the "record" glyph on the primary green, the
/// "stop" glyph on the recording fill. `line` outlines it when given.
pub fn marked_button(ui: &mut Ui, label: &str, mark: ButtonMark, fill: Color32, line: Option<Color32>, enabled: bool) -> Response {
    let t = theme::of_ui(ui);
    // Room for the mark before the words.
    let text = RichText::new(format!("     {label}")).font(button_font(true)).color(t.on_primary);
    let stroke = line.map_or(Stroke::NONE, |c| Stroke::new(1.0, c));
    let resp = sized_button(ui, text, fill, stroke, PRIMARY_HEIGHT, 16.0, enabled);
    let at = Pos2::new(resp.rect.left() + 20.0, resp.rect.center().y);
    let fade = if enabled { 1.0 } else { 0.45 };
    let name = match mark {
        ButtonMark::Record => "record",
        ButtonMark::Stop => "stop",
    };
    // In the button's ink, like the UAC shield on a filled button.
    let ink = crate::widgets::icons::Ink::On(t.on_primary.gamma_multiply(fade));
    crate::widgets::icons::draw_glyph(ui.painter(), at, 15.0, name, ink);
    resp
}

/// An outlined action in the link colour ("Export as CSV"), 34 high.
pub fn link_outline_button(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    outlined_button(ui, label, Outlined::Link, enabled)
}

/// A small outlined word in `ink` ("LIVE", "RECORDING", "REPLAY"), with a
/// dot before it when `dot` is set.
pub fn outlined_tag(ui: &mut Ui, text: &str, ink: Color32, dot: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::new(11.5, theme::semibold()), ink);
    let lead = if dot { 13.0 } else { 0.0 };
    let size = vec2(galley.size().x + lead + 14.0, galley.size().y + 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter();
    p.rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, ink.gamma_multiply(0.45)), egui::StrokeKind::Inside);
    if dot {
        p.circle_filled(Pos2::new(rect.left() + 10.5, rect.center().y), 3.5, ink);
    }
    p.galley(Pos2::new(rect.left() + 7.0 + lead, rect.center().y - galley.size().y / 2.0), galley, ink);
    resp
}

/// A box in `fill` outlined in `edge`, holding `add` across the width it
/// is given (the RECORDING and REPLAY cards, an unsaved trace).
pub fn outlined_card<R>(ui: &mut Ui, fill: Color32, edge: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, edge))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A heading for a part of a side panel (semibold 13), under a `line` rule
/// across the panel.
pub fn ruled_heading(ui: &mut Ui, text: &str) {
    let t = theme::of_ui(ui);
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, t.line));
    ui.add_space(6.0);
    ui.label(RichText::new(text).font(t.fonts.panel_heading.clone()).color(t.text));
}

/// A warm note holding `add` (a reason, a question and its buttons).
pub fn warm_note_with<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = theme::of_ui(ui);
    egui::Frame::new()
        .fill(t.warm_note)
        .corner_radius(CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A telemetry panel, `width` wide: its title (semibold), `sub` beside it
/// (faint) and `value` on the right in the display face, over `add` (a
/// chart, its cells, a note), outlined in a dim `color`. The value is in
/// `color` in dark mode and in `text` in light mode, for contrast.
pub fn telemetry_panel(ui: &mut Ui, title: &str, sub: &str, value: &str, color: Color32, width: f32, add: impl FnOnce(&mut Ui)) {
    let t = theme::of_ui(ui);
    let value_ink = if t.mode == theme::Mode::Dark { color } else { t.text };
    egui::Frame::new()
        .fill(t.panel)
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.45)))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(width - 26.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).font(FontId::new(13.0, theme::semibold())).color(t.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(display_job(value, crate::widgets::display_font(ui, 13.0), value_ink));
                        if !sub.is_empty() {
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.add(egui::Label::new(RichText::new(sub).size(11.5).color(t.faint)).truncate());
                            });
                        }
                    });
                });
                add(ui);
            });
        });
}

/// `text` cut into runs: the degree signs (true) apart from the rest.
/// Michroma, the display face, draws its degree sign on the baseline, so
/// "61.0 °C" would read "61.0 oC"; the degree sign is set in the body face.
pub fn display_runs(text: &str) -> Vec<(&str, bool)> {
    let mut runs: Vec<(&str, bool)> = Vec::new();
    let mut start = 0;
    let mut degree = None;
    for (i, c) in text.char_indices() {
        let is = c == '\u{00b0}';
        if degree.is_some_and(|d| d != is) {
            runs.push((&text[start..i], degree.unwrap_or(false)));
            start = i;
        }
        degree = Some(is);
    }
    if let Some(d) = degree {
        runs.push((&text[start..], d));
    }
    runs
}

/// `text` in `font` and `color` with its degree signs in the body face at
/// the same size (see [`display_runs`]).
pub fn display_job(text: &str, font: FontId, color: Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    for (run, degree) in display_runs(text) {
        let font_id = if degree { FontId::proportional(font.size) } else { font.clone() };
        job.append(run, 0.0, egui::TextFormat { font_id, color, ..Default::default() });
    }
    job
}

/// How a [`line_chart`] line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineStyle {
    Solid,
    Dashed,
    Dotted,
}

/// One line of a [`line_chart`]: its values (oldest first) drawn against
/// `max` in `color` and `style`.
#[derive(Clone, Copy, Debug)]
pub struct ChartLine<'a> {
    pub values: &'a [f32],
    pub max: f32,
    pub style: LineStyle,
    pub color: Color32,
}

/// A chart of `lines`, `size` big, on the always-dark graph surface; each
/// line is squeezed into the width (see [`chart_points`]) and drawn against
/// its own maximum.
pub fn line_chart(ui: &mut Ui, size: egui::Vec2, lines: &[ChartLine]) {
    let (t, dark) = (theme::of_ui(ui), Theme::of(theme::Mode::Dark));
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(4), t.graph_bg, Stroke::new(1.0, dark.well_line), egui::StrokeKind::Inside);
    let inner = rect.shrink2(vec2(1.0, 2.0));
    for line in lines {
        let pts = chart_points(inner, line.values, line.max);
        if pts.len() < 2 {
            continue;
        }
        match line.style {
            LineStyle::Solid => {
                p.add(egui::Shape::line(pts, Stroke::new(1.8, line.color)));
            }
            LineStyle::Dashed => {
                p.extend(egui::Shape::dashed_line(&pts, Stroke::new(1.2, line.color), 4.0, 3.0));
            }
            LineStyle::Dotted => {
                p.extend(egui::Shape::dashed_line(&pts, Stroke::new(1.0, line.color), 1.0, 3.0));
            }
        }
    }
}

/// `data` as points across `rect` against `max`, at most one per pixel
/// (the busiest of the samples a pixel covers).
pub fn chart_points(rect: egui::Rect, data: &[f32], max: f32) -> Vec<Pos2> {
    let n = data.len();
    let cols = (rect.width() as usize).max(1).min(n);
    (0..cols)
        .map(|c| {
            let a = c * n / cols;
            let b = ((c + 1) * n / cols).max(a + 1);
            let v = data[a..b].iter().copied().fold(0.0f32, f32::max);
            let x = rect.left() + rect.width() * c as f32 / (cols - 1).max(1) as f32;
            Pos2::new(x, rect.bottom() - (v / max.max(0.0001)).clamp(0.0, 1.0) * rect.height())
        })
        .collect()
}

/// The fill of a cell `level` (0..=100) busy in `color`: the `well` when
/// idle, else `color` more opaque the busier it is.
pub fn cell_fill(level: f32, color: Color32, well: Color32) -> Color32 {
    if level <= 0.0 {
        return well;
    }
    let alpha = (40.0 + level.min(100.0) * 2.1).round() as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// One small cell per core, engine or sensor on the always-dark well,
/// wrapped across the width, brighter when busier; each says its name under
/// the pointer. The cell `ringed` gives (the hottest sensor) is ringed in
/// its colour.
pub fn cell_grid(ui: &mut Ui, levels: &[f32], names: &[String], color: Color32, ringed: Option<(usize, Color32)>) {
    let dark = Theme::of(theme::Mode::Dark);
    let (w, h, gap) = (16.0, 12.0, 3.0);
    let per_row = ((ui.available_width() + gap) / (w + gap)).floor().max(1.0) as usize;
    let rows = levels.len().div_ceil(per_row);
    if rows == 0 {
        return;
    }
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), rows as f32 * (h + gap) - gap), egui::Sense::hover());
    let p = ui.painter();
    let mut hovered = None;
    for (i, &level) in levels.iter().enumerate() {
        let min = Pos2::new(rect.left() + (i % per_row) as f32 * (w + gap), rect.top() + (i / per_row) as f32 * (h + gap));
        let cell = egui::Rect::from_min_size(min, vec2(w, h));
        // The well under the colour, so a cell reads the same on a light panel.
        p.rect_filled(cell, CornerRadius::same(2), dark.well);
        p.rect(cell, CornerRadius::same(2), cell_fill(level, color, dark.well), Stroke::new(1.0, dark.well_line), egui::StrokeKind::Inside);
        if let Some((_, ring)) = ringed.filter(|(at, _)| *at == i) {
            p.rect_stroke(cell, CornerRadius::same(2), Stroke::new(1.5, ring), egui::StrokeKind::Outside);
        }
        if resp.hover_pos().is_some_and(|at| cell.contains(at)) {
            hovered = names.get(i);
        }
    }
    if let Some(name) = hovered {
        resp.on_hover_text(name.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{self, Mode};
    use egui::{Event, Pos2, Rect, Ui};

    /// One frame of a 900x700 page, with `events`.
    fn frame(ctx: &egui::Context, events: Vec<Event>, draw: impl FnMut(&mut Ui)) -> egui::FullOutput {
        crate::test_ui::run(ctx, (900.0, 700.0), events, draw)
    }

    use crate::test_ui::context;

    /// A Settings card with two groups as add-ons draw them, under `query`.
    fn settings_demo(ui: &mut Ui, query: &str, tile: &mut bool, years: &mut i64) -> CardShown {
        set_settings_query(ui, query);
        settings_card(ui, "Drivers", "Installed drivers.", Some("Open Drivers"), |ui| {
            group_heading(ui, "Updates");
            setting_number(ui, "Suggest the maker's site for drivers older than", None, years, 1..=20, "years");
            setting_note(ui, "A note under the years.");
            group_heading(ui, "Summary");
            setting_switch(ui, "Show a tile on Summary", Some("Newer drivers, after you check"), tile);
        })
    }

    /// Where the text `word` was drawn in `out`.
    fn place(out: &egui::FullOutput, word: &str) -> Pos2 {
        crate::test_ui::flat_shapes(out)
            .iter()
            .find_map(|s| match s {
                egui::Shape::Text(t) if t.galley.text() == word => Some(t.pos + t.galley.rect.center().to_vec2()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{word:?} is drawn"))
    }

    /// Clicks at `at` over three frames of `draw`.
    fn click_at(ctx: &egui::Context, at: Pos2, mut draw: impl FnMut(&mut Ui)) {
        let press = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(ctx, vec![Event::PointerMoved(at), press(true)], &mut draw);
        frame(ctx, vec![press(false)], &mut draw);
    }

    #[test]
    fn a_setting_matches_its_name_or_its_explaining_line_in_any_case() {
        assert!(setting_matches("", "Always on top", None));
        assert!(setting_matches("top", "Always on top", None));
        assert!(setting_matches("summary", "Show a tile", Some("While Summary is open")));
        assert!(!setting_matches("disk", "Always on top", Some("Keep the app above")));
    }

    #[test]
    fn without_a_search_a_card_shows_its_title_blurb_link_groups_and_every_row() {
        let ctx = context(Mode::Dark);
        let (mut tile, mut years, mut shown) = (false, 3, CardShown::default());
        let out = frame(&ctx, vec![], |ui| shown = settings_demo(ui, "", &mut tile, &mut years));
        let words = crate::test_ui::texts(&out);
        for w in ["Drivers", "Installed drivers.", "Open Drivers", "UPDATES", "SUMMARY", "Show a tile on Summary", "Off", "years", "A note under the years."] {
            assert!(words.iter().any(|x| x == w), "{w:?} in {words:?}");
        }
        assert_eq!(shown.rows, 2);
    }

    #[test]
    fn a_search_shows_only_matching_rows_without_their_groups_and_hides_a_card_with_none() {
        let ctx = context(Mode::Dark);
        let (mut tile, mut years, mut shown) = (false, 3, CardShown::default());
        let out = frame(&ctx, vec![], |ui| shown = settings_demo(ui, "  TILE ", &mut tile, &mut years));
        let words = crate::test_ui::texts(&out);
        assert_eq!(shown.rows, 1);
        assert!(words.iter().any(|w| w == "Drivers") && words.iter().any(|w| w == "Show a tile on Summary"), "{words:?}");
        for hidden in ["UPDATES", "SUMMARY", "years", "A note under the years.", "Open Drivers", "Installed drivers."] {
            assert!(!words.iter().any(|w| w == hidden), "{hidden:?} is hidden: {words:?}");
        }
        let out = frame(&ctx, vec![], |ui| shown = settings_demo(ui, "bluetooth", &mut tile, &mut years));
        assert_eq!(shown.rows, 0);
        assert!(crate::test_ui::texts(&out).is_empty(), "nothing of the card is drawn");
    }

    #[test]
    fn the_switch_flips_its_setting_and_says_on_or_off() {
        let ctx = context(Mode::Dark);
        let (mut tile, mut years) = (false, 3);
        let out = frame(&ctx, vec![], |ui| {
            settings_demo(ui, "", &mut tile, &mut years);
        });
        let off = place(&out, "Off");
        // The switch sits right of its "Off".
        click_at(&ctx, off + egui::vec2(36.0, 0.0), |ui| {
            settings_demo(ui, "", &mut tile, &mut years);
        });
        assert!(tile, "flipped on");
        let out = frame(&ctx, vec![], |ui| {
            settings_demo(ui, "", &mut tile, &mut years);
        });
        assert!(crate::test_ui::texts(&out).iter().any(|w| w == "On"));
    }

    #[test]
    fn the_number_steps_by_one_and_stays_in_its_range() {
        let ctx = context(Mode::Dark);
        let (mut tile, mut years) = (false, 19);
        let out = frame(&ctx, vec![], |ui| {
            settings_demo(ui, "", &mut tile, &mut years);
        });
        // The stepper's two glyphs: "minus" on the left, "plus" on the right.
        let mut steps: Vec<Pos2> = crate::test_ui::flat_shapes(&out)
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => Some(m.calc_bounds().center()),
                _ => None,
            })
            .collect();
        steps.sort_by(|a, b| a.x.total_cmp(&b.x));
        let (minus, plus) = (steps[steps.len() - 2], steps[steps.len() - 1]);
        for _ in 0..3 {
            click_at(&ctx, plus, |ui| {
                settings_demo(ui, "", &mut tile, &mut years);
            });
        }
        assert_eq!(years, 20, "stops at the top of 1..=20");
        years = 1;
        click_at(&ctx, minus, |ui| {
            settings_demo(ui, "", &mut tile, &mut years);
        });
        assert_eq!(years, 1, "stops at the bottom");
    }

    #[test]
    fn a_folder_setting_offers_choose_and_its_reset_only_for_a_folder_of_the_users() {
        let ctx = context(Mode::Dark);
        let mut enabled = Vec::new();
        let out = frame(&ctx, vec![], |ui| {
            setting_folder(ui, "History folder", None, r"C:\Scans", Some(("Reset to default", false)), true);
        });
        let words = crate::test_ui::texts(&out);
        assert!(words.iter().any(|w| w == r"C:\Scans") && words.iter().any(|w| w == "Choose\u{2026}"), "{words:?}");
        for (custom, want) in [(false, false), (true, true)] {
            let out = frame(&ctx, vec![], |ui| {
                setting_folder(ui, "History folder", None, r"C:\Scans", Some(("Reset to default", custom)), true);
            });
            let reset = place(&out, "Reset to default");
            let mut act = FolderAct::None;
            click_at(&ctx, reset, |ui| {
                let got = setting_folder(ui, "History folder", None, r"C:\Scans", Some(("Reset to default", custom)), true);
                if got != FolderAct::None {
                    act = got;
                }
            });
            enabled.push(act == FolderAct::Reset);
            assert_eq!(act == FolderAct::Reset, want);
        }
        assert_eq!(enabled, [false, true]);
    }

    #[test]
    fn buttons_have_design_a_heights() {
        let ctx = context(Mode::Dark);
        let mut heights = Vec::new();
        frame(&ctx, vec![], |ui| {
            heights = vec![
                primary_button(ui, "Update", true).rect.height(),
                admin_button(ui, "Install update", true, true).rect.height(),
                admin_button(ui, "Unload", false, true).rect.height(),
                outline_button(ui, "Modify", true).rect.height(),
                destructive_button(ui, "Uninstall", true).rect.height(),
            ];
        });
        assert_eq!(heights, vec![38.0, 36.0, 34.0, 34.0, 34.0]);
    }

    /// Every shape drawn, nested ones flattened.
    fn shapes(out: &egui::FullOutput) -> Vec<egui::Shape> {
        crate::test_ui::flat_shapes(out)
    }

    /// The middle of the text shape that reads `words` (spaces trimmed).
    fn text_at(out: &egui::FullOutput, words: &str) -> Option<Pos2> {
        shapes(out).iter().find_map(|s| match s {
            egui::Shape::Text(t) if t.galley.text().trim() == words => Some(t.visual_bounding_rect().center()),
            _ => None,
        })
    }

    #[test]
    fn a_header_dropdown_lists_its_items_and_tells_which_was_clicked() {
        let ctx = context(Mode::Dark);
        let items = [
            DropItem::Item { id: 1, label: "Process tree", checked: Some(true), enabled: true },
            DropItem::Separator,
            DropItem::Item { id: 2, label: "Expand all", checked: None, enabled: true },
        ];
        let mut picked = None;
        let out = frame(&ctx, vec![], |ui| picked = dropdown_button(ui, "Process View", &items));
        let button = text_at(&out, "Process View").expect("the button reads Process View");
        assert_eq!(picked, None);
        click(&ctx, button, |ui| dropdown_button(ui, "Process View", &items));
        let out = frame(&ctx, vec![], |ui| picked = dropdown_button(ui, "Process View", &items));
        let expand = text_at(&out, "Expand all").expect("the open menu lists its items");
        assert!(text_at(&out, "Process tree").is_some());
        assert_eq!(click(&ctx, expand, |ui| dropdown_button(ui, "Process View", &items)), Some(2));
    }

    #[test]
    fn a_header_dropdown_without_items_opens_nothing() {
        let ctx = context(Mode::Dark);
        let none: [DropItem<u8>; 0] = [];
        let out = frame(&ctx, vec![], |ui| {
            dropdown_button(ui, "Process View", &none);
        });
        let button = text_at(&out, "Process View").expect("the button is drawn");
        click(&ctx, button, |ui| dropdown_button(ui, "Process View", &none));
        frame(&ctx, vec![], |ui| {
            dropdown_button(ui, "Process View", &none);
        });
        assert!(!egui::Popup::is_any_open(&ctx), "no menu opened");
    }

    /// The fill of the rectangle drawn exactly at `rect`.
    fn fill_at(out: &egui::FullOutput, rect: Rect) -> Option<egui::Color32> {
        shapes(out).iter().find_map(|s| match s {
            egui::Shape::Rect(r) if r.rect == rect => Some(r.fill),
            _ => None,
        })
    }

    /// The colour of the first text drawn that contains `words`.
    fn text_colour(out: &egui::FullOutput, words: &str) -> Option<egui::Color32> {
        shapes(out).iter().find_map(|s| match s {
            egui::Shape::Text(t) if t.galley.text().contains(words) => t.galley.job.sections.first().map(|s| s.format.color),
            _ => None,
        })
    }

    /// Where the first text that reads `words` (or starts with them) was drawn.
    fn text_rect(out: &egui::FullOutput, words: &str) -> Rect {
        shapes(out)
            .iter()
            .find_map(|s| match s {
                egui::Shape::Text(t) if t.galley.text().starts_with(words) => Some(Rect::from_min_size(t.pos, t.galley.size())),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no text {words}"))
    }

    /// The fill of the last rectangle drawn under `at` that has one.
    fn fill_under(out: &egui::FullOutput, at: Pos2) -> Option<egui::Color32> {
        shapes(out).iter().rev().find_map(|s| match s {
            egui::Shape::Rect(r) if r.rect.contains(at) && r.fill != egui::Color32::TRANSPARENT => Some(r.fill),
            _ => None,
        })
    }

    /// The colour of the outline drawn exactly at `rect`.
    fn stroke_at(out: &egui::FullOutput, rect: Rect) -> Option<egui::Color32> {
        shapes(out).iter().find_map(|s| match s {
            egui::Shape::Rect(r) if r.rect == rect => Some(r.stroke.color),
            _ => None,
        })
    }

    /// The colour of the text section that reads `words` (spaces trimmed).
    fn section_colour(out: &egui::FullOutput, words: &str) -> Option<egui::Color32> {
        shapes(out).iter().find_map(|s| match s {
            egui::Shape::Text(t) => {
                let job = &t.galley.job;
                job.sections.iter().find(|s| job.text[s.byte_range.clone()].trim() == words).map(|s| s.format.color)
            }
            _ => None,
        })
    }

    /// A left click at `at`, over two frames (press, then release); returns
    /// what `draw` returned on the release frame.
    fn click<R: Default>(ctx: &egui::Context, at: Pos2, mut draw: impl FnMut(&mut Ui) -> R) -> R {
        let press = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let mut r = R::default();
        frame(ctx, vec![Event::PointerMoved(at), press(true)], |ui| {
            draw(ui);
        });
        frame(ctx, vec![press(false)], |ui| r = draw(ui));
        r
    }

    #[derive(Clone, Copy, PartialEq, Debug, Default)]
    enum Shown {
        #[default]
        Updates,
        All,
    }

    #[test]
    fn a_scope_group_shows_counts_and_the_selection_and_reports_a_click() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let items = [(Shown::Updates, "Updates", 42), (Shown::All, "All", 175)];
            let mut first = Some(Shown::All);
            let out = frame(&ctx, vec![], |ui| {
                first = scope_group(ui, Shown::Updates, &items).clicked;
            });
            assert_eq!(first, None);
            assert_eq!(section_colour(&out, "Updates"), Some(t.on_selected));
            assert_eq!(section_colour(&out, "42"), Some(t.selected_sub));
            assert_eq!(section_colour(&out, "All"), Some(t.soft));
            assert_eq!(section_colour(&out, "175"), Some(t.faint));
            assert_eq!(fill_under(&out, text_rect(&out, "Updates").center()), Some(t.selected));
            let all = text_rect(&out, "All").center();
            let clicked = click(&ctx, all, |ui| scope_group(ui, Shown::Updates, &items).clicked);
            assert_eq!(clicked, Some(Shown::All), "{mode:?}");
        }
    }

    #[test]
    fn a_status_band_shows_its_answer_and_its_action_can_be_clicked() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let band = || StatusBand::new(Mark::Dot(t.amber), "1 newer driver is ready for this PC").sub("From Contoso, released 16 October 2026.");
            let mut action = None;
            let out = frame(&ctx, vec![], |ui| {
                band().show(ui, |ui, area| {
                    if area == BandArea::Actions {
                        action = Some(primary_button(ui, "Install update", true).rect);
                    }
                });
            });
            assert_eq!(text_colour(&out, "1 newer driver"), Some(t.text));
            assert_eq!(text_colour(&out, "From Contoso"), Some(t.label));
            assert!(shapes(&out).iter().any(|s| matches!(s, egui::Shape::Rect(r) if r.fill == t.band)), "{mode:?}");
            assert!(shapes(&out).iter().any(|s| matches!(s, egui::Shape::Circle(c) if c.fill == t.amber)), "{mode:?}");
            let at = action.expect("the action is drawn").center();
            let clicked = click(&ctx, at, |ui| {
                let mut clicked = false;
                band().show(ui, |ui, area| {
                    if area == BandArea::Actions && primary_button(ui, "Install update", true).clicked() {
                        clicked = true;
                    }
                });
                clicked
            });
            assert!(clicked, "{mode:?}");
        }
    }

    #[test]
    fn notes_are_warm_or_neutral_from_the_theme() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let out = frame(&ctx, vec![], |ui| {
                warm_note(ui, "Uninstalling it frees 91.5 MB.");
                neutral_note(ui, "Windows has not seen it opened.");
            });
            assert_eq!(text_colour(&out, "frees 91.5 MB"), Some(t.warm_note_text), "{mode:?}");
            assert_eq!(text_colour(&out, "not seen it"), Some(t.neutral_note_text), "{mode:?}");
            let fills: Vec<_> = shapes(&out).iter().filter_map(|s| if let egui::Shape::Rect(r) = s { Some(r.fill) } else { None }).collect();
            assert!(fills.contains(&t.warm_note) && fills.contains(&t.neutral_note), "{mode:?}");
        }
    }

    #[test]
    fn a_side_panel_without_a_selection_says_how_to_get_details() {
        let ctx = context(Mode::Dark);
        let t = theme::Theme::of(Mode::Dark);
        let out = frame(&ctx, vec![], |ui| SidePanel::new("empty").show(ui, None::<&str>, |ui, _| {
            ui.label("never");
        }));
        assert_eq!(text_colour(&out, "Select a single entry for detail information."), Some(t.label));
        assert_eq!(text_colour(&out, "never"), None);
        let out = frame(&ctx, vec![], |ui| SidePanel::new("full").show(ui, Some("Audacity"), |ui, name| {
            ui.label(name);
        }));
        assert!(text_colour(&out, "Audacity").is_some());
        assert_eq!(text_colour(&out, "Select a single entry"), None);
        assert!(shapes(&out).iter().any(|s| matches!(s, egui::Shape::Rect(r) if r.fill == t.panel)));
    }

    #[test]
    fn a_kpi_tile_shows_its_label_figure_and_note_on_the_panel() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let mut tiles = Vec::new();
            let out = frame(&ctx, vec![], |ui| {
                tiles = vec![
                    kpi_tile(ui, 200.0, "Core Average", "5.11 GHz", Some("Effective clock"), false).rect,
                    kpi_tile(ui, 200.0, "Thermal state", "Not reported", None, true).rect,
                ];
            });
            assert_eq!(fill_at(&out, tiles[0]), Some(t.panel), "{mode:?}");
            assert_eq!(stroke_at(&out, tiles[0]), Some(t.line), "{mode:?}");
            assert_eq!(text_colour(&out, "Core Average"), Some(t.label), "{mode:?}");
            assert_eq!(text_colour(&out, "5.11 GHz"), Some(t.text), "{mode:?}");
            assert_eq!(text_colour(&out, "Effective clock"), Some(t.faint), "{mode:?}");
            assert_eq!(text_colour(&out, "Not reported"), Some(t.faint), "a missing figure is faint ({mode:?})");
        }
    }

    #[test]
    fn a_sensor_tree_takes_its_colours_from_the_theme() {
        use crate::widgets::{tree_header, tree_row, RowLead, TreeRow};
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let row = |name, value, selected, missing| TreeRow {
                depth: 3,
                lead: RowLead::Dot(t.graphs.cpu),
                name,
                strong: false,
                grey: missing,
                note: "",
                value,
                missing,
                stale: false,
                min: "4.62 GHz",
                avg: "4.90 GHz",
                max: "5.29 GHz",
                selected,
            };
            let mut rects = Vec::new();
            let out = frame(&ctx, vec![], |ui| {
                tree_header(ui, 700.0, ["Sensor", "Value", "Min", "Avg", "Max"]);
                rects = vec![tree_row(ui, &row("Core Average", "5.11 GHz", true, false), 700.0).rect, tree_row(ui, &row("Fabric Clock", "Unavailable", false, true), 700.0).rect];
            });
            assert_eq!(text_colour(&out, "Sensor"), Some(t.label), "{mode:?}");
            assert_eq!(fill_at(&out, rects[0]), Some(t.selected), "{mode:?}");
            assert_eq!(text_colour(&out, "Core Average"), Some(t.on_selected), "{mode:?}");
            assert_eq!(text_colour(&out, "4.62 GHz"), Some(t.soft), "{mode:?}");
            assert_eq!(text_colour(&out, "4.90 GHz"), Some(t.soft), "Avg like Min and Max ({mode:?})");
            assert_eq!(text_colour(&out, "Unavailable"), Some(t.faint), "{mode:?}");
            assert_eq!(text_colour(&out, "Fabric Clock"), Some(t.faint), "{mode:?}");
        }
    }

    #[test]
    fn a_sensor_tree_draws_min_avg_and_max_under_their_headers() {
        use crate::widgets::{tree_header, tree_row, RowLead, TreeRow};
        let t = theme::Theme::of(Mode::Dark);
        let ctx = context(Mode::Dark);
        let row = TreeRow {
            depth: 3,
            lead: RowLead::Dot(t.graphs.cpu),
            name: "Core Average",
            strong: false,
            grey: false,
            note: "",
            value: "5.11 GHz",
            missing: false,
            stale: false,
            min: "1.00 GHz",
            avg: "2.00 GHz",
            max: "3.00 GHz",
            selected: false,
        };
        let out = frame(&ctx, vec![], |ui| {
            tree_header(ui, 700.0, ["Sensor", "Value", "Min", "Avg", "Max"]);
            tree_row(ui, &row, 700.0);
        });
        let (min, avg, max) = (text_rect(&out, "1.00 GHz"), text_rect(&out, "2.00 GHz"), text_rect(&out, "3.00 GHz"));
        assert!(text_rect(&out, "5.11 GHz").right() < min.left() && min.right() < avg.left() && avg.right() < max.left(), "Value, Min, Avg, Max from left to right");
        for (header, cell) in [("Min", min), ("Avg", avg), ("Max", max)] {
            assert!((text_rect(&out, header).right() - cell.right()).abs() < 0.5, "{header} right-aligned over its column");
        }
    }

    #[test]
    fn buttons_take_their_colours_from_the_theme_in_both_modes() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let mut rects = Vec::new();
            let out = frame(&ctx, vec![], |ui| {
                rects = vec![primary_button(ui, "Main", true).rect, outline_button(ui, "Second", true).rect, destructive_button(ui, "Remove", true).rect];
            });
            assert_eq!(fill_at(&out, rects[0]), Some(t.primary), "{mode:?}");
            assert_eq!(text_colour(&out, "Main"), Some(t.on_primary), "{mode:?}");
            assert_eq!(stroke_at(&out, rects[1]), Some(t.control_line), "{mode:?}");
            assert_eq!(text_colour(&out, "Second"), Some(t.text), "{mode:?}");
            assert_eq!(stroke_at(&out, rects[2]), Some(t.destructive_line), "{mode:?}");
            assert_eq!(text_colour(&out, "Remove"), Some(t.red), "{mode:?}");
        }
    }

    #[test]
    fn a_link_button_is_written_in_the_link_colour_and_reports_a_click() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let mut rect = Rect::NOTHING;
            let out = frame(&ctx, vec![], |ui| rect = link_button(ui, "See up to 8 examples").rect);
            assert_eq!(text_colour(&out, "See up to 8 examples"), Some(t.link), "{mode:?}");
            let clicked = click(&ctx, rect.center(), |ui| link_button(ui, "See up to 8 examples").clicked());
            assert!(clicked, "{mode:?}");
        }
    }

    #[test]
    fn a_section_title_is_michroma_14_in_the_text_colour() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let out = frame(&ctx, vec![], |ui| {
                section_title(ui, "File Type");
            });
            assert_eq!(text_colour(&out, "File Type"), Some(t.text), "{mode:?}");
            let font = shapes(&out).iter().find_map(|s| match s {
                egui::Shape::Text(x) if x.galley.text() == "File Type" => x.galley.job.sections.first().map(|s| s.format.font_id.clone()),
                _ => None,
            });
            assert_eq!(font, Some(FontId::new(14.0, theme::display())), "{mode:?}");
        }
    }

    #[test]
    fn a_switch_is_blue_when_on_outlined_when_off_and_reports_a_click() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let mut rects = Vec::new();
            let out = frame(&ctx, vec![], |ui| {
                rects = vec![switch(ui, true, true).rect, switch(ui, false, true).rect];
            });
            assert_eq!(rects[0].size(), SWITCH_SIZE, "{mode:?}");
            assert_eq!(fill_at(&out, rects[0]), Some(t.primary), "{mode:?}");
            assert_eq!(stroke_at(&out, rects[1]), Some(t.label), "{mode:?}");
            let knobs: Vec<_> = shapes(&out).iter().filter_map(|s| if let egui::Shape::Circle(c) = s { Some(c.fill) } else { None }).collect();
            assert!(knobs.contains(&t.on_primary) && knobs.contains(&t.label), "{mode:?}: knobs {knobs:?}");
            let clicked = click(&ctx, rects[1].center(), |ui| {
                switch(ui, true, true);
                switch(ui, false, true).clicked()
            });
            assert!(clicked, "{mode:?}");
            let locked = click(&ctx, rects[1].center(), |ui| {
                switch(ui, true, true);
                switch(ui, false, false).clicked()
            });
            assert!(!locked, "{mode:?}: a disabled switch only shows its state");
        }
    }

    #[test]
    fn a_segmented_control_marks_the_choice_and_reports_a_click() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let items = [(Shown::Updates, "Celsius"), (Shown::All, "Fahrenheit")];
            let mut picked = Some(Shown::All);
            let out = frame(&ctx, vec![], |ui| {
                picked = segmented(ui, Shown::Updates, &items).clicked;
            });
            assert_eq!(picked, None);
            assert_eq!(text_colour(&out, "Celsius"), Some(t.on_selected), "{mode:?}");
            assert_eq!(text_colour(&out, "Fahrenheit"), Some(t.soft), "{mode:?}");
            assert_eq!(fill_under(&out, text_rect(&out, "Celsius").center()), Some(t.selected), "{mode:?}");
            let clicked = click(&ctx, text_rect(&out, "Fahrenheit").center(), |ui| segmented(ui, Shown::Updates, &items).clicked);
            assert_eq!(clicked, Some(Shown::All), "{mode:?}");
            let rtl = frame(&ctx, vec![], |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    segmented(ui, Shown::Updates, &items);
                });
            });
            assert!(text_rect(&rtl, "Celsius").left() < text_rect(&rtl, "Fahrenheit").left(), "{mode:?}: options read left to right in a right-to-left row");
        }
    }

    #[test]
    fn a_setting_row_shows_its_name_its_explaining_line_and_its_control() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let mut control = None;
            let out = frame(&ctx, vec![], |ui| {
                ui.set_width(400.0);
                setting_row(ui, "Default start page", Some("The page shown when the app opens"), |ui| {
                    control = Some(outline_button(ui, "Summary", true).rect);
                });
            });
            assert_eq!(text_colour(&out, "Default start page"), Some(t.text), "{mode:?}");
            assert_eq!(text_colour(&out, "The page shown"), Some(t.faint), "{mode:?}");
            let control = control.expect("the control is drawn");
            assert!(control.right() > 380.0, "{mode:?}: the control is on the right, {control:?}");
        }
    }

    /// A three-row table in Design A's look; the second row is selected.
    fn table(ui: &mut Ui) {
        use egui_extras::{Column, TableBuilder};
        design_table(ui, |ui| {
            TableBuilder::new(ui)
                .column(Column::initial(160.0))
                .column(Column::remainder())
                .header(TABLE_HEADER_HEIGHT, |mut h| {
                    h.col(|ui| column_heading(ui, "Name"));
                    h.col(|ui| column_heading(ui, "Status"));
                })
                .body(|body| {
                    body.rows(TABLE_ROW_HEIGHT, 3, |mut row| {
                        let i = row.index();
                        let selected = i == 1;
                        row.set_selected(selected);
                        let name = ["Appinfo", "BITS", "Dhcp"][i];
                        row.col(|ui| {
                            cell_text(ui, name, CellInk::Main, selected);
                        });
                        row.col(|ui| {
                            cell_text(ui, &format!("state of {name}"), if i == 0 { CellInk::Good } else { CellInk::Soft }, selected);
                        });
                    });
                });
        });
    }

    #[test]
    fn a_design_table_has_no_stripes_a_blue_selection_and_theme_words() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let stripe = ctx.style().visuals.faint_bg_color;
            let out = frame(&ctx, vec![], table);
            let fills: Vec<_> = shapes(&out).iter().filter_map(|s| if let egui::Shape::Rect(r) = s { Some(r.fill) } else { None }).collect();
            assert!(fills.contains(&t.selected), "{mode:?}: the selected row is blue");
            assert!(!fills.contains(&stripe), "{mode:?}: no stripes");
            assert_eq!(text_colour(&out, "Name"), Some(t.label), "{mode:?}");
            assert_eq!(text_colour(&out, "Appinfo"), Some(t.text), "{mode:?}");
            assert_eq!(text_colour(&out, "state of Appinfo"), Some(t.green), "{mode:?}");
            assert_eq!(text_colour(&out, "state of Dhcp"), Some(t.soft), "{mode:?}");
            assert_eq!(text_colour(&out, "BITS"), Some(t.on_selected), "{mode:?}");
            assert_eq!(text_colour(&out, "state of BITS"), Some(t.selected_sub), "{mode:?}");
            let rule = shapes(&out).iter().any(|s| matches!(s, egui::Shape::LineSegment { stroke, .. } if stroke.color == t.line));
            assert!(rule, "{mode:?}: a line under the headings");
        }
    }

    // Design A tables, sorting and load (#131).

    /// Two rows ("chrome.exe" selected, busy; "Code.exe" idle) under a
    /// Name heading sorted descending and a CPU heading; returns the row
    /// clicked.
    fn sorted_table(ui: &mut Ui) -> Option<usize> {
        use egui_extras::{Column, TableBuilder};
        let mut clicked = None;
        design_table(ui, |ui| {
            TableBuilder::new(ui)
                .sense(egui::Sense::click())
                .column(Column::exact(200.0))
                .column(Column::exact(100.0))
                .header(TABLE_HEADER_HEIGHT, |mut h| {
                    h.col(|ui| {
                        sort_heading(ui, "Name", Some(true));
                    });
                    h.col(|ui| {
                        sort_heading(ui, "CPU", None);
                    });
                })
                .body(|body| {
                    body.rows(TABLE_ROW_HEIGHT, 2, |mut row| {
                        let i = row.index();
                        row.set_selected(i == 0);
                        row.col(|ui| {
                            cell_text(ui, ["chrome.exe", "Code.exe"][i], CellInk::Main, i == 0);
                        });
                        row.col(|ui| {
                            heat_cell(ui, [1.0, 0.0][i]);
                            cell_text(ui, ["80.0%", "0.0%"][i], CellInk::Main, i == 0);
                        });
                        if row.response().clicked() {
                            clicked = Some(i);
                        }
                    });
                });
        });
        clicked
    }

    #[test]
    fn the_sorted_heading_stands_out_with_its_arrow_and_can_be_clicked() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let mut head = None;
            let out = frame(&ctx, vec![], |ui| {
                head = Some(sort_heading(ui, "Memory", Some(false)).rect);
                sort_heading(ui, "PID", None);
            });
            assert_eq!(text_colour(&out, "Memory"), Some(t.text), "{mode:?}");
            assert_eq!(text_colour(&out, "PID"), Some(t.label), "{mode:?}");
            assert_eq!(glyph_tints(&out).len(), 1, "only the sorted heading has its sort glyph, {mode:?}");
            let clicked = click(&ctx, head.unwrap().center(), |ui| sort_heading(ui, "Memory", Some(false)).clicked());
            assert!(clicked, "{mode:?}");
        }
    }

    #[test]
    fn a_busy_cell_is_lit_amber_and_an_idle_one_is_not() {
        let ctx = context(Mode::Dark);
        let t = theme::Theme::of(Mode::Dark);
        let empty = shapes(&frame(&ctx, vec![], |_| {})).len();
        let out = frame(&ctx, vec![], |ui| {
            heat_cell(ui, 0.0);
        });
        assert_eq!(shapes(&out).len(), empty, "idle: nothing painted");
        let out = frame(&ctx, vec![], |ui| {
            heat_cell(ui, 1.0);
        });
        let fill = shapes(&out).iter().filter_map(|s| if let egui::Shape::Rect(r) = s { Some(r.fill) } else { None }).last().expect("busy: lit");
        assert!(fill.a() < 255 && fill.a() > 0, "a translucent tint, {fill:?}");
        assert_eq!(fill, heat_fill(&t, 1.0).unwrap());
        assert!(fill.r() > fill.b(), "amber, {fill:?}");
    }

    #[test]
    fn a_field_card_shows_labels_over_values_on_the_panel() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let out = frame(&ctx, vec![], |ui| {
                field_card(ui, "Hardware", "Processor", &[("Logical processors", "12".into()), ("Speed", "4.70 GHz".into())]);
            });
            assert_eq!(text_colour(&out, "Hardware"), Some(t.faint), "{mode:?}");
            assert_eq!(text_colour(&out, "Processor"), Some(t.text), "{mode:?}");
            assert_eq!(text_colour(&out, "Logical processors"), Some(t.label), "{mode:?}");
            assert_eq!(text_colour(&out, "4.70 GHz"), Some(t.text), "{mode:?}");
            assert!(shapes(&out).iter().any(|s| matches!(s, egui::Shape::Rect(r) if r.fill == t.panel)), "{mode:?}");
        }
    }

    #[test]
    fn a_click_on_a_rows_words_picks_the_row() {
        let ctx = context(Mode::Dark);
        frame(&ctx, vec![], |ui| {
            sorted_table(ui);
        });
        let out = frame(&ctx, vec![], |ui| {
            sorted_table(ui);
        });
        let at = text_shape(&out, "Code.exe").expect("the second row is drawn").visual_bounding_rect().center();
        let clicked = click(&ctx, at, sorted_table);
        assert_eq!(clicked, Some(1));
    }

    /// The first text shape that contains `words`.
    fn text_shape(out: &egui::FullOutput, words: &str) -> Option<egui::epaint::TextShape> {
        shapes(out).into_iter().find_map(|s| match s {
            egui::Shape::Text(t) if t.galley.text().contains(words) => Some(t),
            _ => None,
        })
    }

    // ---- Design A connections parts (#134) ----

    #[test]
    fn a_scope_group_can_mark_one_count_and_show_a_scope_that_cannot_be_chosen() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let items = [(Shown::Updates, "Updates", 42), (Shown::All, "All", 175)];
            let mut group = None;
            let out = frame(&ctx, vec![], |ui| {
                let s = ScopeGroup::new(Shown::Updates, &items).alert(Shown::All).unavailable("Local IPC", "unavailable on Windows", "Windows has none").show(ui);
                group = Some(s.group.rect);
            });
            assert_eq!(section_colour(&out, "175"), Some(t.amber), "{mode:?}");
            assert_eq!(section_colour(&out, "42"), Some(t.selected_sub), "{mode:?}");
            assert_eq!(section_colour(&out, "Local IPC"), Some(t.faint), "{mode:?}");
            assert_eq!(section_colour(&out, "unavailable on Windows"), Some(t.faint), "{mode:?}");
            assert!(group.expect("the group is drawn").width() > 100.0);
        }
    }

    #[test]
    fn an_alert_tile_shows_its_figure_and_outline_in_the_alert_colour() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme::Theme::of(mode);
            let ctx = context(mode);
            let mut rect = None;
            let out = frame(&ctx, vec![], |ui| {
                rect = Some(alert_tile(ui, 200.0, "Public", "28", None, t.amber).rect);
            });
            assert_eq!(text_colour(&out, "28"), Some(t.amber), "{mode:?}");
            assert_eq!(text_colour(&out, "Public"), Some(t.label), "{mode:?}");
            assert_eq!(stroke_at(&out, rect.unwrap()), Some(t.amber.gamma_multiply(0.45)), "{mode:?}");
        }
    }

    // ---- Design A figure cards and accent rail (#136) ----

    /// The fills of every rectangle drawn, the page's own background left out.
    fn fills(out: &egui::FullOutput, page: egui::Color32) -> Vec<egui::Color32> {
        shapes(out)
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Rect(r) if r.fill != page && r.fill != egui::Color32::TRANSPARENT => Some(r.fill),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_bullet_bar_and_a_progress_line_draw_on_the_theme() {
        for mode in [Mode::Dark, Mode::Light] {
            let (ctx, t) = (context(mode), Theme::of(mode));
            let colour = t.graphs.cpu;
            let out = frame(&ctx, vec![], |ui| bullet_bar(ui, 200.0, 0.5, colour, Some(0.6), Some(0.8)));
            let f = fills(&out, t.page);
            for (name, want) in [("track", t.hover), ("fill", colour), ("reference tick", t.text), ("best dashes", t.soft)] {
                assert!(f.contains(&want), "{name} in {mode:?}: {f:?}");
            }
            assert!(f.iter().all(|c| [t.hover, colour, t.text, t.soft].contains(c)), "only tokens in {mode:?}: {f:?}");
            let out = frame(&ctx, vec![], |ui| progress_line(ui, 0.4, colour));
            assert_eq!(fills(&out, t.page), vec![t.hover, colour], "progress track and fill in {mode:?}");
        }
    }

    #[test]
    fn a_selected_rail_item_has_the_rail_fill_and_the_link_bar() {
        assert_eq!(rail_fill(&Theme::of(Mode::Dark)), egui::Color32::from_rgb(0x28, 0x2f, 0x2b), "the dark fill");
        assert_eq!(rail_fill(&Theme::of(Mode::Light)), Theme::of(Mode::Light).selected);
        for mode in [Mode::Dark, Mode::Light] {
            let (ctx, t) = (context(mode), Theme::of(mode));
            let item = RailItem { name: "CPU", tag: Some("Added"), score: Some(("102".into(), t.amber)), status: "Not run yet", status_color: t.label };
            let out = frame(&ctx, vec![], |ui| {
                rail_item(ui, &item, true);
            });
            assert_eq!(fills(&out, t.page), vec![rail_fill(&t), t.link], "fill and accent bar in {mode:?}");
            assert_eq!(text_colour(&out, "CPU"), Some(t.text));
            assert_eq!(text_colour(&out, "Added"), Some(t.label));
            assert_eq!(text_colour(&out, "102"), Some(t.amber));
            assert_eq!(text_colour(&out, "Not run yet"), Some(t.label));
            let out = frame(&ctx, vec![], |ui| {
                rail_item(ui, &item, false);
            });
            assert!(fills(&out, t.page).is_empty(), "an idle entry has no fill in {mode:?}");
        }
    }

    #[test]
    fn the_alert_banner_is_the_alert_note() {
        for mode in [Mode::Dark, Mode::Light] {
            let (ctx, t) = (context(mode), Theme::of(mode));
            let out = frame(&ctx, vec![], |ui| alert_banner(ui, "Runs are not saved"));
            assert_eq!(fills(&out, t.page), vec![t.alert_note], "{mode:?}");
            assert!(shapes(&out).iter().any(|s| matches!(s, egui::Shape::Rect(r) if r.stroke.color == t.destructive_line)), "border in {mode:?}");
            assert_eq!(text_colour(&out, "Runs are not saved"), Some(t.alert_note_text));
            assert_eq!(glyph_tints(&out), [crate::theme::UNTINTED], "the warning glyph in its own colours, {mode:?}");
        }
    }

    /// The tints of the glyphs (textured images) drawn in `out`.
    fn glyph_tints(out: &egui::FullOutput) -> Vec<Color32> {
        crate::test_ui::flat_shapes(out)
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => Some(m.vertices[0].color),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_figure_card_is_a_panel_with_a_soft_label_and_a_grey_unit() {
        for mode in [Mode::Dark, Mode::Light] {
            let (ctx, t) = (context(mode), Theme::of(mode));
            let card = FigureCard {
                label: "Single core",
                color: t.graphs.cpu,
                score: Some(("Score 102".into(), t.green)),
                value: "43,020".into(),
                unit: "Mops/sec",
                value_color: t.text,
            };
            let out = frame(&ctx, vec![], |ui| figure_card(ui, 400.0, &card, |_| {}));
            assert!(fills(&out, t.page).contains(&t.panel), "{mode:?}");
            assert!(shapes(&out).iter().any(|s| matches!(s, egui::Shape::Rect(r) if r.stroke.color == t.line)), "border in {mode:?}");
            assert_eq!(text_colour(&out, "Single core"), Some(t.soft));
            assert_eq!(text_colour(&out, "Score 102"), Some(t.green));
            assert_eq!(text_colour(&out, "Mops/sec"), Some(t.label));
            assert_eq!(text_colour(&out, "43,020"), Some(t.text));
        }
    }

    #[test]
    fn a_secondary_button_is_34_high_in_the_text_colour() {
        for mode in [Mode::Dark, Mode::Light] {
            let (ctx, t) = (context(mode), Theme::of(mode));
            let mut h = 0.0;
            let out = frame(&ctx, vec![], |ui| h = secondary_button(ui, "Run All Benchmarks", true).rect.height());
            assert_eq!(h, OUTLINE_HEIGHT);
            assert_eq!(text_colour(&out, "Run All Benchmarks"), Some(t.text));
            assert!(shapes(&out).iter().any(|s| matches!(s, egui::Shape::Rect(r) if r.stroke.color == t.control_line)), "outline in {mode:?}");
        }
    }

    #[test]
    fn a_run_sparkline_spans_its_box_lowest_at_the_bottom_highest_at_the_top() {
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(160.0, 30.0));
        assert_eq!(spark_points(&[1.0, 3.0, 2.0], rect), [Pos2::new(4.0, 26.0), Pos2::new(80.0, 4.0), Pos2::new(156.0, 15.0)]);
        assert_eq!(spark_points(&[5.0, 5.0], rect), [Pos2::new(4.0, 15.0), Pos2::new(156.0, 15.0)], "a flat run sits in the middle");
        assert!(spark_points(&[5.0], rect).is_empty(), "one run draws no line");
    }

    #[test]
    fn a_telemetry_chart_and_its_cells_stay_dark_in_both_modes() {
        let dark = Theme::of(Mode::Dark);
        for mode in [Mode::Dark, Mode::Light] {
            let (ctx, t) = (context(mode), Theme::of(mode));
            let values = [10.0, 60.0, 30.0];
            let out = frame(&ctx, vec![], |ui| {
                line_chart(ui, egui::vec2(300.0, 80.0), &[ChartLine { values: &values, max: 100.0, style: LineStyle::Solid, color: t.graphs.utilisation }]);
                cell_grid(ui, &[0.0, 50.0], &["CPU 0".into(), "CPU 1".into()], t.graphs.utilisation, None);
            });
            let fills: Vec<egui::Color32> = shapes(&out).iter().filter_map(|s| if let egui::Shape::Rect(r) = s { Some(r.fill) } else { None }).collect();
            assert!(fills.contains(&t.graph_bg), "{mode:?}: chart on graph_bg: {fills:?}");
            assert!(fills.iter().filter(|f| **f == dark.well).count() >= 2, "{mode:?}: idle and busy cells on the dark well: {fills:?}");
        }
    }

    #[test]
    fn a_marked_button_is_a_main_action_with_its_mark() {
        let ctx = context(Mode::Light);
        let t = Theme::of(Mode::Light);
        let mut h = 0.0;
        let out = frame(&ctx, vec![], |ui| h = marked_button(ui, "Record", ButtonMark::Record, t.primary, None, true).rect.height());
        assert_eq!(h, PRIMARY_HEIGHT);
        assert_eq!(text_colour(&out, "Record"), Some(t.on_primary));
        assert_eq!(glyph_tints(&out), [t.on_primary], "the record glyph in the button's ink");
    }

    #[test]
    fn an_idle_cell_shows_the_well_and_a_busier_one_more_of_its_colour() {
        let (green, well) = (Color32::from_rgb(0x4c, 0xd2, 0x6a), Color32::from_rgb(0x18, 0x18, 0x18));
        assert_eq!(cell_fill(0.0, green, well), well);
        assert_eq!(cell_fill(10.0, green, well), Color32::from_rgba_unmultiplied(0x4c, 0xd2, 0x6a, 61));
        assert_eq!(cell_fill(100.0, green, well), Color32::from_rgba_unmultiplied(0x4c, 0xd2, 0x6a, 250));
        assert_eq!(cell_fill(140.0, green, well), cell_fill(100.0, green, well), "capped at full load");
    }

    #[test]
    fn the_degree_sign_is_set_in_the_body_face_the_rest_in_the_display_face() {
        assert_eq!(display_runs("61.0 \u{00b0}C"), [("61.0 ", false), ("\u{00b0}", true), ("C", false)]);
        assert_eq!(display_runs("45.0%"), [("45.0%", false)]);
        assert_eq!(display_runs("\u{00b0}\u{00b0}F"), [("\u{00b0}\u{00b0}", true), ("F", false)]);
        assert_eq!(display_runs(""), []);
    }

    #[test]
    fn a_panel_value_with_a_degree_sign_lays_out_with_the_body_faces_degree() {
        let job = display_job("61.0 \u{00b0}C", FontId::monospace(13.0), Color32::WHITE);
        assert_eq!(job.text, "61.0 \u{00b0}C");
        let fonts: Vec<&egui::FontFamily> = job.sections.iter().map(|s| &s.format.font_id.family).collect();
        assert_eq!(fonts, [&egui::FontFamily::Monospace, &egui::FontFamily::Proportional, &egui::FontFamily::Monospace]);
    }

    #[test]
    fn a_chart_line_keeps_the_busiest_sample_of_each_pixel() {
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(2.0, 100.0));
        let pts = chart_points(rect, &[10.0, 50.0, 20.0, 0.0], 100.0);
        assert_eq!(pts, [Pos2::new(0.0, 50.0), Pos2::new(2.0, 80.0)], "two pixels: 50 then 20");
        let flat = chart_points(Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 10.0)), &[200.0, 5.0], 100.0);
        assert_eq!(flat[0].y, 0.0, "clamped to the top");
        assert!(chart_points(rect, &[], 100.0).is_empty());
    }
}
