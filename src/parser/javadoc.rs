use crate::store::{FieldRow, MethodRow, Store, TypeRow};
use anyhow::Result;
use scraper::{Html, Selector};
use std::path::Path;
use tracing::warn;

pub fn parse_javadoc_dir(dir: &Path, store: &Store) -> Result<()> {
    // Parse package-summary pages first
    walk_html_files(dir, &mut |path| {
        let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
        if filename == "package-summary.html" {
            let content = std::fs::read_to_string(path)?;
            if let Err(e) = parse_package_summary(&content, dir, path, store) {
                warn!("Failed to parse package summary {}: {}", path.display(), e);
            }
        }
        Ok(())
    })?;

    // Parse class pages
    walk_html_files(dir, &mut |path| {
        let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
        if filename.starts_with("package-")
            || filename.starts_with("index")
            || filename.starts_with("allclasses")
            || filename.starts_with("allpackages")
            || filename.starts_with("overview")
            || filename == "help-doc.html"
            || filename == "deprecated-list.html"
            || filename == "serialized-form.html"
            || filename == "constant-values.html"
        {
            return Ok(());
        }

        let content = std::fs::read_to_string(path)?;
        if let Err(e) = parse_class_page(&content, dir, path, store) {
            warn!("Failed to parse javadoc page {}: {}", path.display(), e);
        }
        Ok(())
    })?;

    Ok(())
}

fn walk_html_files(dir: &Path, handler: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_html_files(&path, handler)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("html") {
            handler(&path)?;
        }
    }
    Ok(())
}

fn path_to_package(base_dir: &Path, file_path: &Path) -> String {
    let parent = file_path.parent().unwrap_or(file_path);
    let relative = parent.strip_prefix(base_dir).unwrap_or(parent);
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(".")
}

fn parse_package_summary(html: &str, base_dir: &Path, path: &Path, store: &Store) -> Result<()> {
    let doc = Html::parse_document(html);
    let package_name = path_to_package(base_dir, path);
    if package_name.is_empty() {
        return Ok(());
    }

    let desc_sel =
        Selector::parse(".package-description .block, .contentContainer .block").unwrap();
    let doc_comment = doc
        .select(&desc_sel)
        .next()
        .map(|el| el.text().collect::<String>());

    store.insert_package(&package_name, doc_comment.as_deref())?;
    Ok(())
}

fn parse_class_page(html: &str, base_dir: &Path, path: &Path, store: &Store) -> Result<()> {
    let doc = Html::parse_document(html);
    let package_name = path_to_package(base_dir, path);

    // Extract class name from title or heading
    let title_sel = Selector::parse("h1.title, h2.title, .header h1, .header h2").unwrap();
    let class_name_raw = doc
        .select(&title_sel)
        .next()
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default();

    let (kind, class_name) = parse_type_heading(&class_name_raw);
    let class_name = if class_name.is_empty() {
        // Fallback: derive from filename
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string()
    } else {
        class_name
    };

    if class_name.is_empty() {
        return Ok(());
    }

    let pkg_id = store.insert_package(&package_name, None)?;
    let fqn = if package_name.is_empty() {
        class_name.clone()
    } else {
        format!("{}.{}", package_name, class_name)
    };

    // Extract class-level doc
    let desc_sel = Selector::parse(
        ".class-description .block, .description .block, .contentContainer > .description .block",
    )
    .unwrap();
    let doc_comment = doc
        .select(&desc_sel)
        .next()
        .map(|el| el.text().collect::<String>());

    let type_row = TypeRow {
        id: 0,
        package_id: pkg_id,
        name: class_name,
        fqn,
        kind: kind.to_string(),
        doc_comment,
        annotations: None,
        superclass: None,
        interfaces: None,
    };
    let type_id = store.insert_type(&type_row)?;

    parse_method_details(&doc, type_id, store)?;
    parse_field_details(&doc, type_id, store)?;

    Ok(())
}

fn parse_type_heading(heading: &str) -> (&str, String) {
    let heading = heading.trim();
    let prefixes = [
        ("Class ", "class"),
        ("Interface ", "interface"),
        ("Enum Class ", "enum"),
        ("Enum ", "enum"),
        ("Annotation Type ", "annotation"),
        ("Annotation Interface ", "annotation"),
        ("Record Class ", "record"),
    ];
    for (prefix, kind) in prefixes {
        if let Some(rest) = heading.strip_prefix(prefix) {
            let name = rest.split('<').next().unwrap_or(rest).trim().to_string();
            return (kind, name);
        }
    }
    let name = heading
        .split('<')
        .next()
        .unwrap_or(heading)
        .trim()
        .to_string();
    ("class", name)
}

