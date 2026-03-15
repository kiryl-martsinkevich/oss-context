use crate::store::{FieldRow, MethodRow, Store, TypeRow};
use anyhow::Result;
use std::path::Path;
use tracing::warn;

pub fn parse_source_dir(dir: &Path, store: &Store) -> Result<()> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .expect("Failed to load Java grammar");

    walk_java_files(dir, &mut |path| {
        let content = std::fs::read_to_string(path)?;
        parse_java_file(&mut parser, &content, store)?;
        Ok(())
    })?;
    Ok(())
}

fn walk_java_files(dir: &Path, handler: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_java_files(&path, handler)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("java") {
            if let Err(e) = handler(&path) {
                warn!("Failed to parse {}: {}", path.display(), e);
            }
        }
    }
    Ok(())
}

fn parse_java_file(parser: &mut tree_sitter::Parser, source: &str, store: &Store) -> Result<()> {
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse Java source"))?;

    let root = tree.root_node();

    // Extract package name
    let package_name = extract_package_name(root, source).unwrap_or_default();
    let pkg_id = if !package_name.is_empty() {
        store.insert_package(&package_name, None)?
    } else {
        store.insert_package("(default)", None)?
    };

    // Extract type declarations
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "class_declaration" | "interface_declaration" | "enum_declaration"
            | "annotation_type_declaration" | "record_declaration" => {
                parse_type_declaration(child, source, &package_name, pkg_id, store)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn extract_package_name(root: tree_sitter::Node, source: &str) -> Option<String> {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "package_declaration" {
            let mut inner = child.walk();
            for c in child.children(&mut inner) {
                if c.kind() == "scoped_identifier" || c.kind() == "identifier" {
                    return Some(node_text(c, source).to_string());
                }
            }
        }
    }
    None
}

fn parse_type_declaration(
    node: tree_sitter::Node,
    source: &str,
    package_name: &str,
    pkg_id: i64,
    store: &Store,
) -> Result<()> {
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(n, source).to_string())
        .unwrap_or_default();

    if name.is_empty() {
        return Ok(());
    }

    let fqn = if package_name.is_empty() {
        name.clone()
    } else {
        format!("{}.{}", package_name, name)
    };

    let kind = match node.kind() {
        "class_declaration" => "class",
        "interface_declaration" => "interface",
        "enum_declaration" => "enum",
        "annotation_type_declaration" => "annotation",
        "record_declaration" => "record",
        _ => "class",
    };

    // Extract doc comment (preceding comment node)
    let doc_comment = extract_preceding_doc_comment(node, source);

    // Extract annotations
    let annotations = extract_annotations(node, source);

    // Extract superclass
    let superclass = node
        .child_by_field_name("superclass")
        .and_then(|sc| {
            let mut c = sc.walk();
            let result = sc.children(&mut c)
                .find(|n| n.kind() == "type_identifier" || n.kind() == "scoped_type_identifier")
                .map(|n| node_text(n, source).to_string());
            result
        });

    // Extract interfaces
    let interfaces_node = node.child_by_field_name("interfaces");
    let interfaces = interfaces_node.map(|ifaces| {
        let mut c = ifaces.walk();
        let list: Vec<String> = ifaces
            .children(&mut c)
            .filter(|n| n.kind() == "type_identifier" || n.kind() == "scoped_type_identifier")
            .collect::<Vec<_>>()
            .into_iter()
            .map(|n| node_text(n, source).to_string())
            .collect();
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
    });

    let type_row = TypeRow {
        id: 0,
        package_id: pkg_id,
        name: name.clone(),
        fqn: fqn.clone(),
        kind: kind.to_string(),
        doc_comment,
        annotations: annotations.map(|a| serde_json::to_string(&a).unwrap_or_default()),
        superclass,
        interfaces,
    };
    let type_id = store.insert_type(&type_row)?;

    // Parse body for methods and fields
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            match child.kind() {
                "method_declaration" | "constructor_declaration" => {
                    parse_method(child, source, type_id, store)?;
                }
                "field_declaration" => {
                    parse_field(child, source, type_id, store)?;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn parse_method(
    node: tree_sitter::Node,
    source: &str,
    type_id: i64,
    store: &Store,
) -> Result<()> {
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(n, source).to_string())
        .unwrap_or_else(|| "<init>".to_string());

    let signature = node_text(node, source)
        .lines()
        .next()
        .unwrap_or("")
        .trim_end_matches('{')
        .trim()
        .to_string();

    let return_type = node
        .child_by_field_name("type")
        .map(|n| node_text(n, source).to_string());

    let params = extract_params(node, source);
    let doc_comment = extract_preceding_doc_comment(node, source);
    let annotations = extract_annotations(node, source);
    let is_static = has_modifier(node, source, "static");

    let method_row = MethodRow {
        id: 0,
        type_id,
        name,
        signature,
        return_type,
        params: serde_json::to_string(&params).unwrap_or_else(|_| "[]".to_string()),
        doc_comment,
        annotations: annotations.map(|a| serde_json::to_string(&a).unwrap_or_default()),
        is_static,
    };
    store.insert_method(&method_row)?;
    Ok(())
}

fn parse_field(
    node: tree_sitter::Node,
    source: &str,
    type_id: i64,
    store: &Store,
) -> Result<()> {
    let field_type = node
        .child_by_field_name("type")
        .map(|n| node_text(n, source).to_string())
        .unwrap_or_default();

    // A field_declaration can have multiple declarators
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            let name = child
                .child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }

            let doc_comment = extract_preceding_doc_comment(node, source);
            let annotations = extract_annotations(node, source);
            let is_static = has_modifier(node, source, "static");

            let field_row = FieldRow {
                id: 0,
                type_id,
                name,
                field_type: field_type.clone(),
                doc_comment,
                annotations: annotations.map(|a| serde_json::to_string(&a).unwrap_or_default()),
                is_static,
            };
            store.insert_field(&field_row)?;
        }
    }
    Ok(())
}

