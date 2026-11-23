//! Helpers for tests that draw egui frames without a window.

use crate::theme::{self, Mode};
use egui::{Event, Pos2, RawInput, Rect, Shape, Ui};

/// A context drawing in `mode` with the app's faces.
pub fn context(mode: Mode) -> egui::Context {
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    theme::apply(&ctx, mode);
    ctx
}

/// One frame of a `width` x `height` page with `events`.
pub fn run(ctx: &egui::Context, (width, height): (f32, f32), events: Vec<Event>, mut draw: impl FnMut(&mut Ui)) -> egui::FullOutput {
    let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(width, height))), events, ..Default::default() };
    ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| draw(ui));
    })
}

/// Every shape drawn, nested ones flattened.
pub fn flat_shapes(out: &egui::FullOutput) -> Vec<Shape> {
    fn flat(s: &Shape, out: &mut Vec<Shape>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| flat(s, out)),
            s => out.push(s.clone()),
        }
    }
    let mut all = Vec::new();
    out.shapes.iter().for_each(|c| flat(&c.shape, &mut all));
    all
}

/// Every text drawn, in drawing order.
pub fn texts(out: &egui::FullOutput) -> Vec<String> {
    flat_shapes(out)
        .iter()
        .filter_map(|s| match s {
            Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        })
        .collect()
}

/// Every colour drawn: fills, outlines, lines, words and mesh vertices.
pub fn colours(out: &egui::FullOutput) -> Vec<egui::Color32> {
    let mut all = Vec::new();
    for s in flat_shapes(out) {
        match s {
            Shape::Rect(r) => all.extend([r.fill, r.stroke.color]),
            Shape::Circle(c) => all.extend([c.fill, c.stroke.color]),
            Shape::LineSegment { stroke, .. } => all.push(stroke.color),
            Shape::Path(p) => {
                all.push(p.fill);
                if let egui::epaint::ColorMode::Solid(c) = p.stroke.color {
                    all.push(c);
                }
            }
            Shape::Text(t) => all.extend(t.galley.job.sections.iter().map(|s| s.format.color)),
            Shape::Mesh(m) => all.extend(m.vertices.iter().map(|v| v.color)),
            _ => {}
        }
    }
    all
}

/// One frame of a page drawn with `input`: its texts and the actions it asked for.
fn draw_page<R>(input: RawInput, mut draw: impl FnMut(&mut Ui, &mut crate::platform::PageContext) -> R) -> (Vec<String>, Vec<crate::platform::Action>) {
    let (settings, sample) = (crate::settings::Settings::default(), crate::sampler::Snapshot::default());
    let ctx = egui::Context::default();
    // The app's faces, as at start-up: pages draw in them.
    theme::install_fonts(&ctx);
    let mut actions = Vec::new();
    let output = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut page = crate::platform::PageContext::new(&settings, &sample);
            draw(ui, &mut page);
            actions = std::mem::take(&mut page.actions);
        });
    });
    (texts(&output), actions)
}

/// Every text `draw` shows in one frame of a page, a line each, and the actions it asked for.
pub fn drawn_text<R>(draw: impl FnMut(&mut Ui, &mut crate::platform::PageContext) -> R) -> (String, Vec<crate::platform::Action>) {
    let (lines, actions) = draw_page(RawInput::default(), draw);
    (lines.join("\n"), actions)
}

/// Every text `draw` shows in one frame of a page, a line each.
pub fn page_text<R>(draw: impl FnMut(&mut Ui, &mut crate::platform::PageContext) -> R) -> String {
    drawn_text(draw).0
}

/// Every text `draw` shows in one frame of a page, one entry each.
pub fn drawn_lines<R>(draw: impl FnMut(&mut Ui, &mut crate::platform::PageContext) -> R) -> Vec<String> {
    draw_page(RawInput::default(), draw).0
}

/// Every text `draw` shows in a window of `size`, one entry each; text laid
/// out past its edges is not painted.
pub fn drawn_lines_in<R>(size: egui::Vec2, draw: impl FnMut(&mut Ui, &mut crate::platform::PageContext) -> R) -> Vec<String> {
    let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), ..Default::default() };
    draw_page(input, draw).0
}