fn parse_method_details(doc: &Html, type_id: i64, store: &Store) -> Result<()> {
    let detail_sel = Selector::parse(
        ".method-details .member-list > li, \
         section.method-details ul.member-list > li, \
         #method-detail ul.member-list > li",
    )
    .unwrap();

    let old_detail_sel = Selector::parse(
        ".details .blockList .blockList, \
         .memberSummary tbody tr",
    )
    .unwrap();

    let sig_sel = Selector::parse(".member-signature, .memberSignature, pre").unwrap();
    let block_sel = Selector::parse(".block").unwrap();

    for sel in [&detail_sel, &old_detail_sel] {
        let elements: Vec<_> = doc.select(sel).collect();
        if elements.is_empty() {
            continue;
        }

        for el in elements {
            let sig = el
                .select(&sig_sel)
                .next()
                .map(|s| s.text().collect::<String>());
            let description = el
                .select(&block_sel)
                .next()
                .map(|b| b.text().collect::<String>());

            let signature = sig.unwrap_or_default().trim().to_string();
            if signature.is_empty() {
                continue;
            }

            let name = extract_method_name_from_sig(&signature);

            let method_row = MethodRow {
                id: 0,
                type_id,
                name,
                signature: signature.clone(),
                return_type: None,
                params: "[]".to_string(),
                doc_comment: description,
                annotations: None,
                is_static: signature.contains("static "),
            };
            store.insert_method(&method_row)?;
        }
        break;
    }

    Ok(())
}

fn parse_field_details(doc: &Html, type_id: i64, store: &Store) -> Result<()> {
    let detail_sel = Selector::parse(
        ".field-details .member-list > li, \
         section.field-details ul.member-list > li",
    )
    .unwrap();

    let sig_sel = Selector::parse(".member-signature, .memberSignature, pre").unwrap();
    let block_sel = Selector::parse(".block").unwrap();

    for el in doc.select(&detail_sel) {
        let sig = el
            .select(&sig_sel)
            .next()
            .map(|s| s.text().collect::<String>());
        let description = el
            .select(&block_sel)
            .next()
            .map(|b| b.text().collect::<String>());

        let signature = sig.unwrap_or_default().trim().to_string();
        if signature.is_empty() {
            continue;
        }

        let parts: Vec<&str> = signature.split_whitespace().collect();
        let name = parts.last().unwrap_or(&"").to_string();
        let field_type = parts.iter().rev().nth(1).unwrap_or(&"").to_string();

        let field_row = FieldRow {
            id: 0,
            type_id,
            name,
            field_type,
            doc_comment: description,
            annotations: None,
            is_static: signature.contains("static "),
        };
        store.insert_field(&field_row)?;
    }

    Ok(())
}

fn extract_method_name_from_sig(sig: &str) -> String {
    if let Some(paren_idx) = sig.find('(') {
        let before_paren = sig[..paren_idx].trim();
        before_paren
            .split_whitespace()
            .last()
            .unwrap_or("")
            .to_string()
    } else {
        sig.split_whitespace().last().unwrap_or("").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_type_heading() {
        assert_eq!(
            parse_type_heading("Class ImmutableList<E>"),
            ("class", "ImmutableList".into())
        );
        assert_eq!(
            parse_type_heading("Interface Predicate<T>"),
            ("interface", "Predicate".into())
        );
        assert_eq!(parse_type_heading("Enum Color"), ("enum", "Color".into()));
        assert_eq!(
            parse_type_heading("Annotation Type Override"),
            ("annotation", "Override".into())
        );
        assert_eq!(
            parse_type_heading("Record Class Point"),
            ("record", "Point".into())
        );
    }

    #[test]
    fn test_extract_method_name_from_sig() {
        assert_eq!(
            extract_method_name_from_sig("public void doSomething(String s)"),
            "doSomething"
        );
        assert_eq!(
            extract_method_name_from_sig("static <T> List<T> of(T... elements)"),
            "of"
        );
        assert_eq!(extract_method_name_from_sig("String getName()"), "getName");
    }

    #[test]
    fn test_path_to_package() {
        let base = Path::new("/tmp/docs");
        let file = Path::new("/tmp/docs/com/example/Foo.html");
        assert_eq!(path_to_package(base, file), "com.example");
    }
}
