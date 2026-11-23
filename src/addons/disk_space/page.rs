//! The Disk Space page as drawn: toolbar, status strip, side panel
//! (drive cards, File Type legend, largest folders, the chosen item,
//! History) and main column (canvas, the group's list), with the status
//! band above both, in the theme's tokens and Design A's shared parts
//! (#91, #129). The canvas and the LED bar stay dark in both themes.
//! Drawing only: what it shows comes from [`View`], what it asks for goes
//! back through [`Command`] and the add-on's methods.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{vec2, Align, Align2, Color32, CornerRadius, FontId, Galley, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, Ui};

use super::kind::Category;
use super::phase::Phase;
use super::target::{self, Plan, Target};
use super::{actions, canvas, history, item_of, nav, text, tree, Command, DiskSpace, Done, Drawing, Group, Menu, View};
use crate::platform::{Owner, PageContext};
use crate::sampler::DiskRow;
use crate::theme::{self, Theme};
use crate::widgets::parts::{destructive_button, link_button, outline_button, primary_button, row_fill, section_title, BandArea, Mark, SidePanel, StatusBand};
use crate::widgets::{fit_path, fmt_bytes, led_strip, rgb};

/// The side panel's width.
const SIDE_WIDTH: f32 = 330.0;
/// Below this page width the side panel moves under the main column.
const NARROW: f32 = 800.0;
/// Segments of the status strip's LED bar.
const LEDS: usize = 40;

