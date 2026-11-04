//! The interactive table: reads `#site-data`, replaces the static table in
//! `#tool-table` and fills `#filters`. State lives in the URL query.

use std::collections::{BTreeMap, BTreeSet};

use leptos::mount::mount_to;
use leptos::prelude::*;
use radar_core::{
    ALL_PLATFORMS, Platform, Pricing, Query, ReadState, SiteData, SiteTool, SortKey, TagMode,
};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Document, HtmlElement};

/// UI texts of the page language (`#ui-strings`).
#[derive(Clone, Default)]
struct Strings(BTreeMap<String, String>);

impl Strings {
    fn t(&self, key: &str) -> String {
        self.0
            .get(key)
            .cloned()
            .unwrap_or_else(|| format!("[[{key}]]"))
    }
}

/// `{shown} / {total}`-style template filling.
fn fill(template: &str, shown: usize, total: usize) -> String {
    template
        .replace("{shown}", &shown.to_string())
        .replace("{total}", &total.to_string())
}

/// A bundled Lucide icon, inlined (same DOM as the static HTML's `page.icon`).
fn ic(name: &str) -> AnyView {
    let svg = radar_core::icons::svg(name);
    if svg.is_empty() {
        return ().into_any();
    }
    view! { <span class="ic" inner_html=svg></span> }.into_any()
}

fn cats_empty(t: &SiteTool, category_ids: &[String]) -> bool {
    !t.tags.iter().any(|i| category_ids.contains(i))
}

/// One checkbox row (`label  count`) of a column filter popover.
fn opt<C, N, H>(label: String, checked: C, count: N, on_change: H) -> AnyView
where
    C: Fn() -> bool + Send + Sync + 'static,
    N: Fn() -> usize + Send + Sync + 'static,
    H: Fn(bool) + Send + Sync + 'static,
{
    view! {
        <label class="opt">
            <input type="checkbox"
                prop:checked=checked
                on:change=move |ev| on_change(event_target_checked(&ev)) />
            " " {label} " "
            <span class="count">{count}</span>
        </label>
    }
    .into_any()
}

/// One tag row: the checkbox includes the tag, the ⊖ button next to it (outside
/// the `<label>`, so pressing it never moves the checkbox) excludes it.
fn tag_opt<N>(
    id: String,
    label: String,
    exclude_label: String,
    query: RwSignal<Query>,
    count: N,
) -> AnyView
where
    N: Fn() -> usize + Send + Sync + 'static,
{
    let (id_checked, id_pressed, id_class, id_change, id_press) =
        (id.clone(), id.clone(), id.clone(), id.clone(), id);
    view! {
        <span class="opt-row" class:excluded=move || query.get().tag_mode(&id_class) == TagMode::Exclude>
            <label class="opt">
                <input type="checkbox"
                    prop:checked=move || query.get().tag_mode(&id_checked) == TagMode::Include
                    on:change=move |ev| {
                        let mode = if event_target_checked(&ev) { TagMode::Include } else { TagMode::Off };
                        query.update(|q| q.set_tag(&id_change, mode));
                    } />
                " " <span class="opt-name">{label}</span> " "
                <span class="count">{count}</span>
            </label>
            <button type="button" class="opt-x"
                aria-pressed=move || (query.get().tag_mode(&id_pressed) == TagMode::Exclude).to_string()
                aria-label=exclude_label.clone()
                title=exclude_label
                on:click=move |_| {
                    query.update(|q| {
                        let mode = if q.tag_mode(&id_press) == TagMode::Exclude { TagMode::Off } else { TagMode::Exclude };
                        q.set_tag(&id_press, mode);
                    });
                }>
                {ic("circle-minus")}
            </button>
        </span>
    }
    .into_any()
}

/// Width of a column filter popover (px) and the gap kept to the screen edge.
const POP_WIDTH: f64 = 288.0;
const POP_MARGIN: f64 = 8.0;

/// Where the popover goes: below the button, kept inside the viewport.
fn pop_position(button: &web_sys::Element) -> (f64, f64, f64) {
    let vw = web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(POP_WIDTH + 2.0 * POP_MARGIN);
    let width = POP_WIDTH.min(vw - 2.0 * POP_MARGIN);
    let r = button.get_bounding_client_rect();
    let left = r.left().min(vw - width - POP_MARGIN).max(POP_MARGIN);
    (left, r.bottom() + 4.0, width)
}