fn extract_preceding_doc_comment(node: tree_sitter::Node, source: &str) -> Option<String> {
    let mut prev = node.prev_sibling();
    // Skip annotations to find doc comment
    while let Some(p) = prev {
        if p.kind() == "block_comment" || p.kind() == "line_comment" {
            let text = node_text(p, source);
            if text.starts_with("/**") {
                // Strip /** and */ and leading * from each line
                let cleaned = text
                    .trim_start_matches("/**")
                    .trim_end_matches("*/")
                    .lines()
                    .map(|l| l.trim().trim_start_matches('*').trim())
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                return Some(cleaned);
            }
            return None; // Non-javadoc comment, stop
        } else if p.kind() == "modifiers"
            || p.kind() == "marker_annotation"
            || p.kind() == "annotation"
        {
            prev = p.prev_sibling();
        } else {
            break;
        }
    }
    None
}

fn extract_annotations(node: tree_sitter::Node, source: &str) -> Option<Vec<String>> {
    let mut annotations = Vec::new();
    // Try modifiers field first (method/field declarations)
    if let Some(modifiers) = node.child_by_field_name("modifiers") {
        let mut cursor = modifiers.walk();
        for child in modifiers.children(&mut cursor) {
            if child.kind() == "marker_annotation"
                || child.kind() == "annotation"
                || child.kind() == "single_element_annotation"
            {
                annotations.push(node_text(child, source).to_string());
            }
        }
    }
    // Also check direct children (class/interface declarations)
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "marker_annotation"
            || child.kind() == "annotation"
            || child.kind() == "single_element_annotation"
            || child.kind() == "modifiers"
        {
            if child.kind() == "modifiers" {
                let mut mc = child.walk();
                for gc in child.children(&mut mc) {
                    if gc.kind() == "marker_annotation"
                        || gc.kind() == "annotation"
                        || gc.kind() == "single_element_annotation"
                    {
                        let text = node_text(gc, source).to_string();
                        if !annotations.contains(&text) {
                            annotations.push(text);
                        }
                    }
                }
            } else {
                let text = node_text(child, source).to_string();
                if !annotations.contains(&text) {
                    annotations.push(text);
                }
            }
        }
    }
    if annotations.is_empty() {
        None
    } else {
        Some(annotations)
    }
}