impl DiskSpace {
    /// Draws the page (the add-on's `ui`).
    pub(super) fn draw_page(&mut self, ui: &mut Ui, page: &mut PageContext) {
        self.page_seen();
        let disks = &page.sample.disks;
        let t = theme::of_ui(ui);
        if self.target.is_none() {
            self.target = disks.first().map(|d| Target::Drive(d.mount.clone()));
        }
        self.show_last_of_target();
        let v = self.view();
        if v.phase.active() || self.working() {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        // The drives are listed again by the sampler, so a drive plugged in
        // or out shows up here; a chosen drive that is gone cannot start.
        let plan = self.target.as_ref().map(|t| t.plan(disks));
        let ready = plan.clone().and_then(Result::ok);
        let m = self.menu(disks);
        let now = Instant::now();
        // An item action asked for in the band, the panel or a context
        // menu, carried out once the page is drawn.
        let mut asked: Option<Ask> = None;
        // With results, the band leads the page and holds the main button;
        // before them the main button sits in the toolbar.
        let banded = v.root.is_some();
        let mut cmd = None;
        if banded {
            let (dismissed, c) = self.band(ui, page, &v, &m, now, &mut asked);
            if dismissed {
                self.dismiss_skipped();
            }
            cmd = c;
            ui.add_space(12.0);
        }
        // A toolbar command, carried out once the toolbar is drawn.
        let cmd = self.toolbar(ui, disks, &v, &m, !banded).or(cmd);
        // The note under the toolbar: why Start is off, or that nothing
        // was measured yet.
        if let Some(Err(why)) = &plan {
            ui.label(RichText::new(why).size(12.5).color(t.red));
        } else if v.root.is_none() && v.running.is_none() && ready.is_some() {
            ui.label(RichText::new("Nothing measured yet.").size(12.5).color(t.label));
        }
        ui.add_space(6.0);
        if let Some(c) = cmd {
            self.command(c, disks);
        }
        self.stop_dialog(ui);
        self.clear_dialog(ui);

        status_strip(ui, &v, &t);

        let mut history_click = HistoryClick::None;
        let page_ref = &*page;

        if ui.available_width() < NARROW {
            // A narrow window: the side panel follows the main column.
            egui::ScrollArea::vertical().id_salt("disk-space-narrow-scroll").auto_shrink([false, false]).show(ui, |ui| {
                self.main_column(ui, page_ref, &v, ready, now, &t, &mut asked);
                ui.add_space(14.0);
                panel_frame(&t).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    history_click = self.side_panel(ui, page_ref, &v, now, &t, &mut asked);
                });
            });
        } else {
            // Side panel, in the capture's order: drive card, File Type,
            // Largest Folders, the chosen item's actions, History.
            egui::SidePanel::right("disk-space-side")
                .resizable(false)
                .show_separator_line(false)
                .exact_width(SIDE_WIDTH)
                .frame(egui::Frame::NONE)
                .show_inside(ui, |ui| {
                    SidePanel::new("disk-space-side-scroll").show(ui, Some(()), |ui, ()| {
                        history_click = self.side_panel(ui, page_ref, &v, now, &t, &mut asked);
                    });
                });
            egui::CentralPanel::default().frame(egui::Frame::NONE.inner_margin(Margin { right: 18, ..Default::default() })).show_inside(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("disk-space-main-scroll").auto_shrink([false, false]).show(ui, |ui| {
                    self.main_column(ui, page_ref, &v, ready, now, &t, &mut asked);
                });
            });
        }
        match history_click {
            HistoryClick::Open(id) => self.open_saved(&id),
            HistoryClick::Clear => self.ask_clear(),
            HistoryClick::None => {}
        }
        match asked {
            Some(Ask::Act(verb, key)) => self.act(verb, &key, page),
            Some(Ask::ShowOwner(key)) => self.show_owner(&key, page),
            None => {}
        }
    }

    /// The status band with the main button; gives whether the
    /// skipped-items note was dismissed and the command clicked.
    fn band(&mut self, ui: &mut Ui, page: &PageContext, v: &View, m: &Menu, now: Instant, asked: &mut Option<Ask>) -> (bool, Option<Command>) {
        let links = &mut self.links;
        let mut link = |p: &Path| links.get(p, now, |p| page.owner_of(p));
        status_band(ui, v, m, &mut link, asked)
    }

    /// The main column: canvas (or the veil), the group's list.
    #[allow(clippy::too_many_arguments)]
    fn main_column(&mut self, ui: &mut Ui, page: &PageContext, v: &View, ready: Option<Plan>, now: Instant, t: &Theme, asked: &mut Option<Ask>) {
        ui.spacing_mut().item_spacing.y = 6.0;
        if v.root.is_some() {
            let selected = self.selected();
            let (state, links) = (&self.state, &mut self.links);
            let mut menu = |ui: &mut Ui, key: &nav::Key| {
                let owner = item_of(state, key).and_then(|p| links.get(&p, now, |p| page.owner_of(p)));
                if let Some(a) = item_actions(ui, key, owner.as_ref()) {
                    *asked = Some(a);
                    ui.close();
                }
            };
            let moved = self.canvas.show(ui, v, self.drawing, selected.as_ref(), &mut menu);
            if let Some(m) = moved {
                self.navigate(m);
            }
            // The chip's click or Enter: the same action as the lists'.
            if let Some((verb, key)) = self.canvas.take_action() {
                *asked = Some(Ask::Act(verb, key));
            }
            let n = self.list_length();
            let selected = self.selected();
            let links = &mut self.links;
            let mut link = |p: &Path| links.get(p, now, |p| page.owner_of(p));
            grouped_list(ui, v, self.group, n, selected.as_ref(), t, &mut link, asked);
        } else if let (None, Some(t), Some(plan)) = (&v.running, self.target.clone(), ready) {
            // The veil before the first scan: clicked, or Enter/Space
            // once Tab gave it the focus, it starts the scan.
            if canvas::veil(ui, &format!("Measure {}", t.name())) {
                self.measure(&t, plan);
            }
        }
    }

    /// The side panel's content; gives what was clicked in History.
    fn side_panel(&mut self, ui: &mut Ui, page: &PageContext, v: &View, now: Instant, t: &Theme, asked: &mut Option<Ask>) -> HistoryClick {
        ui.spacing_mut().item_spacing.y = 4.0;
        let cards = self.target.as_ref().map(|t| t.drives(&page.sample.disks)).unwrap_or_default();
        drive_cards(ui, &cards, rgb(page.settings.disk_color), t);
        if !cards.is_empty() {
            rule(ui, t);
        }
        legend(ui, v, t);
        rule(ui, t);
        section_title(ui, &format!("Largest {} folders", self.list_length()));
        let selected = self.selected();
        {
            let links = &mut self.links;
            let mut link = |p: &Path| links.get(p, now, |p| page.owner_of(p));
            if v.root.is_none() {
                ui.label(RichText::new("Nothing measured yet.").size(13.0).color(t.label));
            }
            for (path, bytes) in &v.folders {
                let key = nav::Key::Folder(PathBuf::from(path));
                let on = selected.as_ref() == Some(&key);
                let cells = Cells { size: text::size(*bytes), size_width: 58.0, text: path, path: true, ..Cells::default() };
                item_row(ui, &cells, key, on, t, &mut link, asked);
            }
        }
        if let Some(key) = selected {
            let path = item_of(&self.state, &key);
            let owner = path.as_ref().and_then(|p| self.links.get(p, now, |p| page.owner_of(p)));
            ui.add_space(10.0);
            selected_panel(ui, v, &key, path.as_deref(), owner.as_ref(), t, asked);
        }
        rule(ui, t);
        history_section(ui, self, v.running.is_some(), t)
    }

    /// The toolbar: target / Group / View drop-downs; with `buttons`, the
    /// target and the main button (Start, or Pause/Resume and Stop) on the
    /// right. Gives the command clicked.
    fn toolbar(&mut self, ui: &mut Ui, disks: &[DiskRow], v: &View, m: &Menu, buttons: bool) -> Option<Command> {
        let mut cmd: Option<Command> = None;
        let wide = ui.available_width() >= 900.0;
        ui.horizontal(|ui| {
            ui.set_min_height(30.0);
            let mut picked = self.target.clone();
            let shown = self.target.as_ref().map_or("No drive", Target::name).to_string();
            let mut choose = false;
            ui.scope(|ui| {
                ui.set_max_width(240.0);
                egui::ComboBox::from_id_salt("disk-space-target").selected_text(shown).truncate().show_ui(ui, |ui| {
                    for t in target::choices(disks) {
                        let label = t.label(disks);
                        ui.selectable_value(&mut picked, Some(t), label);
                    }
                    choose = ui.selectable_label(false, "Choose Folder\u{2026}").clicked();
                });
            });
            if choose {
                picked = self.pick_folder().or(picked);
            }
            if picked != self.target {
                cmd = picked.map(Command::Target);
            }
            egui::ComboBox::from_id_salt("disk-space-group").selected_text(format!("Group: {}", self.group.label())).show_ui(ui, |ui| {
                for g in Group::ALL {
                    ui.selectable_value(&mut self.group, g, g.label());
                }
            });
            egui::ComboBox::from_id_salt("disk-space-view").selected_text(format!("View: {}", self.drawing.label())).show_ui(ui, |ui| {
                for d in Drawing::ALL {
                    ui.selectable_value(&mut self.drawing, d, d.label());
                }
            });
            if wide && buttons {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(c) = main_buttons(ui, self.target.as_ref(), v, m) {
                        cmd = Some(c);
                    }
                });
            }
        });
        if !wide && buttons {
            // A narrow page: the main button gets a row of its own.
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(c) = main_buttons(ui, self.target.as_ref(), v, m) {
                        cmd = Some(c);
                    }
                });
            });
        }
        cmd
    }
}

