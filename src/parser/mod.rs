pub mod source;
pub mod javadoc;

use crate::resolver::{JarType, ResolvedJar};
use crate::store::{Store, LibraryId};
use anyhow::Result;

/// Parse a resolved JAR and populate the store
pub fn index_jar(jar: &ResolvedJar, lib: &LibraryId, store: &Store) -> Result<()> {
    match jar.jar_type {
        JarType::Javadoc => {
            javadoc::parse_javadoc_dir(jar.extracted_dir.path(), store)?;
            store.set_library_meta(lib, "javadoc")?;
        }
        JarType::Sources => {
            source::parse_source_dir(jar.extracted_dir.path(), store)?;
            store.set_library_meta(lib, "source")?;
        }
    }
    Ok(())
}
