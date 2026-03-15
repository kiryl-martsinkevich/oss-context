use oss_context::store::Store;
use oss_context::parser::source::parse_source_dir;
use tempfile::{NamedTempFile, TempDir};
use std::fs;

#[test]
fn test_end_to_end_source_indexing_and_search() {
    // Create a fake source JAR directory
    let src_dir = TempDir::new().unwrap();
    let pkg_dir = src_dir.path().join("com").join("example");
    fs::create_dir_all(&pkg_dir).unwrap();

    fs::write(pkg_dir.join("Calculator.java"), r#"
package com.example;

/**
 * A simple calculator for arithmetic operations.
 */
public class Calculator {
    /**
     * Adds two numbers together.
     * @param a first number
     * @param b second number
     * @return the sum
     */
    public int add(int a, int b) {
        return a + b;
    }

    /**
     * Subtracts b from a.
     */
    public int subtract(int a, int b) {
        return a - b;
    }

    /**
     * Multiplies two numbers.
     */
    public static int multiply(int a, int b) {
        return a * b;
    }
}
"#).unwrap();

    fs::write(pkg_dir.join("StringUtils.java"), r#"
package com.example;

/**
 * Utility class for string operations.
 */
public final class StringUtils {
    /**
     * Checks if a string is empty or null.
     */
    public static boolean isEmpty(String s) {
        return s == null || s.isEmpty();
    }
}
"#).unwrap();

    // Create store and index
    let db_file = NamedTempFile::new().unwrap();
    let store = Store::open(db_file.path()).unwrap();
    parse_source_dir(src_dir.path(), &store).unwrap();

    // Test package listing
    let packages = store.list_packages().unwrap();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].name, "com.example");

    // Test type listing
    let types = store.list_types_in_package("com.example").unwrap();
    assert_eq!(types.len(), 2);

    // Test search
    let results = store.search("calculator arithmetic", 10).unwrap();
    assert!(!results.is_empty());
    assert!(results[0].fqn.contains("Calculator"));

    // Test search for method
    let results = store.search("adds two numbers", 10).unwrap();
    assert!(!results.is_empty());

    // Test browse by FQN
    let calc = store.get_type_by_fqn("com.example.Calculator").unwrap().unwrap();
    assert_eq!(calc.kind, "class");

    let methods = store.get_methods_for_type(calc.id).unwrap();
    assert_eq!(methods.len(), 3); // add, subtract, multiply

    // Verify static method
    let multiply = methods.iter().find(|m| m.name == "multiply").unwrap();
    assert!(multiply.is_static);
}
