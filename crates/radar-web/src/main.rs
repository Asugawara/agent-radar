//! The interactive table: reads `#site-data`, replaces the static table in
//! `#tool-table` and fills `#filters`. State lives in the URL query.

use std::collections::BTreeMap;

use leptos::mount::mount_to;
use leptos::prelude::*;
use radar_core::{ALL_PLATFORMS, Platform, Pricing, Query, SiteData, SiteTool, SortKey};
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
fn sync_url(q: &Query) {
    let Some(w) = web_sys::window() else { return };
    let loc = w.location();
    let path = loc.pathname().unwrap_or_default();
    let hash = loc.hash().unwrap_or_default();
    let qs = q.to_query_string();
    let url = if qs.is_empty() {
        format!("{path}{hash}")
    } else {
        format!("{path}?{qs}{hash}")
    };
    if let Ok(h) = w.history() {
        let _ = h.replace_state_with_url(&JsValue::NULL, "", Some(&url));
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
    Effect::new(move |_| sync_url(&query.get()));

    let data = StoredValue::new(data);
    let total = data.with_value(|d| d.tools.len());

    let results = {
        let lang = lang.clone();
        Memo::new(move |_| {
            data.with_value(|d| {
                query
                    .get()
                    .apply(d, &lang)
                    .into_iter()
                    .cloned()
                    .collect::<Vec<SiteTool>>()
            })
        })
    };
    let counts = {
        let lang = lang.clone();
        Memo::new(move |_| data.with_value(|d| query.get().counts(d, &lang)))
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
                        let id2 = id.clone();
                        let id3 = id.clone();
                        let label = t.label.get(&lang_f).to_string();
                        view! {
                            <label class="opt">
                                <input type="checkbox"
                                    prop:checked=move || query.get().tags.contains(&id)
                                    on:change=move |ev| {
                                        let on = event_target_checked(&ev);
                                        query.update(|q| toggle(&mut q.tags, &id2, on));
                                    } />
                                " " {label} " "
                                <span class="count">{move || counts.get().tags.get(&id3).copied().unwrap_or(0)}</span>
                            </label>
                        }
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
        let plain_th = |k: &str, class: &'static str| view! { <th class=class>{s.t(k)}</th> };
        let head = view! {
            <tr>
                {sort_th(SortKey::Name, "table.name", "col-name")}
                {plain_th("table.category", "col-category")}
                {plain_th("table.pricing", "col-pricing")}
                {plain_th("table.oss", "col-oss")}
                {sort_th(SortKey::Stars, "table.stars", "col-stars num")}
                {sort_th(SortKey::Updated, "table.updated", "col-updated")}
            </tr>
        };

        let s_rows = s.clone();
        let lang = lang_t.clone();
        let base = base.clone();
        let generated_at = generated_at.clone();
        let rows = move || {
            let category_ids: Vec<String> = data.with_value(|d| {
                d.facets
                    .iter()
                    .filter(|f| f.id == radar_core::CATEGORY_FACET)
                    .flat_map(|f| f.tags.iter().map(|t| t.id.clone()))
                    .collect()
            });
            let empty = s_rows.t("table.empty");
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
                    let cats = t
                        .tags
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
                                        query.update(|q| {
                                            q.tags.insert(tag_id.clone());
                                        });
                                    }>
                                    {ic(&tag_icon(id))}{tag_label(id)}
                                </a>
                                " "
                            }
                        })
                        .collect_view();
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
                    view! {
                        <tr data-id=t.id.clone()>
                            <td class="col-name">
                                <div class="tool-cell">
                                    <span class="fav" inner_html=radar_core::icons::favicon_inner(t.favicon.as_deref(), &t.name, &t.id)></span>
                                    <div class="tool-main">
                                        <a class="tool-name" href=t.url.clone() target="_blank" rel="noopener">{t.name.clone()}</a>
                                        {repo_link}
                                        <div class="summary">{t.summary.get(&lang).to_string()}</div>
                                    </div>
                                </div>
                            </td>
                            <td class="col-category">{cats}</td>
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