/// A table header cell with a filter button that opens a popover of choices.
/// `n` is the number of selected choices (the button is highlighted above 0).
#[allow(clippy::too_many_arguments)]
fn filter_th<H>(
    id: &'static str,
    class: &'static str,
    label: String,
    aria: String,
    clear_label: String,
    open_col: RwSignal<Option<&'static str>>,
    pos: RwSignal<(f64, f64, f64)>,
    n: Signal<usize>,
    on_clear: H,
    body: AnyView,
) -> impl IntoView
where
    H: Fn() + Send + Sync + 'static,
{
    let btn_id = format!("colfilter-btn-{id}");
    let pop_id = format!("colfilter-pop-{id}");
    let is_open = move || open_col.get() == Some(id);
    let btn_class = move || {
        if n.get() > 0 {
            "colfilter-btn active"
        } else {
            "colfilter-btn"
        }
    };
    view! {
        <th class=class>
            <span class="colfilter">
                <span class="thlabel">{label}</span>
                <button type="button"
                    class=btn_class
                    id=btn_id
                    aria-label=aria.clone()
                    aria-controls=pop_id.clone()
                    aria-expanded=move || if is_open() { "true" } else { "false" }
                    on:click=move |ev| {
                        if open_col.get_untracked() == Some(id) {
                            open_col.set(None);
                            return;
                        }
                        if let Some(el) = ev
                            .current_target()
                            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                        {
                            pos.set(pop_position(&el));
                        }
                        open_col.set(Some(id));
                    }>
                    {ic("list-filter")}
                    {move || {
                        let n = n.get();
                        (n > 0).then(|| view! { <span class="count-badge">{n}</span> })
                    }}
                </button>
                <div class="colfilter-pop" id=pop_id role="group" aria-label=aria
                    hidden=move || !is_open()
                    style=move || {
                        let (left, top, width) = pos.get();
                        format!("left:{left}px;top:{top}px;width:{width}px;max-height:calc(100vh - {top}px - {POP_MARGIN}px)")
                    }>
                    <div class="colfilter-opts">{body}</div>
                    <button type="button" class="colfilter-clear"
                        disabled=move || n.get() == 0
                        on:click=move |_| on_clear()>
                        {clear_label}
                    </button>
                </div>
            </span>
        </th>
    }
}

fn json_of(doc: &Document, id: &str) -> Option<String> {
    doc.get_element_by_id(id).and_then(|e| e.text_content())
}

fn current_query(data: &SiteData) -> Query {
    let search = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default();
    Query::parse(&search, data)
}

/// Mirror the state into the address bar without adding history entries.
/// Pairs the table does not own (`utm_*`, `gclid`, …) stay. `initial`: the
/// state was just read from the address bar, so it is left alone (a rewrite
/// would look like another page view to analytics).
fn sync_url(q: &Query, initial: bool) {
    let Some(w) = web_sys::window() else { return };
    let loc = w.location();
    let hash = loc.hash().unwrap_or_default();
    let qs = q.to_query_string();
    if !initial {
        let path = loc.pathname().unwrap_or_default();
        let search = loc.search().unwrap_or_default();
        let merged = radar_core::merge_query_string(&qs, &search);
        if merged != search.trim_start_matches('?') {
            let url = if merged.is_empty() {
                format!("{path}{hash}")
            } else {
                format!("{path}?{merged}{hash}")
            };
            if let Ok(h) = w.history() {
                let _ = h.replace_state_with_url(&JsValue::NULL, "", Some(&url));
            }
        }
    }
    sync_lang_switch(
        &w,
        &format!(
            "{}{hash}",
            if qs.is_empty() {
                String::new()
            } else {
                format!("?{qs}")
            }
        ),
    );
}

/// Keep the language switch link pointing at the other language with the current state.
fn sync_lang_switch(w: &web_sys::Window, suffix: &str) {
    let Some(doc) = w.document() else { return };
    let Some(a) = doc.get_element_by_id("lang-switch") else {
        return;
    };
    if let Some(base) = a.get_attribute("data-base") {
        let _ = a.set_attribute("href", &format!("{base}{suffix}"));
    }
}