/// The main button, laid right to left: the target (when given), then
/// Start, or Pause/Resume and Stop Scanning (with a spinner while measuring).
fn main_buttons(ui: &mut Ui, target: Option<&Target>, v: &View, m: &Menu) -> Option<Command> {
    let t = theme::of_ui(ui);
    let mut cmd = None;
    if m.running {
        if destructive_button(ui, "Stop Scanning", true).clicked() {
            cmd = Some(Command::Stop);
        }
        if outline_button(ui, m.pause, true).clicked() {
            cmd = Some(if m.pause == "Resume" { Command::Resume } else { Command::Pause });
        }
        if v.phase == Phase::Running {
            ui.spinner();
        }
    } else {
        // A long folder path is cut short, not pushed off the page.
        let (shown, cut) = fit_path(ui, &m.start, &FontId::new(13.0, theme::semibold()), 290.0);
        let start = primary_button(ui, &shown, m.can_start);
        let start = if cut { start.on_hover_text(&m.start) } else { start };
        if start.clicked() {
            cmd = Some(Command::Start);
        }
    }
    if let Some(target) = target {
        ui.scope(|ui| {
            ui.set_max_width(220.0);
            ui.add(egui::Label::new(RichText::new(target.name()).size(13.0).color(t.label)).truncate());
        });
    }
    cmd
}

