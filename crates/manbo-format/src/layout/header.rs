use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// 文件头魔数（本改版写出的 `.qj` 用它）。
pub const MAGIC: [u8; 8] = *b"MANBO\0\0\0";

/// 上游（青简）用过的魔数：**只用于读**。
///
/// 随包数据（词库 / 语言模型 / 释义表 / 整句模型）都是上游时期生成的，重建之前文件头都是这个魔数；
/// 认它在改名之后仍能读旧数据。重跑数据管线后新文件一律是 [`MAGIC`]。
pub const UPSTREAM_MAGIC: [u8; 8] = *b"QINGJIAN";

/// 读时接受的魔数：本版与上游。
pub const ACCEPTED_MAGIC: [[u8; 8]; 2] = [MAGIC, UPSTREAM_MAGIC];

/// 文件头魔数是不是认识的 `.qj`（兼容上游数据）。
pub fn is_known_magic(magic: [u8; 8]) -> bool {
    ACCEPTED_MAGIC.contains(&magic)
}

/// 当前格式版本。布局不兼容时加一，读旧版本的代码按需保留。
pub const FORMAT_VERSION: u16 = 1;

/// 文件头，32 字节，紧跟着 `section_count` 条 [`crate::SectionEntry`]。
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct Header {
    /// [`MAGIC`]。
    pub magic: [u8; 8],

    /// [`FORMAT_VERSION`]。
    pub version: u16,

    /// 数据种类，见 [`crate::Kind`]。
    pub kind: u16,

    /// 分节数。
    pub section_count: u32,

    /// 留给以后（校验和、标志位），现在全零。
    pub reserved: [u8; 16],
}

impl Header {
    pub const SIZE: usize = size_of::<Self>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_upstream_magic_so_old_data_still_loads() {
        assert!(is_known_magic(MAGIC));
        assert!(is_known_magic(UPSTREAM_MAGIC));
        assert!(!is_known_magic(*b"NOTAQJ!\0"));
    }
}
