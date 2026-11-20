//! The "Global connections" artwork at the top of the side panel: a
//! wireframe globe with lines from its centre to one point per address the
//! selected application talks to. It is a drawing, not a map: a point's
//! place comes from its address alone, so it does not move between readings.

use std::f32::consts::FRAC_PI_2;
use std::net::IpAddr;

use egui::{pos2, vec2, Pos2, RichText, Sense, Shape, Stroke, Ui, WidgetInfo, WidgetType};

use super::inventory::{Inventory, Row, Status};

/// The artwork's title.
pub const TITLE: &str = "Global connections";

/// What a screen reader hears for the artwork: one text each for a full
/// (or not yet read), partial and unavailable inventory.
pub fn description(status: Option<Status>) -> &'static str {
    match status {
        Some(Status::Unavailable) => "Global connections artwork. Connection inventory is currently unavailable.",
        Some(Status::Partial) => {
            "Global connections artwork. Inventory is partial or limited to 20,000 rows. Select a connection to inspect its route and owner."
        }
        Some(Status::Full) | None => "Global connections artwork. Select a connection to inspect its route and owner.",
    }
}

/// The line shown under the globe when the drawing is not the whole
/// picture: the inventory sentence of the partial and unavailable
/// artwork texts. None for a full inventory.
pub fn caption(status: Option<Status>) -> Option<&'static str> {
    match status {
        Some(Status::Partial) => Some("Inventory is partial or limited to 20,000 rows."),
        Some(Status::Unavailable) => Some("Connection inventory is currently unavailable."),
        Some(Status::Full) | None => None,
    }
}

/// The widest longitude and latitude a point takes, in degrees, so points
/// stay on the visible face and off the very rim.
const MAX_LON: f32 = 70.0;
const MAX_LAT: f32 = 60.0;

/// FNV-1a over `bytes`: a hash that is the same on every run and build
/// (std's hasher makes no such promise).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

/// Where `addr` sits on the globe's face, as an offset from the centre in
/// units of the radius (x right, y down; inside the unit disc). The place
/// depends only on the address.
pub fn point(addr: IpAddr) -> (f32, f32) {
    let bytes = match addr {
        IpAddr::V4(a) => a.octets().to_vec(),
        IpAddr::V6(a) => a.octets().to_vec(),
    };
    let h = fnv1a(&bytes);
    // Two independent fractions in [0, 1) from the hash's halves.
    let u = (h & 0xffff_ffff) as f32 / 4_294_967_296.0;
    let v = (h >> 32) as f32 / 4_294_967_296.0;
    let lon = ((u * 2.0 - 1.0) * MAX_LON).to_radians();
    let lat = ((v * 2.0 - 1.0) * MAX_LAT).to_radians();
    // Orthographic projection of the front hemisphere.
    (lat.cos() * lon.sin(), -lat.sin())
}

/// Whether `addr` has no place on the globe: the wildcard (`0.0.0.0`, `::`)
/// is no address at all, and loopback never leaves this computer.
fn off_globe(addr: IpAddr) -> bool {
    let addr = match addr {
        IpAddr::V6(a) => a.to_ipv4_mapped().map_or(addr, IpAddr::V4),
        v4 => v4,
    };
    addr.is_unspecified() || addr.is_loopback()
}

/// The addresses the application owning `rows[index]` talks to: each row of
/// its process gives its remote address, or its local one when it has no
/// remote end (a listener or a datagram socket), leaving out the wildcard
/// and loopback. Once each, in row order.
pub fn targets(rows: &[Row], index: usize) -> Vec<IpAddr> {
    let pid = rows[index].pid;
    let mut out: Vec<IpAddr> = Vec::new();
    for r in rows.iter().filter(|r| r.pid == pid) {
        let addr = r.remote.unwrap_or(r.local).ip();
        if !off_globe(addr) && !out.contains(&addr) {
            out.push(addr);
        }
    }
    out
}

/// The globe's height on the panel.
const HEIGHT: f32 = 168.0;

/// Draws the artwork: the title, the wireframe globe with a line from the
/// centre to each address of the application owning `rows[selected]`
/// (none while nothing is selected), and the caption of a partial or
/// unavailable inventory. Unavailable dims the globe and draws no line.
pub fn draw(ui: &mut Ui, inventory: Option<&Inventory>, selected: Option<usize>) {
    let status = inventory.map(|inv| inv.status);
    let t = crate::theme::of_ui(ui);
    let sub = t.label;
    ui.label(RichText::new(TITLE).font(egui::FontId::new(13.0, crate::theme::semibold())).color(t.soft));
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Image, true, description(status)));
    let unavailable = status == Some(Status::Unavailable);
    let wire = if unavailable { t.globe_wire.gamma_multiply(0.4) } else { t.globe_wire };
    let painter = ui.painter_at(rect);
    let centre = rect.center();
    let r = rect.height().min(rect.width()) / 2.0 - 4.0;
    wireframe(&painter, centre, r, Stroke::new(1.0, wire));
    let addresses = match (inventory, selected) {
        (Some(inv), Some(i)) if !unavailable && i < inv.rows.len() => targets(&inv.rows, i),
        _ => Vec::new(),
    };
    for addr in addresses {
        let (x, y) = point(addr);
        let p = centre + vec2(x, y) * r;
        painter.line_segment([centre, p], Stroke::new(1.5, t.globe_route));
        painter.circle_filled(p, 4.0, t.globe_route);
    }
    painter.circle_filled(centre, 6.0, t.link);
    if let Some(text) = caption(status) {
        ui.label(RichText::new(text).color(sub));
    }
}