/// The side panel's frame when it follows the main column (a narrow
/// page): the same panel as [`SidePanel`], without its own scrolling.
fn panel_frame(t: &Theme) -> egui::Frame {
    egui::Frame::new().fill(t.panel).stroke(Stroke::new(1.0, t.line)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(20, 18))
}

/// A line between the side panel's sections.
fn rule(ui: &mut Ui, t: &Theme) {
    ui.add_space(8.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, t.line));
    ui.add_space(8.0);
}

/// The History section: the saved scans (a click shows one), what could
/// not be read, and Clear History…
fn history_section(ui: &mut Ui, d: &DiskSpace, running: bool, t: &Theme) -> HistoryClick {
    let shelf = d.history();
    let mut click = HistoryClick::None;
    ui.horizontal(|ui| {
        section_title(ui, "History");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(shelf.entries.len().to_string()).font(t.fonts.row.clone()).color(t.label));
        });
    });
    if shelf.entries.is_empty() {
        ui.label(RichText::new(history::EMPTY).size(12.5).color(t.faint));
    }
    let today = crate::platform::local_secs().div_euclid(86_400);
    for e in &shelf.entries {
        let done = Done::at(e.finished, 0);
        let when = format!("{} {}", text::day_word(done.day, today), done.at);
        if history_row(ui, &e.root_path, &when, &text::size(e.total_bytes), &text::files(e.file_count), !running, t) {
            click = HistoryClick::Open(e.id.clone());
        }
    }
    if shelf.left_out > 0 {
        ui.label(RichText::new(history::left_out(shelf.left_out)).size(12.5).color(t.label));
    }
    if let Some(e) = &shelf.error {
        ui.label(RichText::new(e).size(12.5).color(t.red));
    }
    ui.add_space(6.0);
    if destructive_button(ui, "Clear History\u{2026}", !shelf.entries.is_empty() && !running).clicked() {
        click = HistoryClick::Clear;
    }
    click
}

/// One saved scan: target in bold, then when, size and files; on two
/// lines when the target is long. True when clicked.
fn history_row(ui: &mut Ui, target: &str, when: &str, size: &str, files: &str, enabled: bool, t: &Theme) -> bool {
    let font = FontId::proportional(12.5);
    let rest = format!("{when}   {size}   ");
    let rest_w = ui.painter().layout_no_wrap(format!("{rest}{files}"), font.clone(), t.text).size().x;
    let width = ui.available_width();
    let target_w = ui.painter().layout_no_wrap(target.to_string(), font.clone(), t.text).size().x;
    let one_line = target_w + 12.0 + rest_w + 16.0 <= width;
    let height = if one_line { 26.0 } else { 42.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    row_fill(p, rect, false, enabled && resp.hovered());
    let ink = if enabled { t.text } else { t.faint };
    let sub = t.faint;
    let left = rect.left() + 8.0;
    let (shown, cut) = fit_path(ui, target, &font, width - 16.0);
    let top = if one_line { rect.center().y } else { rect.top() + 12.0 };
    p.text(Pos2::new(left, top), Align2::LEFT_CENTER, &shown, FontId::proportional(12.5), ink);
    // Bold words: drawn twice, half a point apart.
    p.text(Pos2::new(left + 0.5, top), Align2::LEFT_CENTER, &shown, FontId::proportional(12.5), ink);
    let (x, y) = if one_line { (left + target_w + 12.0, rect.center().y) } else { (left, rect.top() + 30.0) };
    let g = p.layout_no_wrap(rest, font.clone(), ink);
    let w = g.size().x;
    p.galley(Pos2::new(x, y - g.size().y / 2.0), g, ink);
    p.text(Pos2::new(x + w, y), Align2::LEFT_CENTER, files, font, sub);
    let resp = if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp.on_hover_text("Available when the scan ends") };
    let resp = if cut { resp.on_hover_text(target) } else { resp };
    resp.clicked()
}

/// What was clicked in the History section.
enum HistoryClick {
    None,
    /// A saved scan, by id.
    Open(String),
    /// Clear History…
    Clear,
}

