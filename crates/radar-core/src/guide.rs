//! Text generated from `data/tags.toml` for people who write a tool file:
//! the comment block above `tags = [...]`, and the category list the
//! "suggest a tool" issue form must offer.

use crate::model::{CATEGORY_FACET, Facet, Taxonomy};

/// Where the issue form lives (relative to the repository root).
pub const ISSUE_FORM_PATH: &str = ".github/ISSUE_TEMPLATE/suggest-tool.yml";
/// Last option of the form's category dropdown.
pub const NOT_SURE: &str = "Not sure / わからない";
/// `id` of the category dropdown in the form.
pub const FORM_CATEGORY_ID: &str = "category";

/// `"1 to 3"`, `"exactly 2"`, `"up to 3"`, `"any number"`, `"at least 1"`.
fn count_rule(f: &Facet) -> String {
    match (f.min, f.max) {
        (min, Some(max)) if min == max => format!("exactly {max}"),
        (0, Some(max)) => format!("up to {max}"),
        (min, Some(max)) => format!("{min} to {max}"),
        (0, None) => "any number".into(),
        (min, None) => format!("at least {min}"),
    }
}

/// Comment lines (each starts with `# `, the last ends with `\n`) that list,
/// per facet, how many tags to pick and which ids exist.
pub fn tag_guide(tax: &Taxonomy) -> String {
    const WIDTH: usize = 78;
    let mut out = String::from("# Tags: pick ids from data/tags.toml, per facet.\n");
    for f in &tax.facets {
        let kind = if f.min > 0 { "required" } else { "optional" };
        let head = format!("#   {} ({}, {kind}): ", f.id, count_rule(f));
        let indent = "#       ";
        let mut line = head;
        let mut first = true;
        for (i, t) in f.tags.iter().enumerate() {
            let sep = if i + 1 < f.tags.len() { "," } else { "" };
            let word = format!("{}{sep}", t.id);
            if !first && line.len() + 1 + word.len() > WIDTH {
                out.push_str(line.trim_end());
                out.push('\n');
                line = indent.to_string();
                first = true;
            }
            if !first {
                line.push(' ');
            }
            line.push_str(&word);
            first = false;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Options the form's category dropdown must have, in order.
pub fn expected_category_options(tax: &Taxonomy) -> Vec<String> {
    let mut v: Vec<String> = tax
        .facets
        .iter()
        .filter(|f| f.id == CATEGORY_FACET)
        .flat_map(|f| f.tags.iter())
        .map(|t| t.label.en.clone().unwrap_or_else(|| t.id.clone()))
        .collect();
    v.push(NOT_SURE.to_string());
    v
}

/// Options of the dropdown with `id: <id>` in an issue-form YAML, or `None`
/// when there is no such dropdown. Reads the plain layout GitHub documents
/// (`- type: dropdown`, `id:`, `attributes:` / `options:` / `- Option`).
pub fn form_dropdown_options(yaml: &str, id: &str) -> Option<Vec<String>> {
    let lines: Vec<&str> = yaml.lines().collect();
    let id_line = lines.iter().position(|l| {
        let t = l.trim().trim_start_matches("- ").trim();
        t.strip_prefix("id:")
            .is_some_and(|v| unquote(v.trim()) == id)
    })?;
    let opts_line = lines[id_line..]
        .iter()
        .position(|l| l.trim() == "options:")?
        + id_line;
    // The block ends where the indentation drops below the `options:` key.
    let key_indent = indent_of(lines[opts_line]);
    let mut out = Vec::new();
    for l in &lines[opts_line + 1..] {
        if l.trim().is_empty() {
            continue;
        }
        let t = l.trim_start();
        if indent_of(l) < key_indent || !t.starts_with("- ") {
            break;
        }
        out.push(unquote(t[2..].trim()).to_string());
    }
    Some(out)
}

fn indent_of(l: &str) -> usize {
    l.len() - l.trim_start().len()
}

fn unquote(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAGS: &str = r#"
[[facet]]
id = "category"
label = { en = "Category", ja = "カテゴリ" }
min = 1
max = 3
  [[facet.tag]]
  id = "coding-agent"
  label = { en = "Coding agent", ja = "x" }
  [[facet.tag]]
  id = "sandbox"
  label = { en = "Sandbox", ja = "x" }

[[facet]]
id = "capability"
label = { en = "Capability", ja = "x" }
min = 0
  [[facet.tag]]
  id = "cli"
  label = { en = "CLI", ja = "x" }
"#;

    #[test]
    fn guide_lists_rules_and_ids_per_facet() {
        let tax = Taxonomy::parse(TAGS).unwrap();
        let g = tag_guide(&tax);
        assert!(g.lines().all(|l| l.starts_with('#')), "{g}");
        assert!(
            g.contains("#   category (1 to 3, required): coding-agent, sandbox\n"),
            "{g}"
        );
        assert!(
            g.contains("#   capability (any number, optional): cli\n"),
            "{g}"
        );
    }

    #[test]
    fn long_lists_wrap_inside_the_comment() {
        let mut text = String::from(
            "[[facet]]\nid = \"f\"\nlabel = { en = \"F\", ja = \"F\" }\nmin = 1\nmax = 1\n",
        );
        for i in 0..30 {
            text.push_str(&format!(
                "[[facet.tag]]\nid = \"tag-number-{i}\"\nlabel = {{ en = \"T\", ja = \"T\" }}\n"
            ));
        }
        let g = tag_guide(&Taxonomy::parse(&text).unwrap());
        assert!(g.contains("(exactly 1, required)"));
        assert!(g.lines().count() > 3);
        assert!(
            g.lines().all(|l| l.starts_with('#') && l.len() <= 80),
            "{g}"
        );
        assert!(g.contains("tag-number-29\n"));
    }

    #[test]
    fn expected_options_are_category_labels_then_not_sure() {
        let tax = Taxonomy::parse(TAGS).unwrap();
        assert_eq!(
            expected_category_options(&tax),
            vec!["Coding agent", "Sandbox", NOT_SURE]
        );
    }

    #[test]
    fn reads_dropdown_options() {
        let yaml = r#"
body:
  - type: input
    id: url
    attributes:
      label: URL
  - type: dropdown
    id: category
    attributes:
      label: Category / カテゴリ
      options:
        - Coding agent
        - "Not sure / わからない"
  - type: textarea
    id: why
    attributes:
      label: Why
"#;
        assert_eq!(
            form_dropdown_options(yaml, "category").unwrap(),
            vec!["Coding agent", "Not sure / わからない"]
        );
        assert_eq!(form_dropdown_options(yaml, "nope"), None);
    }
}
