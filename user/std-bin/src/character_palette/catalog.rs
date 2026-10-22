//! A deliberately small, searchable collection plus explicit Unicode lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub text: String,
    pub name: String,
    pub category: usize,
}
pub const CATEGORIES: [&str; 6] = [
    "All categories",
    "Punctuation",
    "Mathematics",
    "Currency",
    "Arrows",
    "Symbols & emoji",
];
pub fn codepoints(text: &str) -> String {
    text.chars()
        .map(|ch| format!("U+{:04X}", ch as u32))
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn entries() -> Vec<Entry> {
    let groups: &[(usize, &[(&str, &str)])] = &[
        (
            1,
            &[
                ("—", "em dash"),
                ("–", "en dash"),
                ("…", "ellipsis"),
                ("•", "bullet"),
                ("·", "middle dot"),
                ("‘", "left single quote"),
                ("’", "right single quote apostrophe"),
                ("“", "left double quote"),
                ("”", "right double quote"),
                ("«", "left guillemet"),
                ("»", "right guillemet"),
                ("¿", "inverted question mark"),
                ("¡", "inverted exclamation mark"),
                ("§", "section"),
                ("¶", "pilcrow paragraph"),
                ("†", "dagger"),
                ("‡", "double dagger"),
                ("「", "left corner bracket"),
                ("」", "right corner bracket"),
                ("『", "left white corner bracket"),
                ("』", "right white corner bracket"),
                ("、", "ideographic comma"),
                ("。", "ideographic full stop"),
            ],
        ),
        (
            2,
            &[
                ("±", "plus minus"),
                ("−", "minus"),
                ("×", "multiplication times"),
                ("÷", "division"),
                ("≠", "not equal"),
                ("≈", "approximately equal"),
                ("≤", "less than or equal"),
                ("≥", "greater than or equal"),
                ("∞", "infinity"),
                ("√", "square root"),
                ("∑", "summation"),
                ("∏", "product"),
                ("∫", "integral"),
                ("∂", "partial differential"),
                ("∆", "increment delta"),
                ("∈", "element of"),
                ("∉", "not an element of"),
                ("∅", "empty set"),
                ("∩", "intersection"),
                ("∪", "union"),
                ("π", "pi"),
                ("α", "alpha"),
                ("β", "beta"),
                ("γ", "gamma"),
                ("θ", "theta"),
                ("λ", "lambda"),
                ("μ", "mu micro"),
                ("σ", "sigma"),
                ("Ω", "omega ohm"),
                ("°", "degree"),
                ("²", "superscript two squared"),
                ("³", "superscript three cubed"),
                ("½", "one half fraction"),
                ("¼", "one quarter fraction"),
                ("¾", "three quarters fraction"),
            ],
        ),
        (
            3,
            &[
                ("$", "dollar"),
                ("€", "euro"),
                ("£", "pound sterling"),
                ("¥", "yen yuan"),
                ("¢", "cent"),
                ("₩", "won"),
                ("₹", "Indian rupee"),
                ("₽", "ruble"),
                ("₿", "bitcoin"),
                ("₺", "Turkish lira"),
                ("₫", "dong"),
                ("฿", "Thai baht"),
            ],
        ),
        (
            4,
            &[
                ("←", "left arrow"),
                ("↑", "up arrow"),
                ("→", "right arrow"),
                ("↓", "down arrow"),
                ("↔", "left right arrow"),
                ("↕", "up down arrow"),
                ("↖", "north west arrow"),
                ("↗", "north east arrow"),
                ("↘", "south east arrow"),
                ("↙", "south west arrow"),
                ("⇒", "right double arrow implies"),
                ("⇔", "left right double arrow equivalent"),
                ("↩", "return arrow"),
                ("↪", "right hooked arrow"),
                ("⟶", "long right arrow"),
            ],
        ),
        (
            5,
            &[
                ("©", "copyright"),
                ("®", "registered"),
                ("™", "trademark"),
                ("✓", "check mark"),
                ("✗", "cross mark"),
                ("★", "black star"),
                ("☆", "white star"),
                ("♥", "heart"),
                ("♦", "diamond"),
                ("♣", "club"),
                ("♠", "spade"),
                ("☀", "sun"),
                ("☁", "cloud"),
                ("☂", "umbrella"),
                ("☃", "snowman"),
                ("☕", "coffee hot beverage"),
                ("⚠", "warning"),
                ("⚙", "gear settings"),
                ("♻", "recycling"),
                ("♪", "music note"),
                ("♫", "music notes"),
                ("😀", "grinning face"),
                ("🙂", "smiling face"),
                ("👍", "thumbs up"),
                ("🎉", "party popper celebration"),
                ("🚀", "rocket"),
                ("💡", "light bulb idea"),
                ("📌", "pushpin"),
                ("📎", "paperclip"),
                ("🔍", "magnifying glass search"),
                ("👩‍💻", "woman technologist"),
                ("👨‍💻", "man technologist"),
                ("❤️", "red heart emoji"),
            ],
        ),
    ];
    groups
        .iter()
        .flat_map(|(category, entries)| {
            entries.iter().map(move |(text, name)| Entry {
                text: (*text).into(),
                name: (*name).into(),
                category: *category,
            })
        })
        .collect()
}
pub fn search(query: &str, category: usize) -> Vec<Entry> {
    let query = query.trim().to_lowercase();
    if let Some(hex) = query.strip_prefix("u+") {
        if let Ok(value) = u32::from_str_radix(hex, 16) {
            if let Some(ch) = char::from_u32(value).filter(|ch| !ch.is_control()) {
                let text = ch.to_string();
                if let Some(entry) = entries().into_iter().find(|entry| entry.text == text) {
                    return vec![entry];
                }
                return vec![Entry {
                    name: format!("Unicode character {}", codepoints(&text)),
                    text,
                    category: 0,
                }];
            }
        }
        return Vec::new();
    }
    entries()
        .into_iter()
        .filter(|entry| {
            (category == 0 || category == entry.category)
                && (query.is_empty() || entry.text.to_lowercase() == query || {
                    let haystack =
                        format!("{} {}", entry.name, codepoints(&entry.text)).to_lowercase();
                    query.split_whitespace().all(|word| haystack.contains(word))
                })
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalogue_is_unique_and_categories_are_valid() {
        let entries = entries();
        let mut seen = std::collections::BTreeSet::new();
        for entry in entries {
            assert!(
                !search(&entry.text, 0).is_empty(),
                "symbol lookup: {}",
                entry.text
            );
            assert!(seen.insert(entry.text));
            assert!((1..CATEGORIES.len()).contains(&entry.category));
            assert!(!entry.name.is_empty());
        }
    }
    #[test]
    fn lookup_names_sequences_and_scalar_boundaries() {
        assert_eq!(search("woman technologist", 0)[0].text, "👩‍💻");
        assert_eq!(search("U+1F600", 0)[0].text, "😀");
        assert_eq!(search("U+3042", 0)[0].text, "あ");
        assert_eq!(codepoints("👩‍💻"), "U+1F469 U+200D U+1F4BB");
        for bad in ["U+D800", "U+110000", "U+000A", "U+", "U+nope"] {
            assert!(search(bad, 0).is_empty());
        }
        assert!(search("arrow", 3).is_empty());
        assert!(!search("arrow right", 4).is_empty());
    }
}
