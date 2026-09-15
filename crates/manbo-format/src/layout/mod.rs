//! 文件里的固定布局：文件头、分节表条目、数据种类编号、元数据分节的内容。

mod header;
mod kind;
mod metadata;
mod section_entry;

pub use header::{ACCEPTED_MAGIC, FORMAT_VERSION, Header, is_known_magic, MAGIC, UPSTREAM_MAGIC};
pub use kind::Kind;
pub use metadata::Metadata;
pub use section_entry::SectionEntry;
