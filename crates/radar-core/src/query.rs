//! Filtering, searching and sorting of the tool table, plus the URL query
//! (`?tag=cli&q=agent&sort=-stars`) it is synchronised with. Pure logic, no
//! I/O, so the browser (WASM) and the tests use the same code.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{Platform, Pricing};
use crate::site::{SiteData, SiteTool};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Stars,
    /// `pushed_at`
    Updated,
    Added,
}

impl SortKey {
    pub fn as_str(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Stars => "stars",
            SortKey::Updated => "updated",
            SortKey::Added => "added",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "name" => SortKey::Name,
            "stars" => SortKey::Stars,
            "updated" => SortKey::Updated,
            "added" => SortKey::Added,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub desc: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Sort {
            key: SortKey::Name,
            desc: false,
        }
    }
}

/// Whether the visitor has opened a tool. The marks are kept in the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReadState {
    Unread,
    Read,
}

impl ReadState {
    pub fn as_str(self) -> &'static str {
        match self {
            ReadState::Unread => "unread",
            ReadState::Read => "read",
        }
    }
}

fn parse_read(s: &str) -> Option<ReadState> {
    [ReadState::Unread, ReadState::Read]
        .into_iter()
        .find(|r| r.as_str() == s)
}

fn state_of(read_ids: &BTreeSet<String>, tool: &SiteTool) -> ReadState {
    if read_ids.contains(&tool.id) {
        ReadState::Read
    } else {
        ReadState::Unread
    }
}

/// What the visitor selected. Inside one group (a tag facet, pricing,
/// platforms) values are OR-ed; the groups are AND-ed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub tags: BTreeSet<String>,
    /// Tags whose tools are hidden (`tag=-id`). Never overlaps `tags`.
    pub excluded: BTreeSet<String>,
    pub q: String,
    pub pricing: BTreeSet<Pricing>,
    pub platforms: BTreeSet<Platform>,
    pub read: BTreeSet<ReadState>,
    pub sort: Sort,
    /// 0-based page number.
    pub page: usize,
    pub per_page: PageSize,
}

/// Tools shown per page; the visitor picks one under the table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum PageSize {
    #[default]
    S25,
    S50,
    S100,
}

impl PageSize {
    pub const ALL: [PageSize; 3] = [PageSize::S25, PageSize::S50, PageSize::S100];

    pub fn get(self) -> usize {
        match self {
            PageSize::S25 => 25,
            PageSize::S50 => 50,
            PageSize::S100 => 100,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.get().to_string() == s)
    }
}

/// How a tag takes part in the filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagMode {
    Off,
    Include,
    Exclude,
}

/// A group of conditions; used to leave one group out when counting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Group<'a> {
    Facet(&'a str),
    Pricing,
    Platforms,
    Read,
}

/// Number of tools each choice would show, counted with every condition
/// except the one of its own group applied.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    pub tags: BTreeMap<String, usize>,
    pub pricing: BTreeMap<Pricing, usize>,
    pub platforms: BTreeMap<Platform, usize>,
    pub read: BTreeMap<ReadState, usize>,
}

fn facet_of<'a>(data: &'a SiteData, tag: &str) -> Option<&'a str> {
    data.facets
        .iter()
        .find(|f| f.tags.iter().any(|t| t.id == tag))
        .map(|f| f.id.as_str())
}

fn tag_label<'a>(data: &'a SiteData, tag: &str, lang: &str) -> &'a str {
    data.facets
        .iter()
        .flat_map(|f| f.tags.iter())
        .find(|t| t.id == tag)
        .map(|t| t.label.get(lang))
        .unwrap_or("")
}