fn has_modifier(node: tree_sitter::Node, source: &str, modifier: &str) -> bool {
    if let Some(modifiers) = node.child_by_field_name("modifiers") {
        let mut cursor = modifiers.walk();
        for child in modifiers.children(&mut cursor) {
            if node_text(child, source) == modifier {
                return true;
            }
        }
    }
    false
}

#[derive(serde::Serialize)]
struct ParamInfo {
    name: String,
    #[serde(rename = "type")]
    param_type: String,
}

fn extract_params(method_node: tree_sitter::Node, source: &str) -> Vec<ParamInfo> {
    let mut params = Vec::new();
    if let Some(param_list) = method_node.child_by_field_name("parameters") {
        let mut cursor = param_list.walk();
        for child in param_list.children(&mut cursor) {
            if child.kind() == "formal_parameter" || child.kind() == "spread_parameter" {
                let param_type = child
                    .child_by_field_name("type")
                    .map(|n| node_text(n, source).to_string())
                    .unwrap_or_default();
                let name = child
                    .child_by_field_name("name")
                    .map(|n| node_text(n, source).to_string())
                    .unwrap_or_default();
                params.push(ParamInfo { name, param_type });
            }
        }
    }
    params
}

fn node_text<'a>(node: tree_sitter::Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use tempfile::TempDir;

    fn test_store() -> (Store, TempDir) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let store = Store::open(&db_path).unwrap();
        (store, dir)
    }

    #[test]
    fn test_parse_simple_class() {
        let (store, _dir) = test_store();
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .unwrap();

        let source = r#"
package com.example;

/**
 * A sample class.
 */
public class Foo extends Bar implements Baz {
    private String name;

    /**
     * Does something.
     */
    public void doSomething(String input) {
        // body
    }

    public static int count() {
        return 0;
    }
}
"#;
        parse_java_file(&mut parser, source, &store).unwrap();

        let packages = store.list_packages().unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "com.example");

        let types = store.list_types_in_package("com.example").unwrap();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].name, "Foo");
        assert_eq!(types[0].kind, "class");
        assert!(types[0]
            .doc_comment
            .as_ref()
            .unwrap()
            .contains("sample class"));

        let methods = store.get_methods_for_type(types[0].id).unwrap();
        assert_eq!(methods.len(), 2);

        let fields = store.get_fields_for_type(types[0].id).unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "name");
    }

    #[test]
    fn test_parse_interface() {
        let (store, _dir) = test_store();
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .unwrap();

        let source = r#"
package com.example;

public interface MyInterface {
    void doIt();
    String getName();
}
"#;
        parse_java_file(&mut parser, source, &store).unwrap();
        let types = store.list_types_in_package("com.example").unwrap();
        assert_eq!(types[0].kind, "interface");
        let methods = store.get_methods_for_type(types[0].id).unwrap();
        assert_eq!(methods.len(), 2);
    }

    #[test]
    fn test_parse_annotated_class() {
        let (store, _dir) = test_store();
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .unwrap();

        let source = r#"
package com.example;

@Deprecated
public class Old {
    @Override
    public String toString() { return "old"; }
}
"#;
        parse_java_file(&mut parser, source, &store).unwrap();
        let types = store.list_types_in_package("com.example").unwrap();
        assert!(types[0]
            .annotations
            .as_ref()
            .unwrap()
            .contains("@Deprecated"));
    }

    #[test]
    fn test_search_after_parsing() {
        let (store, _dir) = test_store();
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .unwrap();

        let source = r#"
package com.example;

/**
 * Immutable list implementation.
 */
public class ImmutableList {
    /**
     * Creates an empty immutable list.
     */
    public static ImmutableList of() { return null; }
}
"#;
        parse_java_file(&mut parser, source, &store).unwrap();
        let results = store.search("immutable list", 10).unwrap();
        assert!(!results.is_empty());
    }
}