/// The status band (the shared part): what the scan found (where the
/// space goes, or that it was stopped, or what is being measured), the
/// largest folder and file, when it finished and the note about unreadable
/// folders; the main button on the right. Gives whether the skipped-items
/// note was dismissed and the command clicked.
fn status_band(ui: &mut Ui, v: &View, m: &Menu, link: Link, asked: &mut Option<Ask>) -> (bool, Option<Command>) {
    let t = theme::of_ui(ui);
    let root = v.root.as_deref().unwrap_or("");
    let (mark, headline, sub) = if let Some(s) = &v.where_it_goes {
        (Mark::Dot(t.amber), s.clone(), None)
    } else if let Some(r) = &v.running {
        let mark = if v.phase == Phase::Running { Mark::Busy } else { Mark::Dot(t.amber) };
        (mark, format!("Measuring {r}"), None)
    } else {
        let stopped = format!("Stopped while measuring {root}: {} in {}.", text::size(v.bytes), text::files(v.files));
        (Mark::Dot(t.amber), stopped, (!v.phase.saveable()).then_some("Stopped scans are not saved to History."))
    };
    let mut band = StatusBand::new(mark, &headline).actions_width(300.0);
    if let Some(s) = sub {
        band = band.sub(s);
    }
    let (mut dismissed, mut cmd) = (false, None);
    band.show(ui, |ui, area| match area {
        BandArea::Body => {
            ui.spacing_mut().item_spacing.y = 2.0;
            if let Some((path, bytes)) = &v.largest_folder {
                let cells = Cells { label: Some("Largest folder"), size: text::size(*bytes), size_width: 64.0, text: path, path: true, ..Cells::default() };
                item_row(ui, &cells, nav::Key::Folder(PathBuf::from(path)), false, &t, &mut *link, asked);
            }
            if let Some((path, bytes)) = &v.largest_file {
                let cells = Cells { label: Some("Largest file"), size: text::size(*bytes), size_width: 64.0, text: path, path: true, ..Cells::default() };
                item_row(ui, &cells, nav::Key::File(PathBuf::from(path)), false, &t, &mut *link, asked);
            }
            ui.spacing_mut().item_spacing.y = 8.0;
            if let Some(f) = &v.finished {
                ui.add(egui::Label::new(RichText::new(f).font(t.fonts.label.clone()).color(t.label)).wrap());
            }
            if let Some(note) = &v.skipped {
                dismissed = skipped_note(ui, note, v.skipped_why.as_deref(), &v.skipped_examples, &t);
            }
            if let Some(limit) = &v.hard_link_limit {
                egui::Frame::new().fill(t.warm_note).corner_radius(CornerRadius::same(5)).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(RichText::new(limit).size(13.0).color(t.warm_note_text)).wrap());
                });
            }
        }
        BandArea::Actions => {
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    cmd = main_buttons(ui, None, v, m);
                });
            });
        }
        BandArea::Below => {}
    });
    (dismissed, cmd)
}

/// An item action asked for while the page is drawn.
enum Ask {
    Act(actions::Verb, nav::Key),
    /// The link to the item's row in another add-on (Installed Apps).
    ShowOwner(nav::Key),
}

/// Who owns a path among the other add-ons that are on (cached).
type Link<'a> = &'a mut dyn FnMut(&Path) -> Option<Owner>;

/// The item actions (links in the panel, buttons in a context menu),
/// then the link to the item's row in another add-on when one owns it.
fn item_actions(ui: &mut Ui, key: &nav::Key, owner: Option<&Owner>) -> Option<Ask> {
    item_actions_as(ui, key, owner, |ui, words| ui.button(words))
}

/// [`item_actions`], each one drawn by `add`.
fn item_actions_as(ui: &mut Ui, key: &nav::Key, owner: Option<&Owner>, mut add: impl FnMut(&mut Ui, &str) -> egui::Response) -> Option<Ask> {
    let mut asked = None;
    for verb in actions::Verb::ALL {
        if add(ui, verb.label()).clicked() {
            asked = Some(Ask::Act(verb, key.clone()));
        }
    }
    if let Some(o) = owner {
        if add(ui, &format!("See {} in {}", o.label, o.page)).clicked() {
            asked = Some(Ask::ShowOwner(key.clone()));
        }
    }
    asked
}

