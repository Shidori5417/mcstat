//! Minecraft MOTD (`description`) parser and renderer.
//!
//! Supports both legacy `§` formatting codes and structured JSON chat
//! components (`text`, `color` named + `#RRGGBB`, boolean style flags, `extra`
//! inheritance, `translate` fallback, and component arrays where the first
//! element acts as the parent of subsequent siblings).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::{Map, Value};

/// Maximum recursion depth when traversing nested `extra` / array components.
const MAX_COMPONENT_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct McStyle {
    color: Option<Color>,
    bold: bool,
    italic: bool,
    underlined: bool,
    strikethrough: bool,
    obfuscated: bool,
}

impl McStyle {
    fn reset_modifiers(&mut self) {
        self.bold = false;
        self.italic = false;
        self.underlined = false;
        self.strikethrough = false;
        self.obfuscated = false;
    }

    fn reset_all(&mut self) {
        self.color = None;
        self.reset_modifiers();
    }

    fn to_ratatui(self) -> Style {
        let mut style = Style::default();
        if let Some(color) = self.color {
            style = style.fg(color);
        }
        let mut mods = Modifier::empty();
        if self.bold {
            mods |= Modifier::BOLD;
        }
        if self.italic {
            mods |= Modifier::ITALIC;
        }
        if self.underlined {
            mods |= Modifier::UNDERLINED;
        }
        if self.strikethrough {
            mods |= Modifier::CROSSED_OUT;
        }
        // `obfuscated` (`§k`) renders text as-is per spec.
        if !mods.is_empty() {
            style = style.add_modifier(mods);
        }
        style
    }
}

