use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, bail};
use radar_core::{Severity, Taxonomy};
use toml_edit::{Array, DocumentMut, Item};

use crate::project::Sources;
use crate::tool_file;

/// Prints all diagnostics; returns whether there were errors.
pub fn validate(root: &Path) -> Result<bool> {
    let sources = Sources::load(root)?;
    let (diags, _) = sources.validate();
    let mut errors = 0;
    let mut warnings = 0;
    for d in &diags {
        println!("{d}");
        match d.severity {
            Severity::Error => errors += 1,
            Severity::Warning => warnings += 1,
        }
    }
    if errors > 0 {
        eprintln!("{errors} error(s), {warnings} warning(s)");
    } else {
        println!("ok: {} tools, {warnings} warning(s)", sources.tools.len());
    }
    Ok(errors > 0)
}

/// Canonicalize every tool file. Returns the files that differ.
pub fn fmt(root: &Path, check: bool) -> Result<Vec<String>> {
    let sources = Sources::load(root)?;
    let mut changed = Vec::new();
    for f in &sources.tools {
        match tool_file::canonical_text(&f.text) {
            Ok(out) if out != f.text => {
                if !check {
                    std::fs::write(root.join(&f.path), out)?;
                }
                changed.push(f.path.clone());
            }
            Ok(_) => {}
            Err(e) => bail!("{}: {e}", f.path),
        }
    }
    Ok(changed)
}

pub fn tags_list(root: &Path) -> Result<()> {
    let sources = Sources::load(root)?;
    let (_, project) = sources.validate();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for t in &project.tools {
        for tag in &t.file.tags {
            *counts.entry(tag.as_str()).or_default() += 1;
        }
    }
    for f in &project.taxonomy.facets {
        println!("{} ({})", f.id, f.label.en.as_deref().unwrap_or(""));
        for t in &f.tags {
            let n = counts.get(t.id.as_str()).copied().unwrap_or(0);
            println!(
                "  {:<28} {n:>3}  {}",
                t.id,
                t.label.en.as_deref().unwrap_or("")
            );
        }
    }
    Ok(())
}

fn replace_tag(doc: &mut DocumentMut, from: &str, to: &str) -> bool {
    let Some(arr) = doc.get_mut("tags").and_then(Item::as_array_mut) else {
        return false;
    };
    if !arr.iter().any(|v| v.as_str() == Some(from)) {
        return false;
    }
    let mut out = Array::new();
    let mut seen = Vec::new();
    for v in arr.iter() {
        let s = v.as_str().unwrap_or_default();
        let s = if s == from { to } else { s };
        if !seen.contains(&s) {
            seen.push(s);
            out.push(s);
        }
    }
    *arr = out;
    true
}

fn add_alias(tag: &mut toml_edit::Table, alias: &str) {
    let arr = tag
        .entry("aliases")
        .or_insert_with(|| Item::Value(Array::new().into()))
        .as_array_mut();
    if let Some(arr) = arr
        && !arr.iter().any(|v| v.as_str() == Some(alias))
    {
        arr.push(alias);
    }
}

fn find_tag_table<'a>(
    doc: &'a mut DocumentMut,
    id: &str,
) -> Option<(usize, &'a mut toml_edit::ArrayOfTables, usize)> {
    let facets = doc.get_mut("facet")?.as_array_of_tables_mut()?;
    let n = facets.len();
    // Locate (facet index, tag index) first to satisfy the borrow checker.
    let mut found = None;
    for i in 0..n {
        let tags = facets
            .get(i)?
            .get("tag")
            .and_then(Item::as_array_of_tables)?;
        if let Some(j) = (0..tags.len()).find(|j| {
            tags.get(*j)
                .and_then(|t| t.get("id"))
                .and_then(Item::as_str)
                == Some(id)
        }) {
            found = Some((i, j));
            break;
        }
    }
    let (i, j) = found?;
    Some((i, facets, j))
}

/// `rename`: `from` becomes `to` (the old id stays as an alias).
/// `merge`: tag `from` is removed and becomes an alias of the existing `into`.
pub fn tags_change(root: &Path, from: &str, to: &str, merge: bool) -> Result<Vec<String>> {
    let sources = Sources::load(root)?;
    let (_, project) = sources.validate();
    let tax: &Taxonomy = &project.taxonomy;
    let Some((from_facet, _)) = tax.find_tag(from) else {
        bail!("tag \"{from}\" is not defined in data/tags.toml");
    };
    let to_def = tax.find_tag(to);
    if merge {
        let Some((to_facet, _)) = to_def else {
            bail!("tag \"{to}\" is not defined in data/tags.toml");
        };
        if to_facet.id != from_facet.id {
            bail!(
                "cannot merge across facets (\"{}\" is in {}, \"{to}\" in {})",
                from,
                from_facet.id,
                to_facet.id
            );
        }
    } else if to_def.is_some() || tax.find_alias(to).is_some() {
        bail!("\"{to}\" already exists in data/tags.toml");
    }

    // tags.toml
    let tags_path = root.join("data/tags.toml");
    let mut doc: DocumentMut = sources.tags.text.parse()?;
    {
        let (fi, facets, ti) = find_tag_table(&mut doc, from).expect("tag exists");
        if merge {
            let facet = facets.get_mut(fi).unwrap();
            let tags = facet
                .get_mut("tag")
                .and_then(Item::as_array_of_tables_mut)
                .unwrap();
            tags.remove(ti);
            for t in tags.iter_mut() {
                if t.get("id").and_then(Item::as_str) == Some(to) {
                    add_alias(t, from);
                }
            }
        } else {
            let t = facets
                .get_mut(fi)
                .unwrap()
                .get_mut("tag")
                .and_then(Item::as_array_of_tables_mut)
                .unwrap()
                .get_mut(ti)
                .unwrap();
            t["id"] = toml_edit::value(to);
            add_alias(t, from);
        }
    }
    std::fs::write(&tags_path, doc.to_string())?;

    // tools
    let mut touched = Vec::new();
    for f in &sources.tools {
        let mut d = tool_file::parse_doc(&f.text)?;
        if replace_tag(&mut d, from, to) {
            tool_file::write_doc(&root.join(&f.path), &mut d)?;
            touched.push(f.path.clone());
        }
    }
    Ok(touched)
}