/// The chosen piece in a box: "Selected", its name and size, where it is,
/// and its actions.
fn selected_panel(ui: &mut Ui, v: &View, key: &nav::Key, path: Option<&Path>, owner: Option<&Owner>, t: &Theme, asked: &mut Option<Ask>) {
    let piece = v.pieces.iter().find(|p| &p.key == key);
    egui::Frame::new().stroke(Stroke::new(1.0, t.line)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(RichText::new("Selected").font(t.fonts.label.clone()).color(t.label));
        if let Some(p) = piece {
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(text::size(p.bytes)).size(13.0).color(t.text).strong());
                    ui.add(egui::Label::new(RichText::new(&p.label).size(13.0).color(t.text).strong()).truncate());
                });
            });
        }
        if let Some(path) = path {
            ui.add(egui::Label::new(RichText::new(path.display().to_string()).size(12.0).color(t.faint)).wrap());
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(14.0, 6.0);
            if let Some(a) = item_actions_as(ui, key, owner, link_button) {
                *asked = Some(a);
            }
        });
    });
}

/// The skipped-items note (the warm note, in the band): heading and count
/// once finished, the sentence, "See up to 8 examples" and Dismiss. True
/// when dismissed.
fn skipped_note(ui: &mut Ui, note: &text::Skipped, why: Option<&str>, examples: &[String], t: &Theme) -> bool {
    let mut dismissed = false;
    let open_id = ui.id().with("disk-space-skipped");
    let mut open = ui.data(|d| d.get_temp::<bool>(open_id).unwrap_or(false));
    egui::Frame::new().fill(t.warm_note).corner_radius(CornerRadius::same(5)).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 6.0;
        if note.heading.is_some() || note.count.is_some() {
            ui.horizontal(|ui| {
                if let Some(h) = &note.heading {
                    ui.label(RichText::new(h).size(13.0).color(t.warm_note_text).strong());
                }
                if let Some(c) = &note.count {
                    ui.label(RichText::new(c).size(13.0).color(t.warm_note_text));
                }
            });
        }
        ui.add(egui::Label::new(RichText::new(&note.sentence).size(13.0).color(t.warm_note_text)).wrap());
        if let Some(why) = why {
            ui.add(egui::Label::new(RichText::new(why).size(13.0).color(t.warm_note_text)).wrap());
        }
        let toggle = if open { "Hide the examples".to_string() } else { format!("See up to {} examples", tree::EXAMPLES) };
        ui.horizontal(|ui| {
            if link_button(ui, &toggle).clicked() {
                open = !open;
            }
            ui.label(RichText::new("\u{00b7}").color(t.warm_note_text));
            dismissed = link_button(ui, "Dismiss").clicked();
        });
        if open {
            for path in examples {
                ui.add(egui::Label::new(RichText::new(path).size(12.0).monospace().color(t.warm_note_text)).truncate());
            }
            ui.label(RichText::new(text::EXAMPLES_NOTE).size(12.0).color(t.label));
        }
    });
    ui.data_mut(|d| d.insert_temp(open_id, open));
    dismissed
}

/// The list of the group chosen (File Size / File Type / Folder Size).
#[allow(clippy::too_many_arguments)]
fn grouped_list(ui: &mut Ui, v: &View, group: Group, n: usize, selected: Option<&nav::Key>, t: &Theme, link: Link, asked: &mut Option<Ask>) {
    let rows = v.rows(group);
    if rows.is_empty() {
        return;
    }
    let title = match group {
        Group::FileSize => format!("Largest {n} files"),
        Group::FileType => text::file_types(rows.len() as u64),
        Group::FolderSize => group.label().to_string(),
    };
    ui.add_space(10.0);
    ui.label(RichText::new(title).font(FontId::new(14.0, theme::semibold())).color(t.text));
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, t.line));
    ui.spacing_mut().item_spacing.y = 0.0;
    for row in rows {
        let cells = Cells {
            swatch: Some(row.category),
            size: text::size(row.bytes),
            size_width: 64.0,
            text: row.path.as_deref().unwrap_or(&row.label),
            path: row.path.is_some(),
            detail: row.detail.as_deref(),
            ..Cells::default()
        };
        match &row.path {
            Some(path) => {
                let p = PathBuf::from(path);
                let key = if group == Group::FolderSize { nav::Key::Folder(p) } else { nav::Key::File(p) };
                let on = selected == Some(&key);
                item_row(ui, &cells, key, on, t, link, asked);
            }
            None => {
                list_row(ui, &cells, false, false, t);
            }
        }
    }
}