/// The globe's outline, meridians every 30° and parallels every 30°, seen
/// from the front.
fn wireframe(painter: &egui::Painter, centre: Pos2, r: f32, stroke: Stroke) {
    painter.circle_stroke(centre, r, stroke);
    const STEPS: usize = 48;
    for deg in [-60.0f32, -30.0, 0.0, 30.0, 60.0] {
        // A meridian is a half ellipse as wide as the sine of its longitude.
        let half = r * deg.to_radians().sin();
        let pts: Vec<Pos2> = (0..=STEPS)
            .map(|k| {
                let t = -FRAC_PI_2 + k as f32 / STEPS as f32 * std::f32::consts::PI;
                pos2(centre.x + half * t.cos(), centre.y + r * t.sin())
            })
            .collect();
        painter.add(Shape::line(pts, stroke));
        // A parallel is a chord at the height of its latitude.
        let (s, c) = deg.to_radians().sin_cos();
        let y = centre.y - r * s;
        painter.line_segment([pos2(centre.x - r * c, y), pos2(centre.x + r * c, y)], stroke);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::connections::inventory::Proto;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn the_same_address_always_lands_on_the_same_point() {
        let a = point(ip("142.250.180.14"));
        assert_eq!(a, point(ip("142.250.180.14")));
        assert_eq!(point(ip("2a00:1450::200e")), point(ip("2a00:1450::200e")));
        assert_ne!(a, point(ip("142.250.180.15")));
    }

    #[test]
    fn every_point_stays_on_the_face_of_the_globe() {
        for i in 0..=255u8 {
            for a in [IpAddr::from([10, 0, i, 1]), IpAddr::from([i, 7, 7, i]), IpAddr::from([0x2a00, 0, 0, 0, 0, 0, 0, u16::from(i)])] {
                let (x, y) = point(a);
                assert!(x * x + y * y < 0.98, "{a} at ({x}, {y})");
            }
        }
    }

    #[test]
    fn the_artwork_speaks_one_text_for_each_inventory_state() {
        assert_eq!(TITLE, "Global connections");
        assert_eq!(description(Some(Status::Full)), "Global connections artwork. Select a connection to inspect its route and owner.");
        assert_eq!(description(None), description(Some(Status::Full)));
        assert_eq!(
            description(Some(Status::Partial)),
            "Global connections artwork. Inventory is partial or limited to 20,000 rows. Select a connection to inspect its route and owner."
        );
        assert_eq!(description(Some(Status::Unavailable)), "Global connections artwork. Connection inventory is currently unavailable.");
    }

    #[test]
    fn only_a_partial_or_unavailable_inventory_has_a_caption() {
        assert_eq!(caption(None), None);
        assert_eq!(caption(Some(Status::Full)), None);
        assert_eq!(caption(Some(Status::Partial)), Some("Inventory is partial or limited to 20,000 rows."));
        assert_eq!(caption(Some(Status::Unavailable)), Some("Connection inventory is currently unavailable."));
    }

    fn row(local: &str, remote: Option<&str>, pid: u32) -> Row {
        let proto = if remote.is_some() { Proto::Tcp } else { Proto::Udp };
        Row { proto, local: local.parse().unwrap(), remote: remote.map(|r| r.parse().unwrap()), state: None, pid }
    }

    #[test]
    fn the_targets_are_the_distinct_addresses_of_the_selected_app() {
        let rows = [
            row("192.168.1.5:50001", Some("142.250.180.14:443"), 7),
            row("192.168.1.5:50002", Some("142.250.180.14:443"), 7),
            row("192.168.1.5:50003", Some("140.82.112.3:443"), 7),
            row("0.0.0.0:5353", None, 7),
            row("192.168.1.5:50004", Some("1.1.1.1:443"), 8),
        ];
        // Remote addresses where there is one, the local one for listeners
        // and datagram sockets; once each, in row order; other apps left out.
        assert_eq!(targets(&rows, 1), [ip("142.250.180.14"), ip("140.82.112.3")], "the wildcard is no place on the globe");
        assert_eq!(targets(&rows, 4), [ip("1.1.1.1")]);
    }

    #[test]
    fn wildcard_and_loopback_addresses_are_not_global_points() {
        let rows = [
            row("0.0.0.0:5353", None, 7),
            row("[::]:5355", None, 7),
            row("127.0.0.1:631", None, 7),
            row("[::1]:7000", None, 7),
            row("127.0.0.1:50000", Some("127.0.0.1:631"), 7),
            row("192.168.1.5:137", None, 7),
            row("192.168.1.5:50001", Some("142.250.180.14:443"), 7),
        ];
        assert_eq!(targets(&rows, 0), [ip("192.168.1.5"), ip("142.250.180.14")]);
    }
}
