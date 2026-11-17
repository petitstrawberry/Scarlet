//! Pure keyboard layout shared by the panel and native geometry checks.
use sws_protocol::input_panel;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Layer {
    Letters,
    Symbols,
    MoreSymbols,
    Tools,
    Function,
}
impl Layer {
    /// Remember the tools preference, but retire transient symbol/Fn states.
    pub fn for_activation(self) -> Self {
        match self {
            Self::Tools | Self::Function => Self::Tools,
            _ => Self::Letters,
        }
    }
    pub fn toggle_function(self) -> Self {
        match self {
            Self::Tools => Self::Function,
            Self::Function => Self::Tools,
            _ => self,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Key(u16, u32),
    Modifier(u32),
    SetLayer(Layer),
    ToggleFunction,
    Hide,
}
#[derive(Clone, Copy)]
pub(super) enum Tone {
    Character,
    Utility,
    Toolbar,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RowGeometry {
    Tablet,
    Fill,
}
#[derive(Clone)]
pub(super) struct Key {
    pub plain: &'static str,
    pub shifted: &'static str,
    pub action: Action,
    pub weight: f32,
    pub tone: Tone,
}
#[derive(Clone)]
pub(super) struct KeyRow {
    pub keys: Vec<Key>,
    pub height: f32,
    pub geometry: RowGeometry,
    pub inset: f32,
}
fn key(label: &'static str, shifted: &'static str, code: u16) -> Key {
    Key {
        plain: label,
        shifted,
        action: Action::Key(code, 0),
        weight: 1.0,
        tone: Tone::Character,
    }
}
fn stroke(label: &'static str, code: u16, modifiers: u32) -> Key {
    Key {
        plain: label,
        shifted: label,
        action: Action::Key(code, modifiers),
        weight: 1.0,
        tone: Tone::Character,
    }
}
fn control(label: &'static str, action: Action, weight: f32) -> Key {
    Key {
        plain: label,
        shifted: label,
        action,
        weight,
        tone: Tone::Utility,
    }
}
fn toolbar(label: &'static str, action: Action) -> Key {
    Key {
        plain: label,
        shifted: label,
        action,
        weight: 1.0,
        tone: Tone::Toolbar,
    }
}
fn row(keys: Vec<Key>) -> KeyRow {
    KeyRow {
        keys,
        height: 1.0,
        geometry: RowGeometry::Tablet,
        inset: 0.0,
    }
}
fn fill_row(keys: Vec<Key>) -> KeyRow {
    KeyRow {
        keys,
        height: 1.0,
        geometry: RowGeometry::Fill,
        inset: 0.0,
    }
}
fn terminal_row(function_keys: bool) -> KeyRow {
    use Action::Key;
    KeyRow {
        keys: if function_keys {
            [
                ("F1", 59),
                ("F2", 60),
                ("F3", 61),
                ("F4", 62),
                ("F5", 63),
                ("F6", 64),
                ("F7", 65),
                ("F8", 66),
                ("F9", 67),
                ("F10", 68),
                ("F11", 87),
                ("F12", 88),
            ]
            .into_iter()
            .map(|(label, code)| toolbar(label, Key(code, 0)))
            .collect()
        } else {
            vec![
                toolbar("esc", Key(1, 0)),
                toolbar("tab", Key(15, 0)),
                toolbar("home", Key(102, 0)),
                toolbar("end", Key(107, 0)),
                toolbar("pg up", Key(104, 0)),
                toolbar("pg dn", Key(109, 0)),
                toolbar("←", Key(105, 0)),
                toolbar("↑", Key(103, 0)),
                toolbar("↓", Key(108, 0)),
                toolbar("→", Key(106, 0)),
            ]
        },
        height: 0.73,
        geometry: RowGeometry::Fill,
        inset: 0.0,
    }
}
pub(super) fn rows(numeric: bool, layer: Layer) -> Vec<KeyRow> {
    use Action::*;
    if matches!(layer, Layer::Tools | Layer::Function) {
        // Keep letters available beside latched Ctrl/Alt for terminal shortcuts.
        let mut layout = rows(false, Layer::Letters);
        layout.insert(0, terminal_row(layer == Layer::Function));
        *layout.last_mut().unwrap() = row(vec![
            control(
                if numeric { "123" } else { "ABC" },
                SetLayer(Layer::Letters),
                1.75,
            ),
            control("あ/A", Key(85, 0), 1.3),
            control("ctrl", Modifier(input_panel::CTRL), 1.0),
            control("space", Key(57, 0), 4.05),
            control("alt", Modifier(input_panel::ALT), 1.0),
            control("fn", ToggleFunction, 1.0),
            control("123", SetLayer(Layer::Symbols), 1.05),
            control("hide", Hide, 0.85),
        ]);
        return layout;
    }
    if numeric {
        return vec![
            fill_row(vec![
                key("7", "7", 8),
                key("8", "8", 9),
                key("9", "9", 10),
                control("⌫", Key(14, 0), 1.25),
            ]),
            fill_row(vec![
                key("4", "4", 5),
                key("5", "5", 6),
                key("6", "6", 7),
                key("-", "-", 12),
            ]),
            fill_row(vec![
                key("1", "1", 2),
                key("2", "2", 3),
                key("3", "3", 4),
                key(".", ".", 52),
            ]),
            fill_row(vec![
                control("more", SetLayer(Layer::Tools), 0.8),
                control("hide", Hide, 0.8),
                control("0", Key(11, 0), 2.0),
                control("return", Key(28, 0), 1.25),
            ]),
        ];
    }
    if matches!(layer, Layer::Symbols | Layer::MoreSymbols) {
        let more = layer == Layer::MoreSymbols;
        let mut top = if more {
            vec![
                key("[", "[", 26),
                key("]", "]", 27),
                stroke("{", 26, input_panel::SHIFT),
                stroke("}", 27, input_panel::SHIFT),
                stroke("#", 4, input_panel::SHIFT),
                stroke("%", 6, input_panel::SHIFT),
                stroke("^", 7, input_panel::SHIFT),
                stroke("*", 9, input_panel::SHIFT),
                stroke("+", 13, input_panel::SHIFT),
                key("=", "=", 13),
            ]
        } else {
            vec![
                key("1", "1", 2),
                key("2", "2", 3),
                key("3", "3", 4),
                key("4", "4", 5),
                key("5", "5", 6),
                key("6", "6", 7),
                key("7", "7", 8),
                key("8", "8", 9),
                key("9", "9", 10),
                key("0", "0", 11),
            ]
        };
        top.push(control("⌫", Key(14, 0), 1.55));
        let mut home = if more {
            vec![
                stroke("_", 12, input_panel::SHIFT),
                key("\\", "\\", 43),
                stroke("|", 43, input_panel::SHIFT),
                stroke("~", 41, input_panel::SHIFT),
                key("`", "`", 41),
                stroke("<", 51, input_panel::SHIFT),
                stroke(">", 52, input_panel::SHIFT),
                stroke("$", 5, input_panel::SHIFT),
                stroke("&", 8, input_panel::SHIFT),
            ]
        } else {
            vec![
                key("-", "-", 12),
                key("/", "/", 53),
                stroke(":", 39, input_panel::SHIFT),
                key(";", ";", 39),
                stroke("(", 10, input_panel::SHIFT),
                stroke(")", 11, input_panel::SHIFT),
                stroke("$", 5, input_panel::SHIFT),
                stroke("&", 8, input_panel::SHIFT),
                stroke("@", 3, input_panel::SHIFT),
            ]
        };
        home.push(control("return", Key(28, 0), 2.2));
        // Keep symbol paging beside the symbols; alphabet switches belong on
        // the shared bottom row, reachable from either hand.
        let mut lower = vec![
            control(
                if more { "123" } else { "#+=" },
                SetLayer(if more {
                    Layer::Symbols
                } else {
                    Layer::MoreSymbols
                }),
                1.55,
            ),
            key(".", ".", 52),
            key(",", ",", 51),
            stroke("?", 53, input_panel::SHIFT),
            stroke("!", 2, input_panel::SHIFT),
            key("'", "'", 40),
            stroke("\"", 40, input_panel::SHIFT),
        ];
        lower.extend(if more {
            vec![
                stroke(":", 39, input_panel::SHIFT),
                key(";", ";", 39),
                stroke("(", 10, input_panel::SHIFT),
                stroke(")", 11, input_panel::SHIFT),
            ]
        } else {
            vec![
                stroke("+", 13, input_panel::SHIFT),
                key("=", "=", 13),
                stroke("_", 12, input_panel::SHIFT),
                stroke("%", 6, input_panel::SHIFT),
            ]
        });
        return vec![
            row(top),
            inset_row(home),
            row(lower),
            bottom_row("ABC", Layer::Letters),
        ];
    }
    vec![
        row(vec![
            key("q", "Q", 16),
            key("w", "W", 17),
            key("e", "E", 18),
            key("r", "R", 19),
            key("t", "T", 20),
            key("y", "Y", 21),
            key("u", "U", 22),
            key("i", "I", 23),
            key("o", "O", 24),
            key("p", "P", 25),
            control("⌫", Key(14, 0), 1.55),
        ]),
        inset_row(vec![
            key("a", "A", 30),
            key("s", "S", 31),
            key("d", "D", 32),
            key("f", "F", 33),
            key("g", "G", 34),
            key("h", "H", 35),
            key("j", "J", 36),
            key("k", "K", 37),
            key("l", "L", 38),
            control("return", Key(28, 0), 2.2),
        ]),
        row(vec![
            // Stagger Z between A and S while keeping full-size touch targets.
            control("shift", Modifier(input_panel::SHIFT), 1.0),
            key("z", "Z", 44),
            key("x", "X", 45),
            key("c", "C", 46),
            key("v", "V", 47),
            key("b", "B", 48),
            key("n", "N", 49),
            key("m", "M", 50),
            key(",", "<", 51),
            key(".", ">", 52),
            control("shift", Modifier(input_panel::SHIFT), 1.55),
        ]),
        bottom_row("123", Layer::Symbols),
    ]
}

// Shared units keep letter widths stable and row ends aligned. The home row
// starts half a key inward; the wider Return fills its remaining right edge.
pub(super) const GRID_UNITS: f32 = 13.35;
pub(super) const EDGE_UNITS: f32 = 0.15;
pub(super) const GAP_UNITS: f32 = 0.15;

// Reserve an untargetable strip above the compositor's bottom navigation edge.
// This is additional to the normal key margin, including on numeric/Fn layouts.
pub(super) const BOTTOM_SAFE_AREA: f32 = 16.0;

pub(super) fn panel_height(width: f32, screen_height: f32) -> f32 {
    // Grow the requested surface to retain key heights where screen space allows.
    (width * 0.35 + BOTTOM_SAFE_AREA).min(screen_height * 0.62)
}

pub(super) struct VerticalGeometry {
    pub bottom_padding: f32,
    pub row_unit: f32,
}
impl VerticalGeometry {
    pub fn new(height: f32, top_padding: f32, gap: f32, rows: &[KeyRow]) -> Self {
        let bottom_padding = top_padding + BOTTOM_SAFE_AREA;
        let available =
            (height - top_padding - bottom_padding - gap * (rows.len() - 1) as f32).max(0.0);
        Self {
            bottom_padding,
            row_unit: available / rows.iter().map(|row| row.height).sum::<f32>(),
        }
    }
}

fn inset_row(keys: Vec<Key>) -> KeyRow {
    KeyRow {
        inset: 0.5,
        ..row(keys)
    }
}
fn bottom_row(label: &'static str, layer: Layer) -> KeyRow {
    use Action::*;
    row(vec![
        control(label, SetLayer(layer), 1.75),
        control("あ/A", Key(85, 0), 1.3),
        control("space", Key(57, 0), 6.35),
        control("more", SetLayer(Layer::Tools), 1.0),
        control(label, SetLayer(layer), 1.05),
        control("hide", Hide, 0.85),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_is_centered_without_reducing_its_width() {
        for layer in [
            Layer::Letters,
            Layer::Symbols,
            Layer::MoreSymbols,
            Layer::Tools,
            Layer::Function,
        ] {
            let layout = rows(false, layer);
            let bottom = layout.last().unwrap();
            let index = bottom
                .keys
                .iter()
                .position(|key| key.action == Action::Key(57, 0))
                .unwrap();
            let space = &bottom.keys[index];
            let center = EDGE_UNITS
                + bottom.keys[..index]
                    .iter()
                    .map(|key| key.weight)
                    .sum::<f32>()
                + GAP_UNITS * index as f32
                + space.weight * 0.5;
            assert!((center - GRID_UNITS * 0.5).abs() < 0.0001);
            assert_eq!(
                space.weight,
                if matches!(layer, Layer::Tools | Layer::Function) {
                    4.05
                } else {
                    6.35
                }
            );
        }
    }

    #[test]
    fn fn_replaces_the_toolbar_without_adding_rows_or_changing_other_keys() {
        for numeric in [false, true] {
            let tools = rows(numeric, Layer::Tools);
            let function = rows(numeric, Layer::Function);
            assert_eq!(tools.len(), function.len());
            assert_eq!(tools[0].height, function[0].height);
            assert_eq!(function[0].keys.len(), 12);
            for (key, (label, code)) in function[0].keys.iter().zip([
                ("F1", 59),
                ("F2", 60),
                ("F3", 61),
                ("F4", 62),
                ("F5", 63),
                ("F6", 64),
                ("F7", 65),
                ("F8", 66),
                ("F9", 67),
                ("F10", 68),
                ("F11", 87),
                ("F12", 88),
            ]) {
                assert_eq!(key.plain, label);
                assert!(key.action == Action::Key(code, 0));
            }
            for (before, after) in tools.iter().skip(1).zip(function.iter().skip(1)) {
                assert_eq!(before.height, after.height);
                assert_eq!(before.keys.len(), after.keys.len());
                for (before, after) in before.keys.iter().zip(&after.keys) {
                    assert_eq!(before.plain, after.plain);
                    assert_eq!(before.weight, after.weight);
                    assert!(before.action == after.action);
                }
            }
            assert!(
                function
                    .last()
                    .unwrap()
                    .keys
                    .iter()
                    .any(|key| key.action == Action::ToggleFunction)
            );
        }
    }

    #[test]
    fn tools_preference_survives_close_and_reactivation_but_fn_is_transient() {
        let mut layer = Layer::Tools;
        layer = layer.toggle_function();
        assert!(layer == Layer::Function);
        layer = layer.toggle_function();
        assert!(layer == Layer::Tools);
        layer = layer.toggle_function().for_activation();
        assert!(layer == Layer::Tools);
        assert!(layer.for_activation() == Layer::Tools);
        assert!(Layer::Letters.for_activation() == Layer::Letters);
        assert!(Layer::Symbols.for_activation() == Layer::Letters);
    }

    #[test]
    fn symbol_pages_have_bottom_abc_shortcuts_and_bidirectional_page_switches() {
        for (layer, toggle, target) in [
            (Layer::Symbols, "#+=", Layer::MoreSymbols),
            (Layer::MoreSymbols, "123", Layer::Symbols),
        ] {
            let layout = rows(false, layer);
            let keys: Vec<_> = layout.iter().flat_map(|row| &row.keys).collect();
            let abc: Vec<_> = keys.iter().filter(|key| key.plain == "ABC").collect();
            assert_eq!(abc.len(), 2);
            assert!(
                abc.iter()
                    .all(|key| key.action == Action::SetLayer(Layer::Letters))
            );
            assert_eq!(layout.last().unwrap().keys[0].plain, "ABC");
            let bottom = &layout.last().unwrap().keys;
            assert_eq!(bottom[bottom.len() - 2].plain, "ABC");
            assert_eq!(layout[2].keys[0].plain, toggle);
            assert!(layout[2].keys[0].action == Action::SetLayer(target));
            assert_eq!(
                keys.iter()
                    .filter(|key| key.action == Action::SetLayer(target))
                    .count(),
                1
            );
        }
        let symbols = rows(false, Layer::Symbols);
        let digits: String = symbols[0]
            .keys
            .iter()
            .take(10)
            .map(|key| key.plain)
            .collect();
        assert_eq!(digits, "1234567890");
    }

    #[test]
    fn all_ascii_punctuation_is_reachable_with_correct_keycodes_and_modifiers() {
        // Independent US keycode map: every displayed symbol must send the
        // actual character, including Shift for quotes, braces and pipes.
        let map = [
            (2, "1", "!"),
            (3, "2", "@"),
            (4, "3", "#"),
            (5, "4", "$"),
            (6, "5", "%"),
            (7, "6", "^"),
            (8, "7", "&"),
            (9, "8", "*"),
            (10, "9", "("),
            (11, "0", ")"),
            (12, "-", "_"),
            (13, "=", "+"),
            (26, "[", "{"),
            (27, "]", "}"),
            (39, ";", ":"),
            (40, "'", "\""),
            (41, "`", "~"),
            (43, "\\", "|"),
            (51, ",", "<"),
            (52, ".", ">"),
            (53, "/", "?"),
        ];
        let mut reachable = vec![Layer::Letters];
        let mut index = 0;
        while index < reachable.len() {
            for key in rows(false, reachable[index])
                .iter()
                .flat_map(|row| &row.keys)
            {
                if let Action::SetLayer(layer) = key.action
                    && !reachable.contains(&layer)
                {
                    reachable.push(layer);
                }
            }
            index += 1;
        }
        let mut characters = String::new();
        for layer in [Layer::Symbols, Layer::MoreSymbols] {
            assert!(reachable.contains(&layer));
            for key in rows(false, layer).iter().flat_map(|row| &row.keys) {
                if !matches!(key.tone, Tone::Character) {
                    continue;
                }
                let Action::Key(code, modifiers) = key.action else {
                    panic!("symbol must send a key stroke");
                };
                let (_, plain, shifted) = map
                    .iter()
                    .find(|(candidate, _, _)| *candidate == code)
                    .unwrap();
                let expected = match modifiers {
                    0 => plain,
                    input_panel::SHIFT => shifted,
                    _ => panic!("unexpected symbol modifiers"),
                };
                assert_eq!(key.plain, *expected);
                assert_eq!(key.shifted, *expected);
                characters.push_str(key.plain);
            }
        }
        for punctuation in (b'!'..=b'~').filter(u8::is_ascii_punctuation) {
            assert!(
                characters.contains(punctuation as char),
                "missing {}",
                punctuation as char
            );
        }
    }

    #[test]
    fn all_layers_keep_hitboxes_above_bottom_navigation_and_keys_usable() {
        for (width, screen_height) in [
            (640.0, 480.0),
            (800.0, 1280.0),
            (1280.0, 800.0),
            (1280.0, 600.0),
        ] {
            let height = panel_height(width, screen_height);
            assert!(height <= screen_height * 0.62);
            for numeric in [false, true] {
                for layer in [
                    Layer::Letters,
                    Layer::Symbols,
                    Layer::MoreSymbols,
                    Layer::Tools,
                    Layer::Function,
                ] {
                    let rows = rows(numeric, layer);
                    let tablet = rows.iter().any(|row| row.geometry == RowGeometry::Tablet);
                    let padding = if tablet {
                        width / GRID_UNITS * EDGE_UNITS
                    } else {
                        10.0
                    };
                    let gap = if tablet {
                        width / GRID_UNITS * GAP_UNITS
                    } else {
                        7.0
                    };
                    let geometry = VerticalGeometry::new(height, padding, gap, &rows);
                    assert!(geometry.row_unit >= 38.0);
                    let mut y = padding;
                    for row in &rows {
                        let key_height = geometry.row_unit * row.height;
                        // The secondary toolbar is 0.73 of a normal key row.
                        assert!(key_height >= 27.0);
                        assert!(y + key_height <= height - 16.0);
                        y += key_height + gap;
                    }
                    assert!((y - gap + geometry.bottom_padding - height).abs() < 0.001);
                    // On an unconstrained screen, the extra allocation exactly
                    // offsets the safe area, preserving the former key heights.
                    if width * 0.35 + BOTTOM_SAFE_AREA <= screen_height * 0.62 {
                        let old_available =
                            width * 0.35 - padding * 2.0 - gap * (rows.len() - 1) as f32;
                        let old_unit =
                            old_available / rows.iter().map(|row| row.height).sum::<f32>();
                        assert!((geometry.row_unit - old_unit).abs() < 0.001);
                    }
                }
            }
        }
    }

    #[test]
    fn default_is_four_rows_with_terminal_controls_only_in_tools_layer() {
        let letters = rows(false, Layer::Letters);
        assert_eq!(letters.len(), 4);
        for (row, expected) in letters.iter().zip(["qwertyuiop", "asdfghjkl", "zxcvbnm"]) {
            let actual: String = row
                .keys
                .iter()
                .filter(|key| key.plain.len() == 1 && key.plain.as_bytes()[0].is_ascii_lowercase())
                .map(|key| key.plain)
                .collect();
            assert_eq!(actual, expected);
        }
        for numeric in [false, true] {
            for layer in [Layer::Letters, Layer::Symbols, Layer::MoreSymbols] {
                let layout = rows(numeric, layer);
                let keys: Vec<_> = layout.iter().flat_map(|row| &row.keys).collect();
                assert_eq!(layout.len(), 4);
                assert!(
                    keys.iter()
                        .any(|key| key.action == Action::SetLayer(Layer::Tools))
                );
                assert!(!keys.iter().any(|key| matches!(
                    key.action,
                    Action::Modifier(input_panel::CTRL | input_panel::ALT)
                        | Action::Key(1 | 15 | 102..=109, _)
                )));
            }
        }
    }

    #[test]
    fn every_tablet_row_fits_the_shared_grid() {
        for numeric in [false, true] {
            for layer in [
                Layer::Letters,
                Layer::Symbols,
                Layer::MoreSymbols,
                Layer::Tools,
                Layer::Function,
            ] {
                for row in rows(numeric, layer) {
                    assert!(row.height > 0.0);
                    assert!(row.keys.iter().all(|key| key.weight > 0.0));
                    if row.geometry == RowGeometry::Tablet {
                        let width = EDGE_UNITS * 2.0
                            + row.inset
                            + row.keys.iter().map(|key| key.weight).sum::<f32>()
                            + GAP_UNITS * (row.keys.len() - 1) as f32;
                        assert!((width - GRID_UNITS).abs() < 0.0001, "row width {width}");
                    }
                }
            }
        }
    }

    #[test]
    fn tools_layer_keeps_shortcuts_and_a_clear_exit_available() {
        let layout = rows(false, Layer::Tools);
        let keys: Vec<_> = layout.iter().flat_map(|row| &row.keys).collect();
        for action in [
            Action::Modifier(input_panel::CTRL),
            Action::Modifier(input_panel::ALT),
            Action::Modifier(input_panel::SHIFT),
            Action::SetLayer(Layer::Letters),
            Action::Hide,
        ] {
            assert!(keys.iter().any(|key| key.action == action));
        }
        for code in [
            1, 15, 102, 107, 104, 109, 105, 103, 108, 106, 46, 85, 14, 28, 57,
        ] {
            assert!(
                keys.iter().any(|key| key.action == Action::Key(code, 0)),
                "missing {code}"
            );
        }
        let c = keys.iter().find(|key| key.plain == "c").unwrap();
        assert_eq!(c.shifted, "C");
        assert!(matches!(c.tone, Tone::Character));
    }
}