/// Settings whose CPU colour is the user's own, and that colour.
pub fn custom_cpu() -> (crate::settings::Settings, egui::Color32) {
    let chosen = [0x12, 0x34, 0x56];
    let mut settings = crate::settings::Settings::default();
    settings.cpu_color = chosen;
    (settings, crate::widgets::rgb(chosen))
}

/// Where the string, character literal or comment that starts at `i` ends (the
/// index just past it); `None` when none starts there. Lifetimes are not literals.
fn literal_end(b: &[u8], i: usize) -> Option<usize> {
    let ident = |at: usize| b.get(at).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_');
    match b[i] {
        b'/' if b.get(i + 1) == Some(&b'/') => Some(b[i..].iter().position(|c| *c == b'\n').map_or(b.len(), |n| i + n)),
        b'/' if b.get(i + 1) == Some(&b'*') => {
            let (mut depth, mut at) = (1usize, i + 2);
            while at < b.len() && depth > 0 {
                match (b[at], b.get(at + 1)) {
                    (b'/', Some(b'*')) => (depth, at) = (depth + 1, at + 2),
                    (b'*', Some(b'/')) => (depth, at) = (depth - 1, at + 2),
                    _ => at += 1,
                }
            }
            Some(at.min(b.len()))
        }
        b'"' => {
            let mut at = i + 1;
            while at < b.len() && b[at] != b'"' {
                at += if b[at] == b'\\' { 2 } else { 1 };
            }
            Some((at + 1).min(b.len()))
        }
        b'r' | b'b' if i == 0 || !ident(i - 1) => {
            // r"..", r#".."#, br"..": the closing quote needs as many hashes as the opening one.
            let mut at = i + 1;
            if b[i] == b'b' && b.get(at) == Some(&b'r') {
                at += 1;
            } else if b[i] == b'b' {
                return None;
            }
            let hashes = b[at..].iter().take_while(|c| **c == b'#').count();
            at += hashes;
            if b.get(at) != Some(&b'"') {
                return None;
            }
            let close: Vec<u8> = std::iter::once(b'"').chain(std::iter::repeat(b'#').take(hashes)).collect();
            let found = b[at + 1..].windows(close.len()).position(|w| w == close.as_slice());
            Some(found.map_or(b.len(), |n| at + 1 + n + close.len()))
        }
        b'\'' => {
            // A character literal is one char (or an escape) then a quote; anything else is a lifetime.
            let next = *b.get(i + 1)?;
            if next == b'\\' {
                return Some(b[i + 3..].iter().position(|c| *c == b'\'').map_or(b.len(), |n| i + 4 + n));
            }
            let width = match next {
                0xF0..=0xFF => 4,
                0xE0..=0xEF => 3,
                0xC0..=0xDF => 2,
                _ => 1,
            };
            (b.get(i + 1 + width) == Some(&b'\'')).then_some(i + 2 + width)
        }
        _ => None,
    }
}