/// The File Type legend: colour, name, files and size of each category
/// (sizes of empty ones dimmed).
fn legend(ui: &mut Ui, v: &View, t: &Theme) {
    section_title(ui, "File Type");
    // Before a scan every category shows zeros.
    let rows: Vec<(Category, u64, u64)> = if v.types.is_empty() {
        Category::ALL.iter().map(|&c| (c, 0, 0)).collect()
    } else {
        v.types.iter().map(|t| (t.category, t.files, t.bytes)).collect()
    };
    for (category, files, bytes) in rows {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
        let p = ui.painter();
        let y = rect.center().y;
        swatch(p, Pos2::new(rect.left() + 5.0, y), Some(category));
        p.text(Pos2::new(rect.left() + 18.0, y), Align2::LEFT_CENTER, category.name(), FontId::proportional(13.0), t.text);
        let size_ink = if bytes == 0 { t.faint } else { t.text };
        p.text(Pos2::new(rect.right(), y), Align2::RIGHT_CENTER, text::size(bytes), FontId::proportional(13.0), size_ink);
        p.text(Pos2::new(rect.right() - 66.0, y), Align2::RIGHT_CENTER, text::files(files), FontId::proportional(11.5), t.faint);
    }
}

/// A small dot in the category's colour (nothing for none): the canvas's
/// colours, the same in both themes.
fn swatch(p: &egui::Painter, centre: Pos2, category: Option<Category>) {
    if let Some((red, green, blue)) = category.map(Category::colour) {
        p.circle_filled(centre, 5.0, Color32::from_rgb(red, green, blue));
    }
}

/// The cells of a list row, left to right.
#[derive(Default)]
struct Cells<'a> {
    /// A colour square first (`Some(None)`: an empty slot).
    swatch: Option<Option<Category>>,
    /// A label before the size ("Largest folder").
    label: Option<&'a str>,
    /// The size, right-aligned in `size_width`.
    size: String,
    size_width: f32,
    /// The path (cut in the middle) or a name (cut at the end).
    text: &'a str,
    path: bool,
    /// Grey words on the right.
    detail: Option<&'a str>,
}

/// One list row drawn from `cells`; the full text shows on hover when it
/// had to be cut. Selected rows get the selection fill (the shared row).
fn list_row(ui: &mut Ui, cells: &Cells, selected: bool, sense: bool, t: &Theme) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), if sense { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    row_fill(p, rect, selected, sense && resp.hovered());
    let ink = if selected { t.on_selected } else { t.text };
    let quiet = if selected { t.selected_sub } else { t.label };
    let y = rect.center().y;
    let font = FontId::proportional(13.0);
    let mut x = rect.left() + 8.0;
    if let Some(c) = cells.swatch {
        swatch(p, Pos2::new(x + 5.0, y), c);
        x += 20.0;
    }
    if let Some(l) = cells.label {
        p.text(Pos2::new(x, y), Align2::LEFT_CENTER, l, t.fonts.label.clone(), quiet);
        x += 114.0;
    }
    let size_font = FontId::proportional(13.0);
    p.text(Pos2::new(x + cells.size_width, y), Align2::RIGHT_CENTER, &cells.size, size_font.clone(), ink);
    p.text(Pos2::new(x + cells.size_width + 0.5, y), Align2::RIGHT_CENTER, &cells.size, size_font, ink);
    x += cells.size_width + 10.0;
    let mut right = rect.right() - 8.0;
    if let Some(d) = cells.detail {
        let quiet = if selected { t.selected_sub } else { t.faint };
        let g: std::sync::Arc<Galley> = p.layout_no_wrap(d.to_string(), FontId::proportional(11.5), quiet);
        // The detail gives way to the path in a narrow row.
        if right - g.size().x - 12.0 - x > 120.0 {
            right -= g.size().x;
            p.galley(Pos2::new(right, y - g.size().y / 2.0), g, quiet);
            right -= 12.0;
        }
    }
    let (shown, cut) = if cells.path {
        fit_path(ui, cells.text, &font, right - x)
    } else {
        let g = crate::widgets::fitted(ui, cells.text, 13.0, ink, false, right - x);
        let cut = g.size().x + 1.0 < p.layout_no_wrap(cells.text.to_string(), font.clone(), ink).size().x;
        (g.text().to_string(), cut)
    };
    p.text(Pos2::new(x, y), Align2::LEFT_CENTER, shown, font, ink);
    if cut { resp.on_hover_text(cells.text) } else { resp }
}