fn toggle<T: Ord + Clone>(set: &mut std::collections::BTreeSet<T>, v: &T, on: bool) {
    if on {
        set.insert(v.clone());
    } else {
        set.remove(v);
    }
}

/// localStorage key of the ids of the tools the visitor has opened.
const READ_KEY: &str = "read";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// The read marks saved in the browser. Missing or broken data counts as none.
fn load_read() -> BTreeSet<String> {
    storage()
        .and_then(|s| s.get_item(READ_KEY).ok().flatten())
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

/// Save the read marks. Failures (private mode, full storage) are ignored.
fn save_read(ids: &BTreeSet<String>) {
    if let (Some(s), Ok(v)) = (storage(), serde_json::to_string(ids)) {
        let _ = s.set_item(READ_KEY, &v);
    }
}

/// Mark a tool as read, once, and save it.
fn mark_read(read: RwSignal<BTreeSet<String>>, id: &str) {
    if read.with_untracked(|r| r.contains(id)) {
        return;
    }
    read.update(|r| {
        r.insert(id.to_string());
        save_read(r);
    });
}

/// Runs when the WASM module is initialised in the browser.
#[wasm_bindgen::prelude::wasm_bindgen(start)]
fn start() {
    console_error_panic_hook::set_once();
    let _ = any_spawner::Executor::init_wasm_bindgen();
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let (Some(table_el), Some(filters_el)) = (
        doc.get_element_by_id("tool-table"),
        doc.get_element_by_id("filters"),
    ) else {
        return;
    };
    let Some(data) =
        json_of(&doc, "site-data").and_then(|s| serde_json::from_str::<SiteData>(&s).ok())
    else {
        return; // keep the static table
    };
    let strings = Strings(
        json_of(&doc, "ui-strings")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
    );
    let lang = doc
        .document_element()
        .map(|e| e.get_attribute("lang").unwrap_or_default())
        .filter(|l| l == "ja")
        .unwrap_or_else(|| "en".into());
    let base = table_el
        .get_attribute("data-base")
        .unwrap_or_else(|| "/".into());

    let query = RwSignal::new(current_query(&data));
    Effect::new(move |prev: Option<()>| sync_url(&query.get(), prev.is_none()));
    // Opened tools. Not tracked by the list itself, so a clicked row stays until the query changes.
    let read = RwSignal::new(load_read());

    let data = StoredValue::new(data);
    // The column filter popover that is open (at most one) and where it sits.
    let open_col: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let pop_pos = RwSignal::new((0.0, 0.0, POP_WIDTH));
    let category_ids: Vec<String> = data.with_value(|d| {
        d.facets
            .iter()
            .filter(|f| f.id == radar_core::CATEGORY_FACET)
            .flat_map(|f| f.tags.iter().map(|t| t.id.clone()))
            .collect()
    });
    // A scroll of the table would leave the popover behind its button.
    let close_on_scroll = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
        if open_col.get_untracked().is_some() {
            open_col.set(None);
        }
    });
    let _ = table_el
        .add_event_listener_with_callback("scroll", close_on_scroll.as_ref().unchecked_ref());
    close_on_scroll.forget();
    let total = data.with_value(|d| d.tools.len());

    // Opening a tool does not re-filter the list: a row opened under the
    // unread filter stays until the query changes, so rows do not shift.
    let results = {
        let lang = lang.clone();
        Memo::new(move |_| {
            let q = query.get();
            read.with_untracked(|r| {
                data.with_value(|d| {
                    q.apply(d, &lang, r)
                        .into_iter()
                        .cloned()
                        .collect::<Vec<SiteTool>>()
                })
            })
        })
    };
    let counts = {
        let lang = lang.clone();
        Memo::new(move |_| {
            let q = query.get();
            read.with(|r| data.with_value(|d| q.counts(d, &lang, r)))
        })
    };

    let tag_label = {
        let lang = lang.clone();
        move |id: &str| -> String {
            data.with_value(|d| {
                d.facets
                    .iter()
                    .flat_map(|f| f.tags.iter())
                    .find(|t| t.id == id)
                    .map(|t| t.label.get(&lang).to_string())
                    .unwrap_or_else(|| id.to_string())
            })
        }
    };

    let generated_at = data.with_value(|d| d.generated_at.clone());
    let tag_icon = move |id: &str| -> String {
        data.with_value(|d| {
            d.facets
                .iter()
                .flat_map(|f| f.tags.iter())
                .find(|t| t.id == id)
                .and_then(|t| t.icon.clone())
                .unwrap_or_default()
        })
    };

    // ---- search bar, filter button and (closed until clicked) filter panel ----
    let s = strings.clone();
    let lang_f = lang.clone();
    let filters = move || {
        let s = s.clone();
        let open = RwSignal::new(false);
        let facets = data.with_value(|d| d.facets.clone());
        let facet_views = facets
            .into_iter()
            .map(|f| {
                let tags = f
                    .tags
                    .iter()
                    .map(|t| {
                        let id = t.id.clone();
                        let id_n = id.clone();
                        let label = t.label.get(&lang_f).to_string();
                        let exclude_label = s.t("ui.exclude").replace("{label}", &label);
                        tag_opt(id, label, exclude_label, query, move || {
                            counts.get().tags.get(&id_n).copied().unwrap_or(0)
                        })
                    })
                    .collect_view();
                view! {
                    <fieldset class="facet">
                        <legend>{f.label.get(&lang_f).to_string()}</legend>
                        {tags}
                    </fieldset>
                }
            })
            .collect_view();

        let pricing_views = [Pricing::Free, Pricing::Freemium, Pricing::Paid]
            .into_iter()
            .map(|p| {
                let label = s.t(&format!("pricing.{}", p.as_str()));
                view! {
                    <label class="opt">
                        <input type="checkbox"
                            prop:checked=move || query.get().pricing.contains(&p)
                            on:change=move |ev| {
                                let on = event_target_checked(&ev);
                                query.update(|q| toggle(&mut q.pricing, &p, on));
                            } />
                        " " {label} " "
                        <span class="count">{move || counts.get().pricing.get(&p).copied().unwrap_or(0)}</span>
                    </label>
                }
            })
            .collect_view();

        let platform_views = ALL_PLATFORMS
            .into_iter()
            .map(|p: Platform| {
                let label = s.t(&format!("platform.{}", p.as_str()));
                view! {
                    <label class="opt">
                        <input type="checkbox"
                            prop:checked=move || query.get().platforms.contains(&p)
                            on:change=move |ev| {
                                let on = event_target_checked(&ev);
                                query.update(|q| toggle(&mut q.platforms, &p, on));
                            } />
                        " " {label} " "
                        <span class="count">{move || counts.get().platforms.get(&p).copied().unwrap_or(0)}</span>
                    </label>
                }
            })
            .collect_view();

        let read_views = [ReadState::Unread, ReadState::Read]
            .into_iter()
            .map(|st| {
                let label = s.t(&format!("ui.{}", st.as_str()));
                view! {
                    <label class="opt">
                        <input type="checkbox"
                            prop:checked=move || query.get().read.contains(&st)
                            on:change=move |ev| {
                                let on = event_target_checked(&ev);
                                query.update(|q| toggle(&mut q.read, &st, on));
                            } />
                        " " {label} " "
                        <span class="count">{move || counts.get().read.get(&st).copied().unwrap_or(0)}</span>
                    </label>
                }
            })
            .collect_view();

        let count_tpl = s.t("ui.count");
        let clear_label = s.t("ui.clear");
        view! {
            <div class="toolbar">
                <label class="searchbox">
                    {ic("search")}
                    <input type="search" class="search"
                        placeholder=s.t("ui.search_placeholder")
                        aria-label=s.t("ui.search_label")
                        prop:value=move || query.get().q
                        on:input=move |ev| {
                            let v = event_target_value(&ev);
                            query.update(|q| q.q = v);
                        } />
                </label>
                <button type="button" class="filter-toggle"
                    aria-controls="filter-panel"
                    aria-expanded=move || if open.get() { "true" } else { "false" }
                    on:click=move |_| open.update(|o| *o = !*o)>
                    {ic("sliders-horizontal")}
                    <span>{s.t("ui.filters")}</span>
                    {move || {
                        let n = query.get().filter_count();
                        (n > 0).then(|| view! { <span class="count-badge">{n}</span> })
                    }}
                </button>
                {move || (!query.get().is_empty()).then(|| {
                    let label = clear_label.clone();
                    view! {
                        <button type="button" class="clear"
                            on:click=move |_| query.update(|q| *q = q.cleared())>
                            {ic("x")}
                            <span>{label}</span>
                        </button>
                    }
                })}
                <span class="result-count" aria-live="polite">
                    {move || fill(&count_tpl, results.get().len(), total)}
                </span>
            </div>
            <div id="filter-panel" hidden=move || !open.get()>
                {facet_views}
                <fieldset class="facet">
                    <legend>{s.t("table.pricing")}</legend>
                    {pricing_views}
                </fieldset>
                <fieldset class="facet">
                    <legend>{s.t("table.platforms")}</legend>
                    {platform_views}
                    <label class="opt">
                        <input type="checkbox"
                            prop:checked=move || query.get().open_source
                            on:change=move |ev| {
                                let on = event_target_checked(&ev);
                                query.update(|q| q.open_source = on);
                            } />
                        " " {s.t("ui.oss_only")} " "
                        <span class="count">{move || counts.get().open_source}</span>
                    </label>
                </fieldset>
                <fieldset class="facet">
                    <legend>{s.t("ui.read_group")}</legend>
                    {read_views}
                </fieldset>
            </div>
        }
    };

    // ---- table ----
    let s = strings.clone();
    let lang_t = lang.clone();
    let table = move || {
        let sort_th = {
            let s = s.clone();
            move |key: SortKey, label_key: &'static str, class: &'static str| {
                let label = s.t(label_key);
                view! {
                    <th class=class
                        aria-sort=move || {
                            let so = query.get().sort;
                            if so.key != key { "none" } else if so.desc { "descending" } else { "ascending" }
                        }>
                        <button type="button" class="sort"
                            on:click=move |_| query.update(|q| {
                                if q.sort.key == key {
                                    q.sort.desc = !q.sort.desc;
                                } else {
                                    q.sort.key = key;
                                    q.sort.desc = key != SortKey::Name;
                                }
                            })>
                            {label}
                            {move || {
                                let so = query.get().sort;
                                if so.key != key {
                                    view! { <span class="dim">{ic("arrow-up-down")}</span> }.into_any()
                                } else if so.desc {
                                    ic("arrow-down")
                                } else {
                                    ic("arrow-up")
                                }
                            }}
                        </button>
                    </th>
                }
            }
        };
        // Esc, a click outside the popover and a resize close it.
        let _ = window_event_listener(leptos::ev::keydown, move |ev| {
            if ev.key() == "Escape"
                && let Some(id) = open_col.get_untracked()
            {
                open_col.set(None);
                if let Some(btn) = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.get_element_by_id(&format!("colfilter-btn-{id}")))
                    .and_then(|e| e.dyn_into::<HtmlElement>().ok())
                {
                    let _ = btn.focus();
                }
            }
        });
        let _ = window_event_listener(leptos::ev::click, move |ev| {
            if open_col.get_untracked().is_none() {
                return;
            }
            let inside = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .and_then(|e| e.closest(".colfilter-btn, .colfilter-pop").ok().flatten())
                .is_some();
            if !inside {
                open_col.set(None);
            }
        });
        let _ = window_event_listener(leptos::ev::resize, move |_| open_col.set(None));

        let clear_label = s.t("ui.clear");
        let filter_th_of = |id: &'static str, class: &'static str, label_key: &str| {
            let label = s.t(label_key);
            let aria = s.t("ui.filter_by").replace("{col}", &label);
            (id, class, label, aria)
        };

        let category_body = {
            let facets = data.with_value(|d| d.facets.clone());
            facets
                .into_iter()
                .filter(|f| f.id == radar_core::CATEGORY_FACET)
                .flat_map(|f| f.tags)
                .map(|t| {
                    let id_n = t.id.clone();
                    let label = t.label.get(&lang_t).to_string();
                    let exclude_label = s.t("ui.exclude").replace("{label}", &label);
                    tag_opt(t.id, label, exclude_label, query, move || {
                        counts.get().tags.get(&id_n).copied().unwrap_or(0)
                    })
                })
                .collect_view()
                .into_any()
        };
        let pricing_body = [Pricing::Free, Pricing::Freemium, Pricing::Paid]
            .into_iter()
            .map(|p| {
                opt(
                    s.t(&format!("pricing.{}", p.as_str())),
                    move || query.get().pricing.contains(&p),
                    move || counts.get().pricing.get(&p).copied().unwrap_or(0),
                    move |on| query.update(|q| toggle(&mut q.pricing, &p, on)),
                )
            })
            .collect_view()
            .into_any();
        let oss_body = opt(
            s.t("table.oss_yes"),
            move || query.get().open_source,
            move || counts.get().open_source,
            move |on| query.update(|q| q.open_source = on),
        );

        let cat_ids_n = category_ids.clone();
        let cat_ids_clear = category_ids.clone();
        let cat_n = Signal::derive(move || {
            let q = query.get();
            q.tags
                .iter()
                .chain(q.excluded.iter())
                .filter(|t| cat_ids_n.contains(*t))
                .count()
        });
        let pricing_n = Signal::derive(move || query.get().pricing.len());
        let oss_n = Signal::derive(move || usize::from(query.get().open_source));

        let (id, class, label, aria) = filter_th_of("category", "col-category", "table.category");
        let category_th = filter_th(
            id,
            class,
            label,
            aria,
            clear_label.clone(),
            open_col,
            pop_pos,
            cat_n,
            move || {
                query.update(|q| {
                    q.tags.retain(|t| !cat_ids_clear.contains(t));
                    q.excluded.retain(|t| !cat_ids_clear.contains(t));
                })
            },
            category_body,
        );
        let (id, class, label, aria) = filter_th_of("pricing", "col-pricing", "table.pricing");
        let pricing_th = filter_th(
            id,
            class,
            label,
            aria,
            clear_label.clone(),
            open_col,
            pop_pos,
            pricing_n,
            move || query.update(|q| q.pricing.clear()),
            pricing_body,
        );
        let (id, class, label, aria) = filter_th_of("oss", "col-oss", "table.oss");
        let oss_th = filter_th(
            id,
            class,
            label,
            aria,
            clear_label,
            open_col,
            pop_pos,
            oss_n,
            move || query.update(|q| q.open_source = false),
            oss_body,
        );

        let head = view! {
            <tr>
                {sort_th(SortKey::Name, "table.name", "col-name")}
                {category_th}
                {pricing_th}
                {oss_th}
                {sort_th(SortKey::Stars, "table.stars", "col-stars num")}
                {sort_th(SortKey::Updated, "table.updated", "col-updated")}
            </tr>
        };

        let s_rows = s.clone();
        let category_ids = category_ids.clone();
        let lang = lang_t.clone();
        let base = base.clone();
        let generated_at = generated_at.clone();
        let rows = move || {
            let empty = s_rows.t("table.empty");
            let unread = s_rows.t("ui.unread");
            let list = results.get();
            if list.is_empty() {
                let msg = if total == 0 {
                    empty
                } else {
                    s_rows.t("ui.no_match")
                };
                return view! { <tr><td colspan="6" class="muted">{msg}</td></tr> }.into_any();
            }
            list.into_iter()
                .map(|t| {
                    // Shown in the category column, and (phones) under the summary.
                    let cats = || {
                        t.tags
                            .iter()
                            .filter(|i| category_ids.contains(i))
                            .map(|id| {
                                let href = format!("{base}?tag={id}");
                                let tag_id = id.clone();
                                view! {
                                    <a class="badge badge-category" href=href
                                        on:click=move |ev| {
                                            // Let ctrl/cmd/shift/middle clicks open the link normally.
                                            if ev.button() != 0 || ev.ctrl_key() || ev.meta_key() || ev.shift_key() || ev.alt_key() {
                                                return;
                                            }
                                            ev.prevent_default();
                                            query.update(|q| q.set_tag(&tag_id, TagMode::Include));
                                        }>
                                        {ic(&tag_icon(id))}{tag_label(id)}
                                    </a>
                                    " "
                                }
                            })
                            .collect_view()
                    };
                    let tool_cats = (!cats_empty(&t, &category_ids))
                        .then(|| view! { <div class="tool-cats">{cats()}</div> });
                    let oss = if t.open_source {
                        view! { <td class="col-oss" title=s_rows.t("table.oss_yes")>{ic("git-branch")}</td> }.into_any()
                    } else {
                        view! { <td class="col-oss muted" title=s_rows.t("table.oss_no")>"—"</td> }.into_any()
                    };
                    let stars = match t.stars {
                        Some(n) => view! {
                            <td class="col-stars num" title=n.to_string()>
                                {ic("star")}{radar_core::icons::short_count(n)}
                            </td>
                        }
                        .into_any(),
                        None => view! { <td class="col-stars num muted">"—"</td> }.into_any(),
                    };
                    let updated = match t
                        .pushed_at
                        .as_deref()
                        .and_then(|p| radar_core::icons::relative_time(p, &generated_at, &lang).map(|r| (r, p.chars().take(10).collect::<String>())))
                    {
                        Some((rel, day)) => view! {
                            <td class="col-updated" title=day>{rel}</td>
                        }
                        .into_any(),
                        None => view! { <td class="col-updated muted">"—"</td> }.into_any(),
                    };
                    let repo_link = t.repo.as_ref().map(|r| {
                        view! {
                            <a class="repo-link" href=format!("https://github.com/{r}")
                                target="_blank" rel="noopener" title="GitHub" aria-label="GitHub">
                                {ic("code")}
                            </a>
                        }
                    });
                    // Blue dot until the tool is opened (tracked, so it disappears on click).
                    let tid = t.id.clone();
                    let dot_label = unread.clone();
                    let unread_dot = move || {
                        (!read.with(|r| r.contains(&tid))).then(|| {
                            view! { <span class="unread-dot" role="img" aria-label=dot_label.clone() title=dot_label.clone()></span> }
                        })
                    };
                    let (click_id, aux_id) = (t.id.clone(), t.id.clone());
                    view! {
                        <tr data-id=t.id.clone()>
                            <td class="col-name">
                                <div class="tool-cell">
                                    <span class="fav" inner_html=radar_core::icons::favicon_inner(t.favicon.as_deref(), &t.name, &t.id)></span>
                                    <div class="tool-main">
                                        {unread_dot}
                                        <a class="tool-name" href=t.url.clone() target="_blank" rel="noopener"
                                            on:click=move |_| mark_read(read, &click_id)
                                            on:auxclick=move |ev| {
                                                if ev.button() == 1 {
                                                    mark_read(read, &aux_id);
                                                }
                                            }>{t.name.clone()}</a>
                                        {repo_link}
                                        <div class="summary">{t.summary.get(&lang).to_string()}</div>
                                        {tool_cats}
                                    </div>
                                </div>
                            </td>
                            <td class="col-category">{cats()}</td>
                            <td class="col-pricing">{s_rows.t(&format!("pricing.{}", t.pricing.as_str()))}</td>
                            {oss}
                            {stars}
                            {updated}
                        </tr>
                    }
                })
                .collect_view()
                .into_any()
        };
        view! {
            <table>
                <thead>{head}</thead>
                <tbody>{rows}</tbody>
            </table>
        }
    };

    // Replace the static content only now that everything is ready.
    filters_el.set_inner_html("");
    table_el.set_inner_html("");
    if let (Ok(f), Ok(t)) = (
        filters_el.dyn_into::<HtmlElement>(),
        table_el.dyn_into::<HtmlElement>(),
    ) {
        mount_to(f, filters).forget();
        mount_to(t, table).forget();
    }
}

/// Entry point required of a binary crate; the real one is `start`.
fn main() {}

#[cfg(test)]
mod tests {
    use super::fill;

    #[test]
    fn fill_replaces_both_placeholders() {
        assert_eq!(fill("{shown} / {total}", 3, 30), "3 / 30");
        assert_eq!(fill("{total}件中{shown}件", 1, 2), "2件中1件");
    }
}