/// Map a legacy `0-9a-f` code char (already lowercased) to its exact Minecraft RGB color.
fn legacy_color(code: char) -> Option<Color> {
    match code {
        '0' => Some(Color::Rgb(0x00, 0x00, 0x00)), // black
        '1' => Some(Color::Rgb(0x00, 0x00, 0xAA)), // dark_blue
        '2' => Some(Color::Rgb(0x00, 0xAA, 0x00)), // dark_green
        '3' => Some(Color::Rgb(0x00, 0xAA, 0xAA)), // dark_aqua
        '4' => Some(Color::Rgb(0xAA, 0x00, 0x00)), // dark_red
        '5' => Some(Color::Rgb(0xAA, 0x00, 0xAA)), // dark_purple
        '6' => Some(Color::Rgb(0xFF, 0xAA, 0x00)), // gold
        '7' => Some(Color::Rgb(0xAA, 0xAA, 0xAA)), // gray
        '8' => Some(Color::Rgb(0x55, 0x55, 0x55)), // dark_gray
        '9' => Some(Color::Rgb(0x55, 0x55, 0xFF)), // blue
        'a' => Some(Color::Rgb(0x55, 0xFF, 0x55)), // green
        'b' => Some(Color::Rgb(0x55, 0xFF, 0xFF)), // aqua
        'c' => Some(Color::Rgb(0xFF, 0x55, 0x55)), // red
        'd' => Some(Color::Rgb(0xFF, 0x55, 0xFF)), // light_purple
        'e' => Some(Color::Rgb(0xFF, 0xFF, 0x55)), // yellow
        'f' => Some(Color::Rgb(0xFF, 0xFF, 0xFF)), // white
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParsedColor {
    Rgb(Color),
    Reset,
}

/// Parse a chat component `"color"` string (named color, `"reset"`, or `#RRGGBB`).
fn parse_component_color(raw: &str) -> Option<ParsedColor> {
    let trimmed = raw.trim();
    if let Some(hex) = trimmed.strip_prefix('#') {
        if hex.len() == 6 && hex.is_ascii() {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(ParsedColor::Rgb(Color::Rgb(r, g, b)));
        }
        return None;
    }

    match trimmed.to_ascii_lowercase().as_str() {
        "black" => Some(ParsedColor::Rgb(Color::Rgb(0x00, 0x00, 0x00))),
        "dark_blue" => Some(ParsedColor::Rgb(Color::Rgb(0x00, 0x00, 0xAA))),
        "dark_green" => Some(ParsedColor::Rgb(Color::Rgb(0x00, 0xAA, 0x00))),
        "dark_aqua" => Some(ParsedColor::Rgb(Color::Rgb(0x00, 0xAA, 0xAA))),
        "dark_red" => Some(ParsedColor::Rgb(Color::Rgb(0xAA, 0x00, 0x00))),
        "dark_purple" => Some(ParsedColor::Rgb(Color::Rgb(0xAA, 0x00, 0xAA))),
        "gold" => Some(ParsedColor::Rgb(Color::Rgb(0xFF, 0xAA, 0x00))),
        "gray" => Some(ParsedColor::Rgb(Color::Rgb(0xAA, 0xAA, 0xAA))),
        "dark_gray" => Some(ParsedColor::Rgb(Color::Rgb(0x55, 0x55, 0x55))),
        "blue" => Some(ParsedColor::Rgb(Color::Rgb(0x55, 0x55, 0xFF))),
        "green" => Some(ParsedColor::Rgb(Color::Rgb(0x55, 0xFF, 0x55))),
        "aqua" => Some(ParsedColor::Rgb(Color::Rgb(0x55, 0xFF, 0xFF))),
        "red" => Some(ParsedColor::Rgb(Color::Rgb(0xFF, 0x55, 0x55))),
        "light_purple" => Some(ParsedColor::Rgb(Color::Rgb(0xFF, 0x55, 0xFF))),
        "yellow" => Some(ParsedColor::Rgb(Color::Rgb(0xFF, 0xFF, 0x55))),
        "white" => Some(ParsedColor::Rgb(Color::Rgb(0xFF, 0xFF, 0xFF))),
        "reset" => Some(ParsedColor::Reset),
        _ => None,
    }
}

fn resolve_object_style(obj: &Map<String, Value>, parent: McStyle) -> McStyle {
    let mut style = parent;
    if let Some(color_str) = obj.get("color").and_then(Value::as_str) {
        match parse_component_color(color_str) {
            Some(ParsedColor::Rgb(c)) => style.color = Some(c),
            Some(ParsedColor::Reset) => style.color = None,
            None => {}
        }
    }
    if let Some(b) = obj.get("bold").and_then(Value::as_bool) {
        style.bold = b;
    }
    if let Some(b) = obj.get("italic").and_then(Value::as_bool) {
        style.italic = b;
    }
    if let Some(b) = obj.get("underlined").and_then(Value::as_bool) {
        style.underlined = b;
    }
    if let Some(b) = obj.get("strikethrough").and_then(Value::as_bool) {
        style.strikethrough = b;
    }
    if let Some(b) = obj.get("obfuscated").and_then(Value::as_bool) {
        style.obfuscated = b;
    }
    style
}

fn resolve_component_style(value: &Value, parent: McStyle, depth: usize) -> McStyle {
    if depth > MAX_COMPONENT_DEPTH {
        return parent;
    }
    match value {
        Value::Object(obj) => resolve_object_style(obj, parent),
        Value::Array(items) => match items.first() {
            Some(first) => resolve_component_style(first, parent, depth + 1),
            None => parent,
        },
        _ => parent,
    }
}

struct LineBuilder {
    lines: Vec<Vec<(String, Style)>>,
    has_content: bool,
}

impl LineBuilder {
    fn new() -> Self {
        Self {
            lines: vec![Vec::new()],
            has_content: false,
        }
    }

    fn push_newline(&mut self) {
        self.has_content = true;
        self.lines.push(Vec::new());
    }

    fn push_text(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        self.has_content = true;
        if let Some(current_line) = self.lines.last_mut() {
            if let Some((prev_text, prev_style)) = current_line.last_mut()
                && *prev_style == style
            {
                prev_text.push_str(text);
                return;
            }
            current_line.push((text.to_owned(), style));
        }
    }

    fn push_legacy_str(&mut self, input: &str, initial_style: McStyle) {
        let mut style = initial_style;
        let mut chunk = String::new();
        let mut chars = input.chars().peekable();

        while let Some(ch) = chars.next() {
            match ch {
                '§' => match chars.peek().copied() {
                    None => {}
                    Some('\n' | '\r') => {}
                    Some(raw_code) => {
                        let _ = chars.next();
                        let code = raw_code.to_ascii_lowercase();
                        if let Some(color) = legacy_color(code) {
                            self.flush_chunk(&mut chunk, style);
                            style.color = Some(color);
                            // Vanilla Minecraft: a color code resets formatting modifiers.
                            style.reset_modifiers();
                        } else {
                            match code {
                                'k' => {
                                    self.flush_chunk(&mut chunk, style);
                                    style.obfuscated = true;
                                }
                                'l' => {
                                    self.flush_chunk(&mut chunk, style);
                                    style.bold = true;
                                }
                                'm' => {
                                    self.flush_chunk(&mut chunk, style);
                                    style.strikethrough = true;
                                }
                                'n' => {
                                    self.flush_chunk(&mut chunk, style);
                                    style.underlined = true;
                                }
                                'o' => {
                                    self.flush_chunk(&mut chunk, style);
                                    style.italic = true;
                                }
                                'r' => {
                                    self.flush_chunk(&mut chunk, style);
                                    style.reset_all();
                                }
                                _ => {
                                    // Unknown code: ignore without altering current style.
                                }
                            }
                        }
                    }
                },
                '\r' => {
                    // Normalize `\r\n` or standalone `\r` safely.
                }
                '\n' => {
                    self.flush_chunk(&mut chunk, style);
                    self.push_newline();
                }
                '\t' => {
                    chunk.push(' ');
                }
                c if c.is_control() => {
                    // Drop raw control characters (e.g. ESC) from untrusted server responses.
                }
                c => {
                    chunk.push(c);
                }
            }
        }

        self.flush_chunk(&mut chunk, style);
    }

    fn flush_chunk(&mut self, chunk: &mut String, style: McStyle) {
        if !chunk.is_empty() {
            self.push_text(chunk, style.to_ratatui());
            chunk.clear();
        }
    }

    fn visit(&mut self, value: &Value, parent_style: McStyle, depth: usize) {
        if depth > MAX_COMPONENT_DEPTH {
            return;
        }
        match value {
            Value::String(s) => {
                self.push_legacy_str(s, parent_style);
            }
            Value::Array(items) => {
                if let Some((first, rest)) = items.split_first() {
                    let head_style = resolve_component_style(first, parent_style, depth + 1);
                    self.visit(first, parent_style, depth + 1);
                    for sibling in rest {
                        self.visit(sibling, head_style, depth + 1);
                    }
                }
            }
            Value::Object(obj) => {
                let comp_style = resolve_object_style(obj, parent_style);
                if let Some(text_val) = obj.get("text") {
                    match text_val {
                        Value::String(s) => self.push_legacy_str(s, comp_style),
                        Value::Number(n) => self.push_legacy_str(&n.to_string(), comp_style),
                        Value::Bool(b) => {
                            self.push_legacy_str(if *b { "true" } else { "false" }, comp_style);
                        }
                        _ => {}
                    }
                } else if let Some(key) = obj.get("translate").and_then(Value::as_str) {
                    self.push_legacy_str(key, comp_style);
                }

                if let Some(extra) = obj.get("extra") {
                    match extra {
                        Value::Array(children) => {
                            for child in children {
                                self.visit(child, comp_style, depth + 1);
                            }
                        }
                        Value::Object(_) | Value::String(_) => {
                            self.visit(extra, comp_style, depth + 1);
                        }
                        _ => {}
                    }
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }

    fn finish(self) -> Vec<Line<'static>> {
        if !self.has_content {
            return Vec::new();
        }
        self.lines
            .into_iter()
            .map(|spans| {
                let owned_spans: Vec<Span<'static>> = spans
                    .into_iter()
                    .map(|(text, style)| Span::styled(text, style))
                    .collect();
                Line::from(owned_spans)
            })
            .collect()
    }
}

/// Convert a Minecraft MOTD (`description`) into styled lines.
/// Supports: plain JSON string with legacy `§` codes (0-9a-f colors,
/// k obfuscated, l bold, m strikethrough, n underline, o italic, r reset),
/// chat component objects (`text`, `color` incl. named colors and `#RRGGBB`,
/// `bold`/`italic`/`underlined`/`strikethrough`/`obfuscated`, `extra` with
/// style inheritance, `translate` fallback to the key), arrays of components,
/// and `\n` line breaks.
pub fn motd_to_lines(motd: &Value) -> Vec<Line<'static>> {
    let mut builder = LineBuilder::new();
    builder.visit(motd, McStyle::default(), 0);
    builder.finish()
}

/// Plain-text version of the MOTD (formatting removed, lines joined by
/// a space) for the narrow table column.
pub fn motd_plain(motd: &Value) -> String {
    let lines = motd_to_lines(motd);
    let mut parts: Vec<String> = Vec::with_capacity(lines.len());
    for line in lines {
        let mut raw = String::new();
        for span in line.spans {
            raw.push_str(&span.content);
        }
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            parts.push(trimmed.to_owned());
        }
    }
    parts.join(" ")
}

/// Strip legacy `§` formatting codes and control characters from auxiliary
/// server strings (such as `version.name` or `players.sample` entries).
pub(crate) fn strip_legacy_codes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '§' => {
                if let Some(&next) = chars.peek()
                    && next != '\n'
                    && next != '\r'
                {
                    let _ = chars.next();
                }
            }
            '\t' | '\n' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_all_sixteen_colors_match_minecraft_palette() {
        let cases = [
            ('0', Color::Rgb(0x00, 0x00, 0x00)),
            ('1', Color::Rgb(0x00, 0x00, 0xAA)),
            ('2', Color::Rgb(0x00, 0xAA, 0x00)),
            ('3', Color::Rgb(0x00, 0xAA, 0xAA)),
            ('4', Color::Rgb(0xAA, 0x00, 0x00)),
            ('5', Color::Rgb(0xAA, 0x00, 0xAA)),
            ('6', Color::Rgb(0xFF, 0xAA, 0x00)),
            ('7', Color::Rgb(0xAA, 0xAA, 0xAA)),
            ('8', Color::Rgb(0x55, 0x55, 0x55)),
            ('9', Color::Rgb(0x55, 0x55, 0xFF)),
            ('a', Color::Rgb(0x55, 0xFF, 0x55)),
            ('b', Color::Rgb(0x55, 0xFF, 0xFF)),
            ('c', Color::Rgb(0xFF, 0x55, 0x55)),
            ('d', Color::Rgb(0xFF, 0x55, 0xFF)),
            ('e', Color::Rgb(0xFF, 0xFF, 0x55)),
            ('f', Color::Rgb(0xFF, 0xFF, 0xFF)),
        ];

        for (code, expected_rgb) in cases {
            let lines = motd_to_lines(&json!(format!("§{code}X")));
            assert_eq!(lines.len(), 1);
            assert_eq!(lines[0].spans.len(), 1);
            assert_eq!(lines[0].spans[0].content.as_ref(), "X");
            assert_eq!(lines[0].spans[0].style.fg, Some(expected_rgb));

            // Uppercase hex codes (A-F) must work identically.
            let upper = code.to_ascii_uppercase();
            let upper_lines = motd_to_lines(&json!(format!("§{upper}X")));
            assert_eq!(upper_lines[0].spans[0].style.fg, Some(expected_rgb));
        }
    }

    #[test]
    fn legacy_modifiers_color_reset_and_full_reset() {
        let motd = json!("§a§l§m§n§oStyled§cJustRed§kObf§rPlain");
        let lines = motd_to_lines(&motd);
        assert_eq!(lines.len(), 1);
        let spans = &lines[0].spans;
        assert_eq!(spans.len(), 3);

        // First span: green + bold + strikethrough + underline + italic
        assert_eq!(spans[0].content.as_ref(), "Styled");
        assert_eq!(spans[0].style.fg, Some(Color::Rgb(0x55, 0xFF, 0x55)));
        assert!(spans[0].style.add_modifier.contains(
            Modifier::BOLD | Modifier::CROSSED_OUT | Modifier::UNDERLINED | Modifier::ITALIC
        ));

        // Second span: `§c` sets red AND clears modifiers; `§k` renders text as-is with same style
        assert_eq!(spans[1].content.as_ref(), "JustRedObf");
        assert_eq!(spans[1].style.fg, Some(Color::Rgb(0xFF, 0x55, 0x55)));
        assert_eq!(spans[1].style.add_modifier, Modifier::empty());

        // Third span: `§r` clears color and modifiers
        assert_eq!(spans[2].content.as_ref(), "Plain");
        assert_eq!(spans[2].style.fg, None);
        assert_eq!(spans[2].style.add_modifier, Modifier::empty());
    }

    #[test]
    fn component_hex_colors_named_colors_and_extra_inheritance() {
        let motd = json!({
            "text": "Root ",
            "color": "#12ABef",
            "bold": true,
            "italic": true,
            "extra": [
                {
                    "text": "ChildInherit "
                },
                {
                    "text": "ChildOverride ",
                    "color": "gold",
                    "bold": false,
                    "underlined": true,
                    "strikethrough": true
                },
                "PlainSibling"
            ]
        });

        let lines = motd_to_lines(&motd);
        assert_eq!(lines.len(), 1);
        let spans = &lines[0].spans;
        assert_eq!(spans.len(), 3);

        // "Root " and "ChildInherit " share the exact same style and merge cleanly
        assert_eq!(spans[0].content.as_ref(), "Root ChildInherit ");
        assert_eq!(spans[0].style.fg, Some(Color::Rgb(0x12, 0xAB, 0xEF)));
        assert_eq!(
            spans[0].style.add_modifier,
            Modifier::BOLD | Modifier::ITALIC
        );

        // "ChildOverride " overrides color to gold, bold=false, adds underlined+strikethrough, keeps italic
        assert_eq!(spans[1].content.as_ref(), "ChildOverride ");
        assert_eq!(spans[1].style.fg, Some(Color::Rgb(0xFF, 0xAA, 0x00)));
        assert_eq!(
            spans[1].style.add_modifier,
            Modifier::ITALIC | Modifier::UNDERLINED | Modifier::CROSSED_OUT
        );

        // Plain string inside `extra` inherits parent component style
        assert_eq!(spans[2].content.as_ref(), "PlainSibling");
        assert_eq!(spans[2].style.fg, Some(Color::Rgb(0x12, 0xAB, 0xEF)));
        assert_eq!(
            spans[2].style.add_modifier,
            Modifier::BOLD | Modifier::ITALIC
        );
    }

    #[test]
    fn array_first_element_acts_as_parent_of_rest() {
        let motd = json!([
            { "text": "Header: ", "color": "aqua", "bold": true },
            { "text": "Sub1 " },
            { "text": "Sub2", "color": "#FF0080", "bold": false }
        ]);

        let lines = motd_to_lines(&motd);
        assert_eq!(lines.len(), 1);
        let spans = &lines[0].spans;
        assert_eq!(spans.len(), 2);

        assert_eq!(spans[0].content.as_ref(), "Header: Sub1 ");
        assert_eq!(spans[0].style.fg, Some(Color::Rgb(0x55, 0xFF, 0xFF)));
        assert_eq!(spans[0].style.add_modifier, Modifier::BOLD);

        assert_eq!(spans[1].content.as_ref(), "Sub2");
        assert_eq!(spans[1].style.fg, Some(Color::Rgb(0xFF, 0x00, 0x80)));
        assert_eq!(spans[1].style.add_modifier, Modifier::empty());
    }

    #[test]
    fn translate_fallback_and_legacy_inside_component() {
        let motd = json!({
            "translate": "multiplayer.disconnect.server_shutdown",
            "color": "red",
            "extra": [
                { "text": " §eSoon™" }
            ]
        });

        let lines = motd_to_lines(&motd);
        assert_eq!(lines.len(), 1);
        let spans = &lines[0].spans;
        assert_eq!(spans.len(), 2);
        assert_eq!(
            spans[0].content.as_ref(),
            "multiplayer.disconnect.server_shutdown "
        );
        assert_eq!(spans[0].style.fg, Some(Color::Rgb(0xFF, 0x55, 0x55)));
        assert_eq!(spans[1].content.as_ref(), "Soon™");
        assert_eq!(spans[1].style.fg, Some(Color::Rgb(0xFF, 0xFF, 0x55)));
    }

    #[test]
    fn multiline_splitting_and_plain_joining() {
        let motd = json!("   §6§lHYPIXEL NETWORK   \r\n   §e1.8 - 1.21 §7| §aSKYBLOCK   ");
        let lines = motd_to_lines(&motd);
        assert_eq!(lines.len(), 2);
        assert_eq!(motd_plain(&motd), "HYPIXEL NETWORK 1.8 - 1.21 | SKYBLOCK");

        // Blank middle line preserved in `motd_to_lines`, collapsed cleanly in `motd_plain`
        let gapped = json!("Line1\n\nLine3");
        let gapped_lines = motd_to_lines(&gapped);
        assert_eq!(gapped_lines.len(), 3);
        assert!(gapped_lines[1].spans.is_empty());
        assert_eq!(motd_plain(&gapped), "Line1 Line3");
    }

    #[test]
    fn unknown_codes_malformed_inputs_and_deep_nesting_never_panic() {
        // Unknown `§` code, trailing `§`, `§` before newline, invalid hex colors (incl. UTF-8)
        let weird = json!({
            "text": "He§zllo§\n§xWorld§",
            "color": "#🎉🎉",
            "extra": [
                { "text": "BadHex", "color": "#12345" },
                { "text": "BadName", "color": "ultraviolet" },
                null,
                42,
                true
            ]
        });
        let lines = motd_to_lines(&weird);
        assert_eq!(lines.len(), 2);
        assert_eq!(motd_plain(&weird), "Hello WorldBadHexBadName");

        // Top-level null, number, empty array, empty string
        assert!(motd_to_lines(&json!(null)).is_empty());
        assert!(motd_to_lines(&json!(999)).is_empty());
        assert!(motd_to_lines(&json!([])).is_empty());
        assert!(motd_to_lines(&json!("")).is_empty());
        assert_eq!(motd_plain(&json!(null)), "");
        assert_eq!(motd_plain(&json!("")), "");

        // Deeply nested `extra` (> MAX_COMPONENT_DEPTH) must terminate gracefully
        let mut deep = json!({ "text": "leaf", "color": "green" });
        for _ in 0..150 {
            deep = json!({ "text": "p", "extra": [deep] });
        }
        let deep_lines = motd_to_lines(&deep);
        assert_eq!(deep_lines.len(), 1);
        assert!(!motd_plain(&deep).is_empty());
    }

    #[test]
    fn strip_legacy_codes_cleans_auxiliary_strings() {
        assert_eq!(
            strip_legacy_codes("§cMaintenance §71.21.4"),
            "Maintenance 1.21.4"
        );
        assert_eq!(strip_legacy_codes("Paper 1.21.4"), "Paper 1.21.4");
        assert_eq!(strip_legacy_codes("Trailing§"), "Trailing");
        assert_eq!(strip_legacy_codes("Esc\x1b[31mTest"), "Esc[31mTest");
    }
}