/// One item row: [`list_row`] with the item's actions in its context menu.
fn item_row(ui: &mut Ui, cells: &Cells, key: nav::Key, selected: bool, t: &Theme, link: Link, asked: &mut Option<Ask>) {
    let path = cells.text.to_string();
    list_row(ui, cells, selected, true, t).context_menu(|ui| {
        let owner = link(Path::new(&path));
        if let Some(a) = item_actions(ui, &key, owner.as_ref()) {
            *asked = Some(a);
            ui.close();
        }
    });
}

/// One block per drive: name (Michroma), file system, used / free bar in
/// the disk's colour (red past 90%).
fn drive_cards(ui: &mut Ui, disks: &[&DiskRow], color: Color32, t: &Theme) {
    for (i, d) in disks.iter().enumerate() {
        if i > 0 {
            ui.add_space(10.0);
        }
        let used = d.total.saturating_sub(d.free);
        let frac = if d.total > 0 { used as f32 / d.total as f32 } else { 0.0 };
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.horizontal(|ui| {
                let label = if d.name.is_empty() { d.mount.clone() } else { format!("{} ({})", d.mount, d.name) };
                let extra = if d.removable { " \u{2022} removable" } else { "" };
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(format!("{} \u{2022} {}{}", d.fs, d.kind, extra)).font(t.fonts.label.clone()).color(t.label));
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(label).font(t.fonts.figure_small.clone()).color(t.text)).truncate());
                    });
                });
            });
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(3), t.hover);
            let bar = if frac > 0.9 { t.red } else { color };
            ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(r.width() * frac, r.height())), CornerRadius::same(3), bar);
            ui.label(RichText::new(format!("{} free of {}  ({:.0}% used)", fmt_bytes(d.free), fmt_bytes(d.total), frac * 100.0)).font(t.fonts.label.clone()).color(t.label));
        });
    }
}

/// The status strip (bottom): the LED bar, phase word and last scan, and
/// under them the progress and the note.
fn status_strip(ui: &mut Ui, v: &View, t: &Theme) {
    let outer = egui::Frame::NONE.inner_margin(Margin { top: 12, bottom: 2, ..Default::default() });
    egui::TopBottomPanel::bottom("disk-space-strip").frame(outer).show_separator_line(false).show_inside(ui, |ui| {
        egui::Frame::new().fill(t.panel).stroke(Stroke::new(1.0, t.line)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let fill = v.strip_fill();
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    ui.label(RichText::new(v.strip_last()).font(t.fonts.row.clone()).color(t.label));
                    ui.label(RichText::new(v.phase.label()).font(t.fonts.row.clone()).color(t.soft));
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        led_strip(ui, ui.available_width(), LEDS, fill);
                    });
                });
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                if v.running.is_some() {
                    ui.label(RichText::new(text::progress(v.progress, v.files, v.bytes)).font(t.fonts.label.clone()).color(t.label));
                    if let Some(left) = &v.left {
                        ui.label(RichText::new(left).font(t.fonts.label.clone()).color(t.label));
                    }
                    ui.label(RichText::new("The scan keeps running if you switch tabs or hide the window.").font(t.fonts.label.clone()).color(t.label));
                } else {
                    ui.label(RichText::new(history::SAVED_NOTE).font(t.fonts.label.clone()).color(t.label));
                }
            });
        });
    });
}
