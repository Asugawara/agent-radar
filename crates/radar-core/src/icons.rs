//! Lucide icons (ISC, see `static/icons/LICENSE`) bundled as SVG text, plus
//! the small display helpers (star counts, relative dates) that the static
//! HTML and the WASM table must render identically.

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        /// Raw SVG of a bundled icon.
        pub fn raw(name: &str) -> Option<&'static str> {
            match name {
                $($name => Some(include_str!(concat!("../../../static/icons/", $name, ".svg"))),)*
                _ => None,
            }
        }
        /// Names of all bundled icons.
        pub const NAMES: &[&str] = &[$($name),*];
    };
}

icons!(
    "activity",
    "arrow-down",
    "arrow-up",
    "arrow-up-down",
    "box",
    "boxes",
    "brain",
    "chevron-down",
    "code",
    "external-link",
    "git-branch",
    "languages",
    "layers",
    "moon",
    "mouse-pointer-click",
    "plug",
    "puzzle",
    "route",
    "search",
    "sliders-horizontal",
    "star",
    "sun",
    "x",
);

/// Whether `name` is a bundled icon.
pub fn has(name: &str) -> bool {
    raw(name).is_some()
}

/// Inline-ready SVG (`class="icon"`, `aria-hidden`, one line); empty for an unknown name.
pub fn svg(name: &str) -> String {
    let Some(raw) = raw(name) else {
        return String::new();
    };
    let one_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    one_line
        .replacen("<svg", "<svg class=\"icon\" aria-hidden=\"true\"", 1)
        .replace("\" >", "\">")
        .replace(" width=\"24\" height=\"24\"", "")
}

/// `154k`, `1.2k`, `3.4M`; below 1000 as is.
pub fn short_count(n: u64) -> String {
    fn one(n: u64, unit: u64, suffix: &str) -> String {
        let tenths = n * 10 / unit;
        if tenths >= 100 || tenths.is_multiple_of(10) {
            format!("{}{suffix}", tenths / 10)
        } else {
            format!("{}.{}{suffix}", tenths / 10, tenths % 10)
        }
    }
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        one(n, 1_000, "k")
    } else {
        one(n, 1_000_000, "M")
    }
}

/// Seconds since the Unix epoch of an RFC 3339 / `YYYY-MM-DD` time (UTC; offsets ignored).
fn epoch_seconds(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num =
        |from: usize, len: usize| -> Option<i64> { s.get(from..from + len)?.parse::<i64>().ok() };
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, m, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (hh, mm, ss) = if b.len() >= 19 && (b[10] == b'T' || b[10] == b' ') {
        (num(11, 2)?, num(14, 2)?, num(17, 2)?)
    } else {
        (0, 0, 0)
    };
    // days from civil (Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3_600 + mm * 60 + ss)
}

/// Relative time of `then` seen from `now` (both RFC 3339): `3d ago` / `3日前`.
/// `None` when either cannot be parsed.
pub fn relative_time(then: &str, now: &str, lang: &str) -> Option<String> {
    let diff = (epoch_seconds(now)? - epoch_seconds(then)?).max(0);
    let ja = lang == "ja";
    let (n, en, jp) = if diff < 60 {
        return Some(if ja { "たった今" } else { "just now" }.into());
    } else if diff < 3_600 {
        (diff / 60, "m ago", "分前")
    } else if diff < 86_400 {
        (diff / 3_600, "h ago", "時間前")
    } else if diff < 30 * 86_400 {
        (diff / 86_400, "d ago", "日前")
    } else if diff < 365 * 86_400 {
        (diff / (30 * 86_400), "mo ago", "か月前")
    } else {
        (diff / (365 * 86_400), "y ago", "年前")
    };
    Some(format!("{n}{}", if ja { jp } else { en }))
}

/// First letter or digit of a name, uppercased (`?` when there is none).
pub fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into())
}

/// Hue (0-359) of the placeholder circle, fixed by the tool id (FNV-1a).
pub fn hue(id: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in id.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    h % 360
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Inside of `<span class="fav">`: the favicon image (falling back to the
/// initial in a circle when it fails to load), or just the circle.
/// The static HTML and the WASM table both use this.
pub fn favicon_inner(src: Option<&str>, name: &str, id: &str) -> String {
    let circle = |hidden: &str| {
        format!(
            "<span class=\"fav-i\" style=\"--h:{}\"{hidden}>{}</span>",
            hue(id),
            escape(&initial(name))
        )
    };
    match src {
        Some(src) => format!(
            "<img src=\"{}\" width=\"20\" height=\"20\" loading=\"lazy\" alt=\"\" \
             onerror=\"this.hidden=true;this.nextElementSibling.hidden=false\">{}",
            escape(src),
            circle(" hidden")
        ),
        None => circle(""),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn favicon_markup() {
        let with = super::favicon_inner(Some("/favicons/a.png"), "alpha", "alpha");
        assert!(with.starts_with(
            "<img src=\"/favicons/a.png\" width=\"20\" height=\"20\" loading=\"lazy\" alt=\"\""
        ));
        assert!(with.contains("fav-i") && with.ends_with("hidden>A</span>"));
        let without = super::favicon_inner(None, "<b>", "x");
        assert!(!without.contains("<img") && !without.contains("<b>"));
        assert_eq!(super::initial("  ~claude"), "C");
        assert_eq!(super::initial("~~"), "?");
        assert_eq!(super::hue("a"), super::hue("a"));
        assert!(super::hue("zzz") < 360);
    }

    use super::*;

    #[test]
    fn short_counts() {
        assert_eq!(short_count(0), "0");
        assert_eq!(short_count(999), "999");
        assert_eq!(short_count(1_000), "1k");
        assert_eq!(short_count(1_234), "1.2k");
        assert_eq!(short_count(154_321), "154k");
        assert_eq!(short_count(1_500_000), "1.5M");
    }

    #[test]
    fn relative_times() {
        let now = "2026-11-02T12:00:00Z";
        let r = |t: &str, l: &str| relative_time(t, now, l).unwrap();
        assert_eq!(r("2026-10-30T12:00:00Z", "en"), "3d ago");
        assert_eq!(r("2026-10-30T12:00:00Z", "ja"), "3日前");
        assert_eq!(r("2026-09-02T12:00:00Z", "en"), "2mo ago");
        assert_eq!(r("2026-09-02T12:00:00Z", "ja"), "2か月前");
        assert_eq!(r("2026-11-02T09:00:00Z", "en"), "3h ago");
        assert_eq!(r("2024-11-02T12:00:00Z", "ja"), "2年前");
        assert_eq!(r("2026-11-02T12:00:00Z", "en"), "just now");
        assert!(relative_time("garbage", now, "en").is_none());
    }

    #[test]
    fn svg_is_inline_and_unknown_is_empty() {
        let s = svg("star");
        assert!(s.starts_with("<svg class=\"icon\" aria-hidden=\"true\""));
        assert!(!s.contains('\n'));
        assert!(s.contains("stroke=\"currentColor\""));
        assert_eq!(svg("nope"), "");
        assert!(NAMES.iter().all(|n| has(n)));
    }
}