impl Query {
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
            && self.excluded.is_empty()
            && self.q.trim().is_empty()
            && self.pricing.is_empty()
            && self.platforms.is_empty()
            && self.read.is_empty()
    }

    /// Number of active filter conditions (the search text is not counted).
    pub fn filter_count(&self) -> usize {
        self.tags.len()
            + self.excluded.len()
            + self.pricing.len()
            + self.platforms.len()
            + self.read.len()
    }

    /// Drop every condition (the sort order and the page size stay).
    pub fn cleared(&self) -> Query {
        Query {
            sort: self.sort,
            per_page: self.per_page,
            ..Query::default()
        }
    }

    pub fn tag_mode(&self, id: &str) -> TagMode {
        if self.tags.contains(id) {
            TagMode::Include
        } else if self.excluded.contains(id) {
            TagMode::Exclude
        } else {
            TagMode::Off
        }
    }

    /// Put `id` into `mode`; include and exclude are mutually exclusive.
    pub fn set_tag(&mut self, id: &str, mode: TagMode) {
        self.tags.remove(id);
        self.excluded.remove(id);
        match mode {
            TagMode::Off => {}
            TagMode::Include => {
                self.tags.insert(id.to_string());
            }
            TagMode::Exclude => {
                self.excluded.insert(id.to_string());
            }
        }
    }

    fn matches_text(&self, data: &SiteData, tool: &SiteTool, lang: &str) -> bool {
        let words: Vec<String> = self
            .q
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .collect();
        if words.is_empty() {
            return true;
        }
        let mut hay = format!("{}\n{}", tool.name, tool.summary.get(lang));
        for t in &tool.tags {
            hay.push('\n');
            hay.push_str(tag_label(data, t, lang));
        }
        let hay = hay.to_lowercase();
        words.iter().all(|w| hay.contains(w.as_str()))
    }

    /// Does `tool` satisfy every condition, ignoring the `skip` group?
    /// `read_ids` are the tools the visitor has opened.
    pub fn matches(
        &self,
        data: &SiteData,
        tool: &SiteTool,
        lang: &str,
        read_ids: &BTreeSet<String>,
        skip: Option<&Group>,
    ) -> bool {
        let skipped = |g: Group| skip == Some(&g);
        if !self.matches_text(data, tool, lang) {
            return false;
        }
        if !skipped(Group::Pricing)
            && !self.pricing.is_empty()
            && !self.pricing.contains(&tool.pricing)
        {
            return false;
        }
        if !skipped(Group::Platforms)
            && !self.platforms.is_empty()
            && !tool.platforms.iter().any(|p| self.platforms.contains(p))
        {
            return false;
        }
        if !skipped(Group::Read)
            && !self.read.is_empty()
            && !self.read.contains(&state_of(read_ids, tool))
        {
            return false;
        }
        for facet in &data.facets {
            if skipped(Group::Facet(&facet.id)) {
                continue;
            }
            let selected: Vec<&String> = facet
                .tags
                .iter()
                .map(|t| &t.id)
                .filter(|id| self.tags.contains(*id))
                .collect();
            if !selected.is_empty() && !selected.iter().any(|id| tool.tags.contains(*id)) {
                return false;
            }
            if facet
                .tags
                .iter()
                .any(|t| self.excluded.contains(&t.id) && tool.tags.contains(&t.id))
            {
                return false;
            }
        }
        true
    }

    /// Matching tools in the requested order.
    pub fn apply<'a>(
        &self,
        data: &'a SiteData,
        lang: &str,
        read_ids: &BTreeSet<String>,
    ) -> Vec<&'a SiteTool> {
        let mut out: Vec<&SiteTool> = data
            .tools
            .iter()
            .filter(|t| self.matches(data, t, lang, read_ids, None))
            .collect();
        sort_tools(&mut out, self.sort);
        out
    }

    pub fn counts(&self, data: &SiteData, lang: &str, read_ids: &BTreeSet<String>) -> Counts {
        let mut c = Counts::default();
        for facet in &data.facets {
            let g = Group::Facet(&facet.id);
            for t in &facet.tags {
                let n = data
                    .tools
                    .iter()
                    .filter(|tool| {
                        tool.tags.contains(&t.id)
                            && self.matches(data, tool, lang, read_ids, Some(&g))
                    })
                    .count();
                c.tags.insert(t.id.clone(), n);
            }
        }
        for p in [Pricing::Free, Pricing::Freemium, Pricing::Paid] {
            let n = data
                .tools
                .iter()
                .filter(|t| {
                    t.pricing == p && self.matches(data, t, lang, read_ids, Some(&Group::Pricing))
                })
                .count();
            c.pricing.insert(p, n);
        }
        for p in ALL_PLATFORMS {
            let n = data
                .tools
                .iter()
                .filter(|t| {
                    t.platforms.contains(&p)
                        && self.matches(data, t, lang, read_ids, Some(&Group::Platforms))
                })
                .count();
            c.platforms.insert(p, n);
        }
        for st in [ReadState::Unread, ReadState::Read] {
            let n = data
                .tools
                .iter()
                .filter(|t| {
                    state_of(read_ids, t) == st
                        && self.matches(data, t, lang, read_ids, Some(&Group::Read))
                })
                .count();
            c.read.insert(st, n);
        }
        c
    }

    /// Parse `a=b&c=d` (a leading `?` is fine). Unknown keys and values are
    /// ignored; tags are checked against `data`.
    pub fn parse(query: &str, data: &SiteData) -> Query {
        let mut out = Query::default();
        for pair in query.trim_start_matches('?').split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let (k, v) = (decode(k), decode(v));
            match k.as_str() {
                "tag" => match v.strip_prefix('-') {
                    Some(id) => {
                        if facet_of(data, id).is_some() {
                            out.set_tag(id, TagMode::Exclude);
                        }
                    }
                    None => {
                        if facet_of(data, &v).is_some() {
                            out.set_tag(&v, TagMode::Include);
                        }
                    }
                },
                "q" => out.q = v,
                "pricing" => {
                    if let Some(p) = parse_pricing(&v) {
                        out.pricing.insert(p);
                    }
                }
                "platform" => {
                    if let Some(p) = parse_platform(&v) {
                        out.platforms.insert(p);
                    }
                }
                "read" => {
                    if let Some(r) = parse_read(&v) {
                        out.read.insert(r);
                    }
                }
                "sort" => {
                    let (desc, name) = match v.strip_prefix('-') {
                        Some(rest) => (true, rest),
                        None => (false, v.as_str()),
                    };
                    if let Some(key) = SortKey::parse(name) {
                        out.sort = Sort { key, desc };
                    }
                }
                "page" => {
                    if let Ok(n) = v.parse::<usize>()
                        && n >= 2
                    {
                        out.page = n - 1;
                    }
                }
                "per_page" => {
                    if let Some(p) = PageSize::parse(&v) {
                        out.per_page = p;
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Inverse of [`Query::parse`], without the leading `?`. Empty when
    /// nothing differs from the default.
    pub fn to_query_string(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for t in &self.tags {
            parts.push(format!("tag={}", encode(t)));
        }
        for t in &self.excluded {
            parts.push(format!("tag=-{}", encode(t)));
        }
        if !self.q.trim().is_empty() {
            parts.push(format!("q={}", encode(self.q.trim())));
        }
        for p in &self.pricing {
            parts.push(format!("pricing={}", p.as_str()));
        }
        for p in &self.platforms {
            parts.push(format!("platform={}", p.as_str()));
        }
        for r in &self.read {
            parts.push(format!("read={}", r.as_str()));
        }
        if self.sort != Sort::default() {
            let dash = if self.sort.desc { "-" } else { "" };
            parts.push(format!("sort={dash}{}", self.sort.key.as_str()));
        }
        if self.per_page != PageSize::default() {
            parts.push(format!("per_page={}", self.per_page.get()));
        }
        if self.page > 0 {
            parts.push(format!("page={}", self.page + 1));
        }
        parts.join("&")
    }

    /// Set the page, but go back to the first page if any other part of the
    /// query changed compared with `before`.
    pub fn reset_page_if_changed(&mut self, before: &Query) {
        let mut now = self.clone();
        now.page = 0;
        let mut was = before.clone();
        was.page = 0;
        if now != was {
            self.page = 0;
        }
    }
}

/// Page numbers for the pager, 0-based; `None` is a gap (`…`). Shows the
/// first and last page and the neighbours of `current`.
pub fn page_items(current: usize, count: usize) -> Vec<Option<usize>> {
    if count <= 7 {
        return (0..count).map(Some).collect();
    }
    let mut shown = vec![0, count - 1];
    for p in [
        current.checked_sub(1),
        Some(current),
        current.checked_add(1),
    ]
    .into_iter()
    .flatten()
    {
        if p < count {
            shown.push(p);
        }
    }
    shown.sort_unstable();
    shown.dedup();
    let mut out = Vec::new();
    let mut prev: Option<usize> = None;
    for p in shown {
        if prev.is_some_and(|q| p - q >= 2) {
            out.push(None);
        }
        out.push(Some(p));
        prev = Some(p);
    }
    out
}

/// Keys [`Query::parse`] reads. Other keys in the address bar (`utm_*`,
/// `gclid`, …) belong to someone else and are kept.
const OWN_KEYS: [&str; 8] = [
    "tag", "q", "pricing", "platform", "read", "sort", "per_page", "page",
];

/// The search part for the address bar, without the leading `?`: `own`
/// (from [`Query::to_query_string`]) followed by every pair of `current`
/// (`?` optional) whose key is not one of ours, unchanged and in order.
pub fn merge_query_string(own: &str, current: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !own.is_empty() {
        parts.push(own);
    }
    for pair in current.trim_start_matches('?').split('&') {
        if pair.is_empty() {
            continue;
        }
        let key = decode(pair.split_once('=').map_or(pair, |(k, _)| k));
        if !OWN_KEYS.contains(&key.as_str()) {
            parts.push(pair);
        }
    }
    parts.join("&")
}

pub const ALL_PLATFORMS: [Platform; 7] = [
    Platform::Macos,
    Platform::Windows,
    Platform::Linux,
    Platform::Web,
    Platform::Ios,
    Platform::Android,
    Platform::Unspecified,
];

fn parse_pricing(s: &str) -> Option<Pricing> {
    [Pricing::Free, Pricing::Freemium, Pricing::Paid]
        .into_iter()
        .find(|p| p.as_str() == s)
}

fn parse_platform(s: &str) -> Option<Platform> {
    ALL_PLATFORMS.into_iter().find(|p| p.as_str() == s)
}

/// Sort in place. Tools without a value for `stars` / `updated` always come
/// last, whichever the direction. Ties fall back to the name (A–Z).
pub fn sort_tools(tools: &mut [&SiteTool], sort: Sort) {
    use std::cmp::Ordering;
    tools.sort_by(|a, b| {
        let by_name = || a.name.to_lowercase().cmp(&b.name.to_lowercase());
        let ord = match sort.key {
            SortKey::Name => {
                let o = by_name();
                return if sort.desc { o.reverse() } else { o };
            }
            SortKey::Added => directed(a.added.cmp(&b.added), sort.desc),
            SortKey::Stars => match (a.stars, b.stars) {
                (Some(x), Some(y)) => directed(x.cmp(&y), sort.desc),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
            SortKey::Updated => match (&a.pushed_at, &b.pushed_at) {
                (Some(x), Some(y)) => directed(x.cmp(y), sort.desc),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
        };
        ord.then_with(by_name)
    });
}

fn directed(o: std::cmp::Ordering, desc: bool) -> std::cmp::Ordering {
    if desc { o.reverse() } else { o }
}

/// Percent-decode (`+` is a space). Invalid escapes are kept as they are.
pub fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() && hex(b[i + 1]).is_some() && hex(b[i + 2]).is_some() => {
                out.push(hex(b[i + 1]).unwrap() * 16 + hex(b[i + 2]).unwrap());
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// Percent-encode everything except unreserved characters.
pub fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Lang;
    use crate::site::{SiteFacet, SiteTag};

    fn l(en: &str, ja: &str) -> Lang {
        Lang {
            en: en.into(),
            ja: ja.into(),
        }
    }

    fn tag(id: &str, en: &str, ja: &str) -> SiteTag {
        SiteTag {
            id: id.into(),
            label: l(en, ja),
            short: l(en, ja),
            icon: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tool(
        id: &str,
        pricing: Pricing,
        oss: bool,
        platforms: &[Platform],
        tags: &[&str],
        added: &str,
        stars: Option<u64>,
        pushed: Option<&str>,
    ) -> SiteTool {
        SiteTool {
            id: id.into(),
            name: id.to_string(),
            url: format!("https://example.com/{id}"),
            repo: None,
            summary: l(&format!("{id} summary"), &format!("{id} の概要")),
            pricing,
            open_source: oss,
            platforms: platforms.to_vec(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            added: added.into(),
            links: Default::default(),
            stars,
            pushed_at: pushed.map(String::from),
            archived: false,
            favicon: None,
        }
    }

    fn data() -> SiteData {
        use Platform::*;
        use Pricing::*;
        SiteData {
            generated_at: "2026-01-01T00:00:00Z".into(),
            facets: vec![
                SiteFacet {
                    id: "category".into(),
                    label: l("Category", "カテゴリ"),
                    min: 1,
                    max: Some(3),
                    tags: vec![
                        tag("cli", "CLI", "CLI"),
                        tag("mcp-server", "MCP server", "MCP サーバー"),
                        tag("sandbox", "Sandbox", "サンドボックス"),
                    ],
                },
                SiteFacet {
                    id: "works".into(),
                    label: l("Works with", "対応"),
                    min: 0,
                    max: None,
                    tags: vec![tag("works-with-claude", "Claude", "Claude")],
                },
            ],
            tools: vec![
                tool(
                    "alpha",
                    Free,
                    true,
                    &[Macos, Linux],
                    &["cli", "works-with-claude"],
                    "2026-01-03",
                    Some(100),
                    Some("2026-03-01T00:00:00Z"),
                ),
                tool(
                    "Beta",
                    Freemium,
                    false,
                    &[Web],
                    &["mcp-server"],
                    "2026-01-01",
                    Some(5000),
                    Some("2026-02-01T00:00:00Z"),
                ),
                tool(
                    "gamma",
                    Paid,
                    true,
                    &[Windows, Web],
                    &["cli", "sandbox"],
                    "2026-01-02",
                    None,
                    None,
                ),
                tool(
                    "delta",
                    Free,
                    false,
                    &[Linux],
                    &["sandbox", "works-with-claude"],
                    "2026-01-04",
                    Some(100),
                    Some("2026-04-01T00:00:00Z"),
                ),
            ],
        }
    }

    fn ids(v: &[&SiteTool]) -> Vec<String> {
        v.iter().map(|t| t.id.clone()).collect()
    }

    fn q(s: &str) -> Query {
        Query::parse(s, &data())
    }

    #[test]
    fn filter_count_ignores_search_text() {
        let mut q = Query::default();
        assert_eq!(q.filter_count(), 0);
        q.q = "mcp".into();
        q.tags.insert("cli".into());
        q.pricing.insert(Pricing::Free);
        assert_eq!(q.filter_count(), 2);
    }

    #[test]
    fn empty_query_shows_everything_by_name() {
        let d = data();
        assert_eq!(
            ids(&Query::default().apply(&d, "en", &BTreeSet::new())),
            ["alpha", "Beta", "delta", "gamma"]
        );
    }

    #[test]
    fn tags_in_a_facet_are_or() {
        let d = data();
        assert_eq!(
            ids(&q("tag=cli&tag=mcp-server").apply(&d, "en", &BTreeSet::new())),
            ["alpha", "Beta", "gamma"]
        );
    }

    #[test]
    fn facets_are_and() {
        let d = data();
        assert_eq!(
            ids(&q("tag=cli&tag=sandbox&tag=works-with-claude").apply(&d, "en", &BTreeSet::new())),
            ["alpha", "delta"]
        );
    }

    #[test]
    fn pricing_and_platforms_or_inside_and_between() {
        let d = data();
        assert_eq!(
            ids(&q("pricing=free&pricing=paid").apply(&d, "en", &BTreeSet::new())),
            ["alpha", "delta", "gamma"]
        );
        assert_eq!(
            ids(&q("platform=web&platform=linux").apply(&d, "en", &BTreeSet::new())),
            ["alpha", "Beta", "delta", "gamma"]
        );
        assert_eq!(
            ids(&q("pricing=free&platform=linux").apply(&d, "en", &BTreeSet::new())),
            ["alpha", "delta"]
        );
    }

    #[test]
    fn platform_unspecified_filters_and_counts() {
        let mut d = data();
        d.tools.push(tool(
            "omega",
            Pricing::Free,
            false,
            &[Platform::Unspecified],
            &["cli"],
            "2026-01-05",
            None,
            None,
        ));
        assert_eq!(
            ids(&q("platform=unspecified").apply(&d, "en", &BTreeSet::new())),
            ["omega"]
        );
        assert_eq!(q("platform=unspecified").platforms.len(), 1);
        assert_eq!(
            q("platform=unspecified&platform=linux").to_query_string(),
            "platform=linux&platform=unspecified"
        );
        assert_eq!(
            q(&q("platform=unspecified").to_query_string())
                .platforms
                .len(),
            1
        );
    }

    #[test]
    fn text_search_covers_name_summary_and_tag_labels_ignoring_case() {
        let d = data();
        assert_eq!(
            ids(&q("q=BETA").apply(&d, "en", &BTreeSet::new())),
            ["Beta"]
        );
        assert_eq!(
            ids(&q("q=gamma%20summary").apply(&d, "en", &BTreeSet::new())),
            ["gamma"]
        );
        assert_eq!(
            ids(&q("q=mcp+server").apply(&d, "en", &BTreeSet::new())),
            ["Beta"]
        );
        // the tag label of the displayed language is searched
        assert_eq!(
            ids(&q("q=サンドボックス").apply(&d, "ja", &BTreeSet::new())),
            ["delta", "gamma"]
        );
        assert!(
            q("q=サンドボックス")
                .apply(&d, "en", &BTreeSet::new())
                .is_empty()
        );
        // summary of the other language is not searched
        assert!(
            q("q=%E3%81%AE%E6%A6%82%E8%A6%81")
                .apply(&d, "en", &BTreeSet::new())
                .is_empty()
        );
        assert_eq!(
            q("q=%E3%81%AE%E6%A6%82%E8%A6%81")
                .apply(&d, "ja", &BTreeSet::new())
                .len(),
            4
        );
    }

    #[test]
    fn text_search_combines_with_filters() {
        let d = data();
        assert_eq!(
            ids(&q("q=a&tag=sandbox").apply(&d, "en", &BTreeSet::new())),
            ["delta", "gamma"]
        );
        assert_eq!(
            ids(&q("q=alpha&tag=sandbox").apply(&d, "en", &BTreeSet::new())),
            Vec::<String>::new()
        );
    }

    #[test]
    fn counts_ignore_only_their_own_group() {
        let d = data();
        let query = q("tag=cli&pricing=free");
        let c = query.counts(&d, "en", &BTreeSet::new());
        // category count: pricing=free applied, category ignored
        assert_eq!(c.tags["cli"], 1); // alpha
        assert_eq!(c.tags["sandbox"], 1); // delta
        assert_eq!(c.tags["mcp-server"], 0);
        // works facet: category=cli and pricing=free applied
        assert_eq!(c.tags["works-with-claude"], 1); // alpha
        // pricing: category=cli applied, pricing ignored
        assert_eq!(c.pricing[&Pricing::Free], 1);
        assert_eq!(c.pricing[&Pricing::Paid], 1);
        assert_eq!(c.pricing[&Pricing::Freemium], 0);
        assert_eq!(c.platforms[&Platform::Linux], 1);
    }

    #[test]
    fn counts_include_the_search_text() {
        let d = data();
        let c = q("q=delta").counts(&d, "en", &BTreeSet::new());
        assert_eq!(c.tags["sandbox"], 1);
        assert_eq!(c.tags["cli"], 0);
    }

    #[test]
    fn sort_stars_missing_last_both_directions() {
        let d = data();
        let mut query = q("sort=stars");
        assert_eq!(
            ids(&query.apply(&d, "en", &BTreeSet::new())),
            ["alpha", "delta", "Beta", "gamma"]
        );
        query = q("sort=-stars");
        assert_eq!(
            ids(&query.apply(&d, "en", &BTreeSet::new())),
            ["Beta", "alpha", "delta", "gamma"]
        );
    }

    #[test]
    fn sort_updated_and_added() {
        let d = data();
        assert_eq!(
            ids(&q("sort=-updated").apply(&d, "en", &BTreeSet::new())),
            ["delta", "alpha", "Beta", "gamma"]
        );
        assert_eq!(
            ids(&q("sort=updated").apply(&d, "en", &BTreeSet::new())),
            ["Beta", "alpha", "delta", "gamma"]
        );
        assert_eq!(
            ids(&q("sort=added").apply(&d, "en", &BTreeSet::new())),
            ["Beta", "gamma", "alpha", "delta"]
        );
        assert_eq!(
            ids(&q("sort=-added").apply(&d, "en", &BTreeSet::new())),
            ["delta", "alpha", "gamma", "Beta"]
        );
    }

    #[test]
    fn sort_name_desc_is_case_insensitive() {
        let d = data();
        assert_eq!(
            ids(&q("sort=-name").apply(&d, "en", &BTreeSet::new())),
            ["gamma", "delta", "Beta", "alpha"]
        );
    }

    #[test]
    fn parse_ignores_unknown_keys_and_values() {
        let query =
            q("?tag=nope&tag=cli&foo=bar&pricing=gold&platform=amiga&sort=bogus&oss=yes&q=x");
        assert_eq!(query.tags, BTreeSet::from(["cli".to_string()]));
        assert!(query.pricing.is_empty() && query.platforms.is_empty());
        assert_eq!(query.sort, Sort::default());
        assert_eq!(query.q, "x");
    }

    #[test]
    fn parse_the_documented_example() {
        let query = q("?tag=cli&tag=mcp-server&q=agent&pricing=free&platform=macos&sort=-stars");
        assert_eq!(query.tags.len(), 2);
        assert_eq!(query.q, "agent");
        assert!(query.pricing.contains(&Pricing::Free));
        assert!(query.platforms.contains(&Platform::Macos));
        assert_eq!(
            query.sort,
            Sort {
                key: SortKey::Stars,
                desc: true
            }
        );
    }

    #[test]
    fn query_string_round_trips() {
        let query = q("tag=mcp-server&tag=cli&q=a%20b%26c&pricing=free&platform=macos&sort=-stars");
        let s = query.to_query_string();
        assert_eq!(
            s,
            "tag=cli&tag=mcp-server&q=a%20b%26c&pricing=free&platform=macos&sort=-stars"
        );
        assert_eq!(q(&s), query);
        assert_eq!(Query::default().to_query_string(), "");
        // default sort is not written
        assert_eq!(q("sort=name").to_query_string(), "");
        assert_eq!(q("sort=-name").to_query_string(), "sort=-name");
    }

    #[test]
    fn non_ascii_search_round_trips() {
        let query = Query {
            q: "サンド ボックス".into(),
            ..Query::default()
        };
        let s = query.to_query_string();
        assert!(s.is_ascii());
        assert_eq!(q(&s).q, "サンド ボックス");
    }

    #[test]
    fn exclude_hides_tools_with_the_tag() {
        let d = data();
        assert_eq!(
            ids(&q("tag=-sandbox").apply(&d, "en", &BTreeSet::new())),
            ["alpha", "Beta"]
        );
    }

    #[test]
    fn include_and_exclude_combine_inside_a_facet() {
        let d = data();
        // cli tools (alpha, gamma) minus sandbox tools (gamma, delta)
        assert_eq!(
            ids(&q("tag=cli&tag=-sandbox").apply(&d, "en", &BTreeSet::new())),
            ["alpha"]
        );
    }

    #[test]
    fn exclude_is_and_ed_with_other_facets() {
        let d = data();
        assert_eq!(
            ids(&q("tag=works-with-claude&tag=-sandbox").apply(&d, "en", &BTreeSet::new())),
            ["alpha"]
        );
    }

    #[test]
    fn counts_ignore_the_own_groups_exclusion() {
        let d = data();
        let c = q("tag=-sandbox").counts(&d, "en", &BTreeSet::new());
        // exclusion of the category facet is ignored for its own counts
        assert_eq!(c.tags["sandbox"], 2);
        assert_eq!(c.tags["cli"], 2);
        // other facets still see the exclusion: delta is hidden
        assert_eq!(c.tags["works-with-claude"], 1);
    }

    #[test]
    fn exclude_round_trips() {
        let query = q("tag=cli&tag=-sandbox&tag=-works-with-claude");
        assert_eq!(
            query.to_query_string(),
            "tag=cli&tag=-sandbox&tag=-works-with-claude"
        );
        assert_eq!(q(&query.to_query_string()), query);
        assert_eq!(query.filter_count(), 3);
        assert!(!query.is_empty());
        assert!(query.cleared().is_empty());
    }

    #[test]
    fn unknown_excluded_tag_is_ignored() {
        let query = q("tag=-nope&tag=-");
        assert!(query.excluded.is_empty());
        assert!(query.is_empty());
    }

    #[test]
    fn include_and_exclude_of_one_tag_are_exclusive() {
        let mut query = q("tag=cli");
        query.set_tag("cli", TagMode::Exclude);
        assert_eq!(query.tag_mode("cli"), TagMode::Exclude);
        assert!(query.tags.is_empty());
        query.set_tag("cli", TagMode::Include);
        assert_eq!(query.tag_mode("cli"), TagMode::Include);
        assert!(query.excluded.is_empty());
        query.set_tag("cli", TagMode::Off);
        assert_eq!(query.tag_mode("cli"), TagMode::Off);
        // a hand-written URL cannot hold both either: the later one wins
        let both = q("tag=cli&tag=-cli");
        assert!(both.tags.is_empty() && both.excluded.len() == 1);
    }

    #[test]
    fn clear_keeps_sort() {
        let c = q("tag=cli&q=x&sort=-stars").cleared();
        assert!(c.is_empty());
        assert_eq!(c.sort.key, SortKey::Stars);
    }

    #[test]
    fn merge_keeps_foreign_pairs_in_order() {
        assert_eq!(
            merge_query_string("q=claude", "?utm_source=x&q=old&tag=a&gclid=abc"),
            "q=claude&utm_source=x&gclid=abc"
        );
    }

    #[test]
    fn merge_without_own_keys() {
        assert_eq!(
            merge_query_string("", "?utm_source=x&sort=-stars"),
            "utm_source=x"
        );
    }

    #[test]
    fn merge_of_nothing() {
        assert_eq!(merge_query_string("", ""), "");
        assert_eq!(merge_query_string("oss=1", "?"), "oss=1");
    }

    #[test]
    fn merge_treats_encoded_own_keys_as_own() {
        assert_eq!(merge_query_string("", "%71=x&a=b"), "a=b");
    }

    #[test]
    fn decode_handles_plus_and_bad_escapes() {
        assert_eq!(decode("a+b%20c"), "a b c");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("%zz"), "%zz");
        assert_eq!(decode("%41"), "A");
    }

    #[test]
    fn read_state_round_trips() {
        let query = q("read=unread&read=read&read=bogus");
        assert_eq!(
            query.read,
            BTreeSet::from([ReadState::Unread, ReadState::Read])
        );
        assert_eq!(query.to_query_string(), "read=unread&read=read");
        assert_eq!(q(&query.to_query_string()), query);
    }

    #[test]
    fn read_filter_uses_the_opened_set() {
        let d = data();
        let read = BTreeSet::from(["alpha".to_string()]);
        assert_eq!(
            ids(&q("read=unread").apply(&d, "en", &read)),
            ["Beta", "delta", "gamma"]
        );
        assert_eq!(ids(&q("read=read").apply(&d, "en", &read)), ["alpha"]);
    }

    #[test]
    fn read_counts_ignore_only_their_own_group() {
        let d = data();
        let read = BTreeSet::from(["alpha".to_string()]);
        // cli tools are alpha (read) and gamma (unread)
        let c = q("read=unread&tag=cli").counts(&d, "en", &read);
        assert_eq!(c.read[&ReadState::Read], 1);
        assert_eq!(c.read[&ReadState::Unread], 1);
        // with only the read filter, the other side counts the whole table
        let c = q("read=unread").counts(&d, "en", &read);
        assert_eq!(c.read[&ReadState::Read], 1);
        assert_eq!(c.read[&ReadState::Unread], 3);
    }

    #[test]
    fn page_is_read_from_the_url() {
        let query = q("?tag=cli&page=3");
        assert_eq!(query.page, 2);
        assert_eq!(query.to_query_string(), "tag=cli&page=3");
    }

    #[test]
    fn first_page_is_not_written() {
        for s in ["page=0", "page=1", "page=abc", "page=-2"] {
            let query = q(s);
            assert_eq!(query.page, 0, "{s}");
            assert!(!query.to_query_string().contains("page"), "{s}");
        }
    }

    #[test]
    fn page_is_not_a_filter() {
        let mut query = q("page=4");
        assert!(query.is_empty());
        assert_eq!(query.filter_count(), 0);
        query.page = 2;
        assert!(query.cleared().is_empty());
    }

    #[test]
    fn changing_a_filter_goes_back_to_the_first_page() {
        let before = Query {
            page: 2,
            ..Query::default()
        };
        let mut tag = before.clone();
        tag.set_tag("cli", TagMode::Include);
        tag.reset_page_if_changed(&before);
        assert_eq!(tag.page, 0);

        let mut search = before.clone();
        search.q = "x".into();
        search.reset_page_if_changed(&before);
        assert_eq!(search.page, 0);

        let mut sort = before.clone();
        sort.sort = Sort {
            key: SortKey::Stars,
            desc: true,
        };
        sort.reset_page_if_changed(&before);
        assert_eq!(sort.page, 0);
    }

    #[test]
    fn changing_only_the_page_keeps_it() {
        let before = Query {
            page: 2,
            ..Query::default()
        };
        let mut next = before.clone();
        next.page = 3;
        next.reset_page_if_changed(&before);
        assert_eq!(next.page, 3);
    }

    #[test]
    fn page_size_is_read_from_the_url() {
        let query = q("?per_page=50&page=2");
        assert_eq!(query.per_page, PageSize::S50);
        assert_eq!(query.page, 1);
        assert_eq!(query.to_query_string(), "per_page=50&page=2");
    }

    #[test]
    fn default_or_unknown_page_size_is_not_written() {
        for s in ["per_page=25", "per_page=30", "per_page=abc"] {
            let query = q(s);
            assert_eq!(query.per_page, PageSize::S25, "{s}");
            assert!(!query.to_query_string().contains("per_page"), "{s}");
        }
    }

    #[test]
    fn clearing_the_filters_keeps_the_page_size() {
        let mut query = q("per_page=100");
        query.set_tag("cli", TagMode::Include);
        let cleared = query.cleared();
        assert_eq!(cleared.per_page, PageSize::S100);
        assert!(cleared.is_empty());
    }

    #[test]
    fn changing_the_page_size_goes_back_to_the_first_page() {
        let before = Query {
            page: 2,
            ..Query::default()
        };
        let mut next = before.clone();
        next.per_page = PageSize::S50;
        next.reset_page_if_changed(&before);
        assert_eq!(next.page, 0);
    }

    #[test]
    fn page_size_list_is_25_50_100() {
        let sizes: Vec<usize> = PageSize::ALL.into_iter().map(PageSize::get).collect();
        assert_eq!(sizes, [25, 50, 100]);
    }

    #[test]
    fn page_items_shows_the_neighbours_and_the_ends() {
        assert_eq!(page_items(0, 20), [Some(0), Some(1), None, Some(19)]);
        assert_eq!(
            page_items(10, 20),
            [Some(0), None, Some(9), Some(10), Some(11), None, Some(19)]
        );
        assert_eq!(page_items(19, 20), [Some(0), None, Some(18), Some(19)]);
        assert_eq!(page_items(0, 1), [Some(0)]);
        assert_eq!(
            page_items(3, 7),
            [
                Some(0),
                Some(1),
                Some(2),
                Some(3),
                Some(4),
                Some(5),
                Some(6)
            ]
        );
        assert_eq!(page_items(0, 8), [Some(0), Some(1), None, Some(7)]);
    }
}