/// Where the item that starts `text` (just after its `#[cfg(test)]`) ends: past
/// its closing brace, its `;`, or (for a field) its `,`.
fn item_end(text: &str) -> usize {
    let head = text.trim_start();
    let name = head.split(':').next().unwrap_or("").trim();
    let name = name.strip_prefix("pub").map_or(name, str::trim);
    let is_field = head.contains(':')
        && !name.is_empty()
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !head[name.len()..].trim_start().starts_with("::");
    let b = text.as_bytes();
    let (mut parens, mut braces, mut i) = (0usize, 0usize, 0usize);
    while i < b.len() {
        if let Some(end) = literal_end(b, i) {
            i = end;
            continue;
        }
        match b[i] {
            b'(' | b'[' => parens += 1,
            b')' | b']' => parens = parens.saturating_sub(1),
            b'{' => braces += 1,
            b'}' if braces > 0 => {
                braces -= 1;
                if braces == 0 && parens == 0 {
                    return i + 1;
                }
            }
            b';' if parens == 0 && braces == 0 => return i + 1,
            b',' if parens == 0 && braces == 0 && is_field => return i + 1,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// `src` without its test-only items: every `#[cfg(test)]` attribute and the
/// item, field or statement it marks, wherever it sits in the file. A marker
/// inside a string, character literal or comment is not an attribute.
pub fn non_test_code(src: &str) -> String {
    const MARKER: &str = "#[cfg(test)]";
    let b = src.as_bytes();
    let (mut out, mut kept, mut i) = (String::new(), 0usize, 0usize);
    while i < b.len() {
        if let Some(end) = literal_end(b, i) {
            i = end;
        } else if b[i..].starts_with(MARKER.as_bytes()) {
            out.push_str(&src[kept..i]);
            let after = i + MARKER.len();
            i = after + item_end(&src[after..]);
            kept = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&src[kept..]);
    out
}

#[cfg(test)]
mod scanner_tests {
    use super::non_test_code;

    #[test]
    fn the_scan_drops_every_test_only_item_and_keeps_the_code_after_them() {
        let sample = "fn a() { one }\n#[cfg(test)]\nmod early { fn t() { \"Zed\" } }\nfn b() { two }\n\
                      struct S {\n    #[cfg(test)]\n    recording: Option<Arc<Foo>>,\n    kept: u8,\n}\n\
                      #[cfg(test)]\nuse zed::Zed;\n#[cfg(test)]\npub fn helper<A, B>(x: A, y: B) -> u8 { 1 }\nfn c() { three }\n";
        let code = non_test_code(sample);
        for kept in ["one", "two", "three", "kept: u8"] {
            assert!(code.contains(kept), "{kept} survives: {code}");
        }
        for dropped in ["Zed", "zed", "recording", "helper"] {
            assert!(!code.contains(dropped), "{dropped} is test-only: {code}");
        }
    }

    #[test]
    fn the_scan_finds_an_addon_name_placed_after_an_early_test_marker() {
        let sample = "fn a() {}\n#[cfg(test)]\nmod early {}\nfn leak() { \"Disk Space\" }\n";
        assert!(non_test_code(sample).contains("Disk Space"), "the whole-file scan sees it");
    }

    #[test]
    fn a_brace_or_quote_in_a_character_literal_does_not_end_or_extend_the_item() {
        let sample = "#[cfg(test)]\nmod t { fn f() { let _ = ('{', '}', '\"', '\\'', '\\\\'); } }\nfn kept() { \"Visible\" }\n";
        let code = non_test_code(sample);
        assert!(code.contains("Visible") && !code.contains("mod t"), "{code}");
        let sample = "#[cfg(test)]\nmod t { fn f<'a>(x: &'a str) { let _ = '{'; } }\nfn kept() { Visible }\n";
        let code = non_test_code(sample);
        assert!(code.contains("Visible") && !code.contains("mod t"), "a lifetime is not a literal: {code}");
    }

    #[test]
    fn a_brace_or_quote_in_a_raw_string_does_not_end_or_extend_the_item() {
        let sample = "#[cfg(test)]\nmod t { fn f() { let _ = r#\"} \" {\"#; let _ = r\"}\"; let _ = br\"{\"; } }\nfn kept() { Visible }\n";
        let code = non_test_code(sample);
        assert!(code.contains("Visible") && !code.contains("mod t"), "{code}");
    }

    #[test]
    fn a_brace_in_a_comment_does_not_end_or_extend_the_item() {
        let sample = "#[cfg(test)]\nmod t {\n // } stray\n /* { /* nested } */ } */\n}\nfn kept() { Visible }\n";
        let code = non_test_code(sample);
        assert!(code.contains("Visible") && !code.contains("stray") && !code.contains("mod t"), "{code}");
    }

    #[test]
    fn braces_nested_inside_parentheses_are_balanced() {
        let sample = "#[cfg(test)]\nmod t { fn f() { g(json!({ \"a\": { 1 } })); } }\nfn kept() { Visible }\n";
        let code = non_test_code(sample);
        assert!(code.contains("Visible") && !code.contains("mod t"), "{code}");
    }

    #[test]
    fn a_marker_inside_a_string_or_comment_is_not_an_attribute() {
        let sample = "const A: &str = \"#[cfg(test)]\";\n// #[cfg(test)]\nfn kept() { Visible }\n";
        let code = non_test_code(sample);
        assert!(code.contains("Visible") && code.contains("const A"), "{code}");
    }
}
